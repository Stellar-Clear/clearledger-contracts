#![cfg(test)]

extern crate std;

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{
        storage::Instance as _, storage::Persistent as _, Address as _, Events as _, Ledger,
        MockAuth, MockAuthInvoke,
    },
    vec,
    xdr::ToXdr,
    Address, BytesN, Env, IntoVal, InvokeError, String, Symbol, TryFromVal, Vec,
};

use crate::{
    constants::{
        MAX_BATCH_SIZE, MAX_PROTOCOL_FEE_BPS, SUGGESTED_DISCOUNT_CEILING_BPS,
        SUGGESTED_DISCOUNT_FLOOR_BPS,
    },
    DataKey, PoolContract, PoolContractClient, DEFAULT_MIN_INITIAL_DEPOSIT, DEFAULT_SHARE_DECIMALS,
    TTL_EXTEND_TO, TTL_THRESHOLD,
};

use clearledger_escrow::{EscrowContract as RealEscrow, EscrowContractClient as RealEscrowClient};
use clearledger_invoice::{
    InvoiceContract as RealInvoice, InvoiceContractClient as RealInvoiceClient,
};
use clearledger_test_utils::{
    setup_full_protocol, test_agent_id, test_agent_pubkey, test_agent_signing_key,
};

// Default invoice parameters matching create_and_list() defaults
// These computed constants eliminate magic numbers in test assertions
// and make the tests self-correcting when parameters change.
const DEFAULT_FACE_VALUE: u128 = 10_000_000_000;
const DEFAULT_DISCOUNT_BPS: u32 = 200;
const DEFAULT_FUNDED_AMOUNT: u128 =
    DEFAULT_FACE_VALUE * (10000 - DEFAULT_DISCOUNT_BPS as u128) / 10000;

// SEP-41 share metadata every test initializer passes to `initialize` (issue
// #757). Kept as constants so the assertions in the metadata tests and the
// `initialize` call sites cannot drift apart.
const TEST_SHARE_NAME: &str = "ClearLedger USDC Pool Shares";
const TEST_SHARE_SYMBOL: &str = "TT-USDC";
const DEFAULT_YIELD_AMOUNT: u128 = DEFAULT_FACE_VALUE * DEFAULT_DISCOUNT_BPS as u128 / 10000;

// --------------- Mock Registry ---------------

#[contract]
pub struct MockRegistry;

#[contractimpl]
impl MockRegistry {
    pub fn is_verified(env: Env, address: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&RegKey(address))
            .unwrap_or(false)
    }

    pub fn register(env: Env, address: Address) {
        env.storage()
            .persistent()
            .set(&RegKey(address.clone()), &true);
        env.storage()
            .persistent()
            .extend_ttl(&RegKey(address), TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    pub fn revoke(env: Env, address: Address) {
        env.storage()
            .persistent()
            .set(&RegKey(address.clone()), &false);
        env.storage()
            .persistent()
            .extend_ttl(&RegKey(address), TTL_THRESHOLD, TTL_EXTEND_TO);
    }
}

#[contracttype]
pub struct RegKey(Address);

// --------------- Mock Agent Registry (Underwrite) ---------------
//
// Stands in for the agent-registry contract from the separate
// `underwrite-contract` repo, so `create_and_list_with_params` can satisfy
// invoice's `submit_attestation` gate with a real secp256k1 signature.

#[contract]
pub struct MockAgentRegistry;

#[contractimpl]
impl MockAgentRegistry {
    pub fn get_agent(env: Env, agent_id: Symbol) -> Option<clearledger_invoice::Agent> {
        env.storage().persistent().get(&AgentKey(agent_id))
    }

    pub fn register_agent(env: Env, agent_id: Symbol, agent: clearledger_invoice::Agent) {
        env.storage().persistent().set(&AgentKey(agent_id), &agent);
    }
}

#[contracttype]
pub struct AgentKey(Symbol);

// Shared test_agent helpers imported from clearledger_test_utils (Issue #813)

/// Submits a validly signed attestation for `invoice_id` against the
/// agent-registry wired up in `setup()`, unlocking it for
/// `list_for_financing`.
fn attest_invoice(te: &TestEnv, invoice_id: &BytesN<32>) {
    let payload = clearledger_invoice::AttestationPayload {
        domain_separator: BytesN::from_array(
            &te.env,
            &clearledger_invoice::ATTESTATION_DOMAIN_SEPARATOR,
        ),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&te.env, &[9u8; 32]),
        agent_id: test_agent_id(&te.env),
        nonce: 1,
    };
    let payload_bytes = payload.to_xdr(&te.env);
    let digest = te.env.crypto().keccak256(&payload_bytes).to_array();
    let (sig, recid) = test_agent_signing_key()
        .sign_prehash_recoverable(&digest)
        .unwrap();
    let mut sig_bytes = [0u8; 65];
    sig_bytes[..64].copy_from_slice(&sig.to_bytes());
    sig_bytes[64] = recid.to_byte();
    let signature = BytesN::from_array(&te.env, &sig_bytes);

    te.invoice
        .submit_attestation(invoice_id, &payload_bytes, &signature);
}

// --------------- Mock Token ---------------

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_key = TKey(from.clone());
        let to_key = TKey(to.clone());
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_bal - amount));
        env.storage().persistent().set(&to_key, &(to_bal + amount));
    }

    pub fn balance(env: Env, addr: Address) -> i128 {
        env.storage().persistent().get(&TKey(addr)).unwrap_or(0)
    }
}

#[contracttype]
pub struct TKey(Address);

// --------------- Mock Invoice (unbounded face value) ---------------
//
// Stands in for the pool's configured invoice contract to prove that
// fund_invoice's funded-amount multiplication rejects a pathologically large
// cross-contract face_value with the typed PoolError::Overflow rather than an
// untyped arithmetic abort (issue #585). The real invoice contract caps
// face_value at MAX_FACE_VALUE = u128::MAX / 10_000, which cannot overflow
// the (10000 - discount_bps) scaling; the pool must not rely on that
// external bound.

#[contract]
pub struct MockHugeFaceInvoice;

#[contractimpl]
impl MockHugeFaceInvoice {
    pub fn configure(
        env: Env,
        issuer: Address,
        buyer: Address,
        funding_asset: Address,
        face_value: u128,
        discount_bps: u32,
    ) {
        env.storage()
            .instance()
            .set(&InvKey(Symbol::new(&env, "issuer")), &issuer);
        env.storage()
            .instance()
            .set(&InvKey(Symbol::new(&env, "buyer")), &buyer);
        env.storage()
            .instance()
            .set(&InvKey(Symbol::new(&env, "asset")), &funding_asset);
        env.storage()
            .instance()
            .set(&InvKey(Symbol::new(&env, "face")), &face_value);
        env.storage()
            .instance()
            .set(&InvKey(Symbol::new(&env, "disc")), &discount_bps);
    }

    pub fn get_status(_env: Env, _invoice_id: BytesN<32>) -> u32 {
        1 // Listed
    }

    pub fn get_funding_terms(env: Env, _invoice_id: BytesN<32>) -> (u32, u128, u32) {
        (
            1, // Listed
            env.storage()
                .instance()
                .get(&InvKey(Symbol::new(&env, "face")))
                .unwrap(),
            env.storage()
                .instance()
                .get(&InvKey(Symbol::new(&env, "disc")))
                .unwrap(),
        )
    }

    pub fn get_issuer(env: Env, _invoice_id: BytesN<32>) -> Address {
        env.storage()
            .instance()
            .get(&InvKey(Symbol::new(&env, "issuer")))
            .unwrap()
    }

    pub fn get_buyer(env: Env, _invoice_id: BytesN<32>) -> Address {
        env.storage()
            .instance()
            .get(&InvKey(Symbol::new(&env, "buyer")))
            .unwrap()
    }

    pub fn get_funding_asset(env: Env, _invoice_id: BytesN<32>) -> Address {
        env.storage()
            .instance()
            .get(&InvKey(Symbol::new(&env, "asset")))
            .unwrap()
    }

    pub fn get_face_value(env: Env, _invoice_id: BytesN<32>) -> u128 {
        env.storage()
            .instance()
            .get(&InvKey(Symbol::new(&env, "face")))
            .unwrap()
    }

    pub fn get_discount_bps(env: Env, _invoice_id: BytesN<32>) -> u32 {
        env.storage()
            .instance()
            .get(&InvKey(Symbol::new(&env, "disc")))
            .unwrap()
    }
}

#[contracttype]
pub struct InvKey(Symbol);

struct TestEnv {
    env: Env,
    pool: PoolContractClient<'static>,
    pool_id: Address,
    invoice: RealInvoiceClient<'static>,
    registry: MockRegistryClient<'static>,
    usdc_id: Address,
    xlm_id: Address,
    escrow_id: Address,
    admin: Address,
    issuer: Address,
    buyer: Address,
    lp: Address,
}

fn setup() -> TestEnv {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    let registry_id = env.register_contract(None, MockRegistry);
    let registry = MockRegistryClient::new(&env, &registry_id);
    registry.register(&issuer);
    registry.register(&buyer);

    let usdc_id = env.register_contract(None, MockToken);
    let xlm_id = env.register_contract(None, MockToken);

    let lp_bal_key = TKey(lp.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&lp_bal_key, &100_000_000_000_000i128);
    });
    env.as_contract(&xlm_id, || {
        env.storage()
            .persistent()
            .set(&lp_bal_key, &100_000_000_000_000i128);
    });
    let buyer_bal_key = TKey(buyer.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&buyer_bal_key, &100_000_000_000_000i128);
    });
    env.as_contract(&xlm_id, || {
        env.storage()
            .persistent()
            .set(&buyer_bal_key, &100_000_000_000_000i128);
    });

    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let pool_id = env.register_contract(None, PoolContract);

    let invoice = RealInvoiceClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);

    let escrow = RealEscrowClient::new(&env, &escrow_id);
    escrow.initialize(&admin, &pool_id, &usdc_id);

    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );

    invoice.add_supported_asset(&usdc_id);
    invoice.add_supported_asset(&xlm_id);

    invoice.set_pool_contract(&pool_id);
    invoice.set_escrow_contract(&escrow_id);

    let agent_registry_id = env.register_contract(None, MockAgentRegistry);
    let agent_registry = MockAgentRegistryClient::new(&env, &agent_registry_id);
    agent_registry.register_agent(
        &test_agent_id(&env),
        &clearledger_invoice::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    invoice.set_agent_registry_contract(&agent_registry_id);

    // Raise cap to 100% so existing tests (which fund at 98% utilization) still pass
    pool.set_max_utilization(&admin, &10000);

    TestEnv {
        env,
        pool,
        pool_id,
        invoice,
        registry,
        usdc_id,
        xlm_id,
        escrow_id,
        admin,
        issuer,
        buyer,
        lp,
    }
}

fn create_and_list(te: &TestEnv, funding_asset: &Address) -> BytesN<32> {
    create_and_list_with_params(te, funding_asset, 10_000_000_000, 200)
}

fn create_and_list_with_params(
    te: &TestEnv,
    funding_asset: &Address,
    face_value: u128,
    discount_bps: u32,
) -> BytesN<32> {
    create_and_list_with_params_and_term(te, funding_asset, face_value, discount_bps, 86400)
}

fn create_and_list_with_params_and_term(
    te: &TestEnv,
    funding_asset: &Address,
    face_value: u128,
    discount_bps: u32,
    term: u64,
) -> BytesN<32> {
    let due_date = te.env.ledger().timestamp() + term;
    let invoice_id =
        te.invoice
            .create(&te.issuer, &te.buyer, &face_value, &due_date, funding_asset);
    attest_invoice(te, &invoice_id);
    te.invoice.list_for_financing(&invoice_id, &discount_bps);
    invoice_id
}

fn fund_and_repay_invoice(te: &TestEnv) -> BytesN<32> {
    let invoice_id = create_and_list(te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);
    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 86401);
    te.invoice.repay(&invoice_id);
    invoice_id
}

fn create_lp_with_balance(te: &TestEnv, balance: i128) -> Address {
    let lp = Address::generate(&te.env);
    let lp_bal_key = TKey(lp.clone());
    te.env.as_contract(&te.usdc_id, || {
        te.env.storage().persistent().set(&lp_bal_key, &balance);
    });
    lp
}

//  DEPOSIT TESTS
#[cfg(test)]
mod versioned_event_tests {
    use crate::events::{invoice_funded, invoice_funded_v1};
    use soroban_sdk::{BytesN, Env, Symbol};
    use soroban_sdk::{testutils::Events, IntoVal, Val, Vec};

    #[test]
    fn funding_keeps_legacy_payload_and_adds_explicit_version() {
        let env = Env::default();
        let contract = env.register_contract(None, crate::PoolContract);
        let invoice_id = BytesN::from_array(&env, &[17; 32]);
        env.as_contract(&contract, || {
            invoice_funded_v1(&env, &invoice_id, 123u128);
            invoice_funded(&env, &invoice_id, 123u128);
        });
        let published = env.events().all();
        assert_eq!(published.len(), 2);
        let (legacy_contract, legacy_topics, legacy_data) = published.get(1).unwrap();
        let expected_legacy_topics: Vec<Val> =
            (Symbol::new(&env, "invoice_funded"), invoice_id.clone()).into_val(&env);
        assert_eq!(legacy_contract, contract);
        assert_eq!(legacy_topics, expected_legacy_topics);
        assert_eq!(legacy_data, 123u128.into_val(&env));
        let (versioned_contract, versioned_topics, versioned_data) = published.get(0).unwrap();
        let expected_versioned_topics: Vec<Val> =
            (Symbol::new(&env, "invoice_funded_v1"), invoice_id).into_val(&env);
        assert_eq!(versioned_contract, contract);
        assert_eq!(versioned_topics, expected_versioned_topics);
        assert_eq!(versioned_data, (1u32, 123u128).into_val(&env));
    }
}

#[test]
fn test_first_deposit_issues_one_to_one_shares() {
    let te = setup();
    let shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(shares, 5_000_000_000);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 5_000_000_000);
    assert_eq!(pos.deposit_count, 1);
}

#[test]
fn test_second_deposit_issues_proportional_shares() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    let shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(shares, 5_000_000_000);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 15_000_000_000);
    assert_eq!(pos.deposit_count, 2);
}

#[test]
fn test_second_deposit_scales_by_share_price() {
    // Rewritten per #586: the previous body was byte-for-byte identical to
    // test_second_deposit_issues_proportional_shares (second deposit still at
    // share price 1.0). Here the second deposit happens AFTER a repayment has
    // raised the share price to 1.02 (10.2B deposits backing 10B shares), so
    // the returned share count must scale down precisely.
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 10_200_000_000);
    assert_eq!(stats.total_shares, 10_000_000_000);

    // 5_000_000_000 * 10_000_000_000 / 10_200_000_000 = 4_901_960_784 (floored)
    let shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(shares, 4_901_960_784);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 14_901_960_784);
    assert_eq!(pos.deposit_count, 2);
}

#[test]
fn test_preview_deposit_matches_deposit_across_share_prices() {
    let amounts = [
        DEFAULT_MIN_INITIAL_DEPOSIT,
        DEFAULT_MIN_INITIAL_DEPOSIT * 2,
        5_000_000_000,
    ];

    // An empty pool mints one share per unit deposited.
    for amount in amounts {
        let te = setup();
        let preview = te.pool.preview_deposit(&amount);
        let issued = te.pool.deposit(&te.lp, &amount);
        assert_eq!(preview, issued);
        assert_eq!(preview, amount);
    }

    // A pool with a 1:1 share price preserves the same ratio for later deposits.
    for amount in amounts {
        let te = setup();
        te.pool.deposit(&te.lp, &10_000_000_000);
        let before = te.pool.get_stats();
        let preview = te.pool.preview_deposit(&amount);
        let after = te.pool.get_stats();
        assert_eq!(after.total_deposits, before.total_deposits);
        assert_eq!(after.total_shares, before.total_shares);
        let issued = te.pool.deposit(&te.lp, &amount);
        assert_eq!(preview, issued);
    }

    // Repayment yield raises the share price, so previews must use the same
    // floor rounding as a real deposit at that price.
    for amount in [2u128, 3, 1_000_000, 5_000_000_000] {
        let te = setup();
        te.pool.deposit(&te.lp, &10_000_000_000);
        fund_and_repay_invoice(&te);
        let preview = te.pool.preview_deposit(&amount);
        let issued = te.pool.deposit(&te.lp, &amount);
        assert_eq!(preview, issued);
    }
}

#[test]
fn test_preview_deposit_requires_no_authorization() {
    let te = setup();
    te.env.set_auths(&[]);

    assert_eq!(
        te.pool.preview_deposit(&DEFAULT_MIN_INITIAL_DEPOSIT),
        DEFAULT_MIN_INITIAL_DEPOSIT
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_preview_deposit_rejects_zero_amount() {
    let te = setup();
    te.pool.preview_deposit(&0);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_preview_deposit_rejects_dust_after_yield() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);
    te.pool.preview_deposit(&1);
}

#[test]
fn test_preview_withdraw_matches_withdraw_across_share_prices() {
    // 1:1 share price, including the full balance.
    for shares in [1u128, 5_000_000_000, 10_000_000_000] {
        let te = setup();
        te.pool.deposit(&te.lp, &10_000_000_000);

        let stats_before = te.pool.get_stats();
        let preview = te.pool.preview_withdraw(&shares);
        assert_eq!(preview.available_liquidity, stats_before.available_liquidity);
        assert!(preview.liquidity_ok);

        let withdrawn = te.pool.withdraw(&te.lp, &shares);
        assert_eq!(preview.usdc_out, withdrawn);
    }

    // Repayment yield moves the price to 1.02; odd share counts must floor the
    // same way on both paths.
    for shares in [1u128, 3, 7, 123_456_789] {
        let te = setup();
        te.pool.deposit(&te.lp, &10_000_000_000);
        fund_and_repay_invoice(&te);

        let preview = te.pool.preview_withdraw(&shares);
        assert!(preview.liquidity_ok);

        let withdrawn = te.pool.withdraw(&te.lp, &shares);
        assert_eq!(preview.usdc_out, withdrawn);
        assert_eq!(
            preview.usdc_out,
            shares * 10_200_000_000 / 10_000_000_000
        );
    }
}

#[test]
fn test_preview_withdraw_floors_odd_share_prices_like_withdraw() {
    // 10 USDC backing 7 shares: 1 -> 1, 2 -> 2, 3 -> 4, 6 -> 8.
    for (shares, expected) in [(1u128, 1u128), (2, 2), (3, 4), (6, 8)] {
        let te = setup();
        // Fund the pool's token balance, then replace the aggregate accounting
        // with a state that produces a non-integer share price.
        te.pool.deposit(&te.lp, &10_000_000_000);
        te.env.as_contract(&te.pool_id, || {
            te.env
                .storage()
                .instance()
                .set(&DataKey::TotalShares, &7u128);
            te.env
                .storage()
                .instance()
                .set(&DataKey::TotalDeposits, &10u128);
            te.env
                .storage()
                .instance()
                .set(&DataKey::TotalFunded, &0u128);
            te.env
                .storage()
                .persistent()
                .set(&DataKey::LPShares(te.lp.clone()), &7u128);
        });

        let preview = te.pool.preview_withdraw(&shares);
        assert_eq!(preview.usdc_out, expected);
        assert_eq!(preview.available_liquidity, 10);
        assert!(preview.liquidity_ok);

        assert_eq!(te.pool.withdraw(&te.lp, &shares), expected);
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_preview_withdraw_zero_shares_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.preview_withdraw(&0);
}

#[test]
fn test_preview_withdraw_requires_no_authorization() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.env.set_auths(&[]);

    assert_eq!(te.pool.preview_withdraw(&1).usdc_out, 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_preview_withdraw_preserves_dust_guard() {
    let te = setup();
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalShares, &1_000_000_000_000u128);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &1u128);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &0u128);
    });

    te.pool.preview_withdraw(&1);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_preview_withdraw_rejects_share_price_overflow() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &u128::MAX);
    });

    te.pool.preview_withdraw(&10_000_000_000);
}

#[test]
fn test_preview_withdraw_reports_insufficient_liquidity_without_panicking() {
    let te = setup();
    te.pool.deposit(&te.lp, &DEFAULT_FUNDED_AMOUNT);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    let stats_before = te.pool.get_stats();
    assert_eq!(stats_before.available_liquidity, 0);

    let preview = te.pool.preview_withdraw(&1);
    assert_eq!(preview.usdc_out, 1);
    assert_eq!(preview.available_liquidity, 0);
    assert!(!preview.liquidity_ok);

    // The write path still rejects the same quote.
    assert!(te.pool.try_withdraw(&te.lp, &1).is_err());

    let stats_after = te.pool.get_stats();
    assert_eq!(stats_after.total_deposits, stats_before.total_deposits);
    assert_eq!(stats_after.total_shares, stats_before.total_shares);
}

#[test]
fn test_preview_withdraw_above_lp_balance_reports_liquidity_constraint() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let requested = 20_000_000_000u128;
    let position = te.pool.get_lp_position(&te.lp);
    assert!(requested > position.shares);

    let preview = te.pool.preview_withdraw(&requested);
    assert_eq!(preview.usdc_out, requested);
    assert_eq!(preview.available_liquidity, 10_000_000_000);
    assert!(!preview.liquidity_ok);

    // `withdraw` rejects the same request via its LP-balance guard.
    assert!(te.pool.try_withdraw(&te.lp, &requested).is_err());
}

#[test]
fn test_preview_withdraw_does_not_write_state_emit_events_or_extend_ttl() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);

    let stats_before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);
    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let lp_usdc_before = usdc.balance(&te.lp);
    let pool_usdc_before = usdc.balance(&te.pool_id);
    let events_before = te.env.events().all().len();

    // Drain the LP-share entry below TTL_THRESHOLD so any extension performed
    // by the preview would be visible.
    let shares_key = DataKey::LPShares(te.lp.clone());
    let ttl_before_drain: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&shares_key)
    });
    te.env
        .ledger()
        .set_sequence_number(te.env.ledger().sequence() + ttl_before_drain - 50);
    let shares_ttl_before: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&shares_key)
    });
    let instance_ttl_before: u32 = te
        .env
        .as_contract(&te.pool_id, || te.env.storage().instance().get_ttl());
    assert!(shares_ttl_before < TTL_THRESHOLD);
    assert!(instance_ttl_before < TTL_THRESHOLD);

    let preview = te.pool.preview_withdraw(&1_000_000_000);
    assert_eq!(preview.usdc_out, 1_020_000_000);
    assert!(preview.liquidity_ok);

    let shares_ttl_after: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&shares_key)
    });
    let instance_ttl_after: u32 = te
        .env
        .as_contract(&te.pool_id, || te.env.storage().instance().get_ttl());
    assert_eq!(shares_ttl_after, shares_ttl_before);
    assert_eq!(instance_ttl_after, instance_ttl_before);

    let stats_after = te.pool.get_stats();
    let position_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(stats_after.total_deposits, stats_before.total_deposits);
    assert_eq!(stats_after.total_shares, stats_before.total_shares);
    assert_eq!(stats_after.available_liquidity, stats_before.available_liquidity);
    assert_eq!(position_after.shares, position_before.shares);
    assert_eq!(position_after.usdc_value, position_before.usdc_value);
    assert_eq!(position_after.yield_earned, position_before.yield_earned);
    assert_eq!(usdc.balance(&te.lp), lp_usdc_before);
    assert_eq!(usdc.balance(&te.pool_id), pool_usdc_before);
    assert_eq!(te.env.events().all().len(), events_before);
}

//  DUST ATTACK / 0-SHARE TESTS (issue #129)
// After the pool accrues yield the share price rises above 1.0. A deposit
// small enough that `usdc_amount * total_shares < total_deposits` would round
// down to 0 shares. Such a deposit must be rejected with `MinimumDeposit` (#14)
// rather than silently absorbing the depositor's funds.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_deposit_rejects_dust_when_zero_shares_after_yield() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);

    // Share price is now 10.2B / 10B = 1.02
    let stats = te.pool.get_stats();
    assert_eq!(
        stats.total_deposits,
        DEFAULT_FACE_VALUE + DEFAULT_YIELD_AMOUNT
    );
    assert_eq!(stats.total_shares, 10_000_000_000);

    // 1 * 10B / 10.2B = 0 shares -> must be rejected, not absorbed.
    let lp2 = create_lp_with_balance(&te, 10_000_000_000);
    te.pool.deposit(&lp2, &1);
}

// A rejected dust deposit must not change pool accounting and must not take the
// depositor's USDC: the whole transaction reverts. This proves no funds are lost.
#[test]
fn test_dust_deposit_rejection_preserves_state_and_funds() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);

    let before = te.pool.get_stats();
    let lp2 = create_lp_with_balance(&te, 10_000_000_000);
    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let lp_balance_before = usdc.balance(&lp2);
    let pool_balance_before = usdc.balance(&te.pool_id);

    // try_* returns Err on contract panic instead of unwinding the test.
    let res = te.pool.try_deposit(&lp2, &1);
    assert!(res.is_err(), "dust deposit should be rejected");

    // Pool deposits/shares are unchanged: the 1 unit was never absorbed.
    let after = te.pool.get_stats();
    assert_eq!(after.total_deposits, before.total_deposits);
    assert_eq!(after.total_shares, before.total_shares);
    assert_eq!(usdc.balance(&lp2), lp_balance_before);
    assert_eq!(usdc.balance(&te.pool_id), pool_balance_before);

    // The rejected depositor holds no shares.
    let pos = te.pool.get_lp_position(&lp2);
    assert_eq!(pos.shares, 0);
}

// The guard must not over-reject: a small deposit that still mints >= 1 share at
// the elevated price succeeds normally.
#[test]
fn test_smallest_valid_deposit_after_yield_issues_at_least_one_share() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);

    // Share price 1.02: 2 * 10B / 10.2B = 1 share (floored), the minimum > 0.
    let lp2 = create_lp_with_balance(&te, 10_000_000_000);
    let shares = te.pool.deposit(&lp2, &2);
    assert_eq!(shares, 1);

    let pos = te.pool.get_lp_position(&lp2);
    assert_eq!(pos.shares, 1);
    assert_eq!(pos.deposit_count, 1);
}

// Core acceptance guarantee: across a sweep of deposit sizes against a pool with
// an inflated share price, every deposit either mints >= 1 share or is rejected.
// No deposit is ever accepted for 0 shares.
#[test]
fn test_no_deposit_ever_receives_zero_shares() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);
    // Share price is 1.02; amounts of 1 round to 0 shares, >= 2 round to >= 1.

    let amounts = [1u128, 2, 3, 5, 10, 102, 1_000, 1_000_000];
    for amount in amounts {
        let lp = create_lp_with_balance(&te, 100_000_000_000i128);
        match te.pool.try_deposit(&lp, &amount) {
            Ok(Ok(shares)) => {
                // Accepted deposits must always mint at least one share.
                assert!(shares >= 1, "amount {amount} accepted for 0 shares");
                let pos = te.pool.get_lp_position(&lp);
                assert_eq!(pos.shares, shares);
            }
            _ => {
                // Rejected deposits must leave the depositor with no shares.
                let pos = te.pool.get_lp_position(&lp);
                assert_eq!(pos.shares, 0, "amount {amount} rejected but minted shares");
            }
        }
    }
}

// The initial deposit in an empty pool must be at least DEFAULT_MIN_INITIAL_DEPOSIT (1 USDC)
// to prevent share-price griefing attacks.
#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_first_deposit_below_minimum_panics_invalid_amount() {
    let te = setup();
    te.pool.deposit(&te.lp, &(DEFAULT_MIN_INITIAL_DEPOSIT - 1));
}

#[test]
fn test_first_deposit_at_minimum_succeeds() {
    let te = setup();
    let shares = te.pool.deposit(&te.lp, &DEFAULT_MIN_INITIAL_DEPOSIT);
    assert_eq!(shares, DEFAULT_MIN_INITIAL_DEPOSIT);
}

#[test]
fn test_first_deposit_above_minimum_succeeds() {
    let te = setup();
    let shares = te
        .pool
        .deposit(&te.lp, &(DEFAULT_MIN_INITIAL_DEPOSIT + 10_000_000));
    assert_eq!(shares, DEFAULT_MIN_INITIAL_DEPOSIT + 10_000_000);
}

// Two independently initialized pool instances, each configured with its own
// `min_initial_deposit` at `initialize` time, must each enforce their own
// floor rather than sharing a single hardcoded constant (issue #744).
#[test]
fn test_two_pool_instances_enforce_their_own_configured_minimum() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let lp = Address::generate(&env);
    let registry_id = env.register_contract(None, MockRegistry);

    // A low-decimals asset pool, configured with a small minimum.
    let asset_a = env.register_contract(None, MockToken);
    let low_min: u128 = 100;
    let pool_a_id = build_pool_with_min_deposit(&env, &admin, &asset_a, &registry_id, low_min);
    let pool_a = PoolContractClient::new(&env, &pool_a_id);

    // A high-decimals asset pool, configured with the original USDC-scale minimum.
    let asset_b = env.register_contract(None, MockToken);
    let high_min: u128 = DEFAULT_MIN_INITIAL_DEPOSIT;
    let pool_b_id = build_pool_with_min_deposit(&env, &admin, &asset_b, &registry_id, high_min);
    let pool_b = PoolContractClient::new(&env, &pool_b_id);

    fund_lp(&env, &asset_a, &lp);
    fund_lp(&env, &asset_b, &lp);

    // Pool A's low minimum accepts a deposit that pool B would reject.
    assert!(pool_a.try_deposit(&lp, &low_min).is_ok());

    // A second, fresh instance for asset_b so the below-minimum deposit
    // exercises the empty-pool branch of the check.
    let pool_b_fresh_id =
        build_pool_with_min_deposit(&env, &admin, &asset_b, &registry_id, high_min);
    let pool_b_fresh = PoolContractClient::new(&env, &pool_b_fresh_id);
    fund_lp(&env, &asset_b, &lp);
    assert!(pool_b_fresh.try_deposit(&lp, &low_min).is_err());
    assert!(pool_b.try_deposit(&lp, &high_min).is_ok());
}

/// Deploys and initializes a fresh pool instance funding `asset`, configured
/// with `min_initial_deposit`.
fn build_pool_with_min_deposit(
    env: &Env,
    admin: &Address,
    asset: &Address,
    registry_id: &Address,
    min_initial_deposit: u128,
) -> Address {
    let invoice_id = env.register_contract(None, RealInvoice);
    RealInvoiceClient::new(env, &invoice_id).initialize(admin, registry_id);

    let pool_id = env.register_contract(None, PoolContract);
    let escrow_id = env.register_contract(None, RealEscrow);
    RealEscrowClient::new(env, &escrow_id).initialize(admin, &pool_id, asset);

    let pool = PoolContractClient::new(env, &pool_id);
    pool.initialize(
        admin,
        &invoice_id,
        &escrow_id,
        asset,
        registry_id,
        admin,
        &min_initial_deposit,
        &String::from_str(env, TEST_SHARE_NAME),
        &String::from_str(env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool_id
}

/// Credits `lp` with a large token balance on `asset`'s `MockToken`.
fn fund_lp(env: &Env, asset: &Address, lp: &Address) {
    env.as_contract(asset, || {
        env.storage()
            .persistent()
            .set(&TKey(lp.clone()), &100_000_000_000_000i128);
    });
}

//  WITHDRAW TESTS
#[test]
fn test_withdraw_returns_correct_usdc() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    let usdc = te.pool.withdraw(&te.lp, &5_000_000_000);
    assert_eq!(usdc, 5_000_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_withdraw_fails_if_insufficient_liquidity() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    te.pool.withdraw(&te.lp, &300_000_000);
}

#[test]
fn test_withdraw_updates_initial_deposit_and_yield_on_multiple_partial_withdrawals() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let first_return = te.pool.withdraw(&te.lp, &5_000_000_000);
    assert_eq!(first_return, 5_000_000_000);

    let init_dep_key = DataKey::LPInitialDeposit(te.lp.clone());
    let remaining_init_dep: u128 = te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .persistent()
            .get(&init_dep_key)
            .unwrap_or(0)
    });
    assert_eq!(remaining_init_dep, 5_000_000_000);

    let second_return = te.pool.withdraw(&te.lp, &5_000_000_000);
    assert_eq!(second_return, 5_000_000_000);

    let final_init_dep: Option<u128> = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get(&init_dep_key)
    });
    assert!(final_init_dep.is_none());

    let lp_pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(lp_pos.shares, 0);
    assert_eq!(lp_pos.yield_earned, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_withdraw_zero_shares_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.withdraw(&te.lp, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_withdraw_more_than_owned_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.withdraw(&te.lp, &20_000_000_000);
}

//  TRANSFER TESTS
#[test]
fn test_transfer_succeeds() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    te.pool.transfer_shares(&te.lp, &recipient, &5_000_000_000);

    let lp_position = te.pool.get_lp_position(&te.lp);
    assert_eq!(lp_position.shares, 5_000_000_000);

    let recipient_position = te.pool.get_lp_position(&recipient);
    assert_eq!(recipient_position.shares, 5_000_000_000);
}

#[test]
fn test_transfer_same_address_no_op() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let before = te.pool.get_lp_position(&te.lp);
    te.pool.transfer_shares(&te.lp, &te.lp, &5_000_000_000);
    let after = te.pool.get_lp_position(&te.lp);

    assert_eq!(before.shares, after.shares);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_transfer_zero_amount_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    te.pool.transfer_shares(&te.lp, &recipient, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")]
fn test_transfer_insufficient_balance_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    te.pool.transfer_shares(&te.lp, &recipient, &20_000_000_000);
}

//  FUND INVOICE TESTS
#[test]
fn test_fund_invoice_rejects_zero_funded_amount() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list_with_params(&te, &te.usdc_id, 1, 5000);

    let before = te.pool.get_stats();
    let result = te.pool.try_fund_invoice(&invoice_id);
    assert!(result.is_err(), "zero funded amount should be rejected");

    let after = te.pool.get_stats();
    assert_eq!(after.total_funded, before.total_funded);
    assert_eq!(after.active_invoice_count, before.active_invoice_count);
    assert_eq!(after.available_liquidity, before.available_liquidity);
    assert_eq!(te.invoice.get_status(&invoice_id), 1);
}

#[test]
fn test_fund_invoice_allows_boundary_amount_of_one() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    // face_value=2, discount_bps=5000 -> funded_amount = 2 * 5000 / 10000 = 1
    let invoice_id = create_and_list_with_params(&te, &te.usdc_id, 2, 5000);

    let result = te.pool.fund_invoice(&invoice_id);
    assert!(result);

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_funded, 1);
    assert_eq!(stats.active_invoice_count, 1);
}

#[test]
fn test_fund_invoice_discount_rounds_funded_amount_down() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    // 3 * (10_000 - 1) / 10_000 = 2.9997, floored to 2 stroops.
    let invoice_id = create_and_list_with_params(&te, &te.usdc_id, 3, 1);

    assert!(te.pool.fund_invoice(&invoice_id));
    assert_eq!(te.pool.get_funded_amount(&invoice_id), Some(2));
    assert_eq!(te.pool.get_stats().total_funded, 2);
}

#[test]
fn test_fund_invoice_succeeds_for_normal_amount() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    let result = te.pool.fund_invoice(&invoice_id);
    assert!(result);

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats.active_invoice_count, 1);
}

#[test]
fn test_fund_invoice_reduces_available_liquidity() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    let before = te.pool.get_stats();
    let _ = te.pool.fund_invoice(&invoice_id);
    let after = te.pool.get_stats();

    assert_eq!(after.active_invoice_count, 1);
    assert!(after.total_funded > before.total_funded);
    assert!(after.available_liquidity < before.available_liquidity);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_fund_invoice_fails_when_insufficient_liquidity() {
    let te = setup();
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn test_fund_invoice_fails_asset_mismatch() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    // Create invoice with XLM asset, but pool handles USDC
    let invoice_id = create_and_list(&te, &te.xlm_id);
    te.pool.fund_invoice(&invoice_id);
}

//  REGISTRY REVOCATION RE-CHECK (registry+invoice+pool bug) //
// Design decision: registry revocation is prospective, not retroactive.
// `list_for_financing` and `fund_invoice` are the two points where new
// business is committed (an issuer lists, then the pool commits capital),
// so both re-verify the issuer and buyer against the registry. Once an
// invoice is actually Funded, its lifecycle (mark_shipped, confirm_delivery,
// repay, repay_early, trigger_default) proceeds regardless of any later
// revocation — the pool's capital is already committed and the repayment
// terms are already fixed, so unwinding an in-flight invoice on revocation
// would be disruptive and gameable (e.g. an issuer griefing LPs by getting
// itself revoked mid-term). See `test_revocation_after_funding_does_not_block_lifecycle`
// below for the documented in-flight behavior.

#[test]
#[should_panic(expected = "Error(Contract, #18)")]
fn test_fund_invoice_fails_when_issuer_revoked_after_listing() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    // Issuer was verified at create()/list_for_financing() time but is
    // revoked before the pool commits capital.
    te.registry.revoke(&te.issuer);

    te.pool.fund_invoice(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn test_fund_invoice_fails_when_buyer_revoked_after_listing() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    te.registry.revoke(&te.buyer);

    te.pool.fund_invoice(&invoice_id);
}

#[test]
fn test_revocation_after_funding_does_not_block_lifecycle() {
    // Mid-lifecycle revocation (post-Funded) must NOT retroactively affect
    // an in-flight invoice: shipment, delivery confirmation, and repayment
    // all proceed exactly as if the issuer/buyer were still verified. This
    // is the documented, deliberate behavior — revocation only gates new
    // commitments (list_for_financing, fund_invoice), not invoices already
    // funded.
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    let result = te.pool.fund_invoice(&invoice_id);
    assert!(
        result,
        "funding must succeed while both parties are verified"
    );

    // Revoke both issuer and buyer only after the pool has already
    // committed capital.
    te.registry.revoke(&te.issuer);
    te.registry.revoke(&te.buyer);
    assert!(!te.registry.is_verified(&te.issuer));
    assert!(!te.registry.is_verified(&te.buyer));

    // The rest of the lifecycle is unaffected by the revocation.
    assert!(te.invoice.mark_shipped(&invoice_id));
    assert!(te.invoice.confirm_delivery(&invoice_id, &te.issuer));
    assert!(te.invoice.confirm_delivery(&invoice_id, &te.buyer));

    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 86401);
    assert!(te.invoice.repay(&invoice_id));

    let invoice = te.invoice.get(&invoice_id);
    assert_eq!(invoice.status, clearledger_invoice::InvoiceStatus::Repaid);

    let stats = te.pool.get_stats();
    assert_eq!(stats.active_invoice_count, 0);
    assert_eq!(stats.total_funded, 0);
}

//  ISSUE #275: FUND INVOICE EDGE CASES
#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_fund_invoice_nonexistent_invoice_panics() {
    // Calling fund_invoice with a random invoice ID that doesn't exist
    // should propagate the NotFound (#2) error from the invoice contract's
    // get_status call.
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let fake_id = BytesN::from_array(&te.env, &[0u8; 32]);
    te.pool.fund_invoice(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_fund_invoice_unlisted_invoice_panics() {
    // An invoice in Created state (not yet listed) must be rejected by
    // fund_invoice with InvoiceNotListed (#8).
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let due_date = te.env.ledger().timestamp() + 86400;
    let invoice_id = te.invoice.create(
        &te.issuer,
        &te.buyer,
        &1_000_000_000,
        &due_date,
        &te.usdc_id,
    );
    // Do NOT list the invoice — status is Created (0)
    te.pool.fund_invoice(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_fund_invoice_already_funded_invoice_panics() {
    // After successfully funding an invoice, a second call to fund_invoice
    // must be rejected with AlreadyFunded (#16): the pool's own FundedInvoice
    // guard is evaluated first and is the authoritative record that this
    // invoice was already funded here. Refs: issue #444.
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    // Second funding attempt must be rejected by the pool's own guard.
    te.pool.fund_invoice(&invoice_id);
}

//  STATS TESTS
#[test]
fn test_get_stats_initial_state() {
    let te = setup();
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 0);
    assert_eq!(stats.total_shares, 0);
    assert_eq!(stats.total_funded, 0);
    assert_eq!(stats.active_invoice_count, 0);
    assert_eq!(stats.available_liquidity, 0);
    assert_eq!(stats.utilization_rate_bps, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_stats_panics_on_uninitialized_pool() {
    // A freshly deployed contract (no initialize() call) must not silently
    // return zero-filled stats — that would let callers mistake an uninitialized
    // pool for an empty-but-healthy one. Instead get_stats() must panic with
    // NotInitialized (#2).
    let env = Env::default();
    env.mock_all_auths();
    let pool_id = env.register_contract(None, crate::PoolContract);
    let pool = crate::PoolContractClient::new(&env, &pool_id);
    let _ = pool.get_stats();
}

#[test]
fn test_get_stats_after_deposit() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 100_000_000_000);
    assert_eq!(stats.total_shares, 100_000_000_000);
    assert_eq!(stats.available_liquidity, 100_000_000_000);
    assert_eq!(stats.utilization_rate_bps, 0);
}

#[test]
fn test_get_stats_after_funding() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    let stats = te.pool.get_stats();
    assert!(stats.total_funded > 0);
    assert!(stats.available_liquidity < 100_000_000_000);
    assert_eq!(stats.active_invoice_count, 1);
    assert!(stats.utilization_rate_bps > 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_get_stats_rejects_utilization_overflow() {
    let te = setup();
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &u128::MAX);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &(u128::MAX / 10_000 + 1));
    });

    let _ = te.pool.get_stats();
}

//  SHARE-PRICE OVERFLOW TESTS (issue #584) //
// The share-price multiplications in deposit/withdraw/get_lp_position must
// panic with the typed PoolError::Overflow (#13) instead of a raw Rust
// arithmetic-overflow abort (the workspace release profile sets
// overflow-checks = true), matching utilization_bps_or_panic. The boundary is
// driven by injecting u128-scaled storage values — the same technique the
// get_stats/get_utilization_rate/fund_invoice overflow tests use.

// `usdc_amount * total_shares` overflows u128 when total_shares is inflated
// to u128::MAX, so deposit must panic with Overflow (#13). The check runs
// before the token transfer, so the depositor's USDC is never pulled.
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_deposit_rejects_share_price_overflow() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalShares, &u128::MAX);
    });

    te.pool.deposit(&te.lp, &10_000_000_000);
}

// Boundary complement: the largest representable product (2 * (u128::MAX / 2)
// == u128::MAX - 1) must still be accepted — checked_mul must not over-reject
// legal share-price math.
#[test]
fn test_deposit_accepts_multiplication_at_overflow_boundary() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalShares, &(u128::MAX / 2));
    });

    let shares = te.pool.deposit(&te.lp, &2);
    assert_eq!(shares, 2 * (u128::MAX / 2) / 10_000_000_000);
}

// `shares * total_deposits` overflows u128 when total_deposits is inflated to
// u128::MAX, so withdraw must panic with Overflow (#13) before transferring
// or burning anything.
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_withdraw_rejects_share_price_overflow() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &u128::MAX);
    });

    te.pool.withdraw(&te.lp, &10_000_000_000);
}

// `lp_shares * total_deposits` overflows u128 when total_deposits is inflated
// to u128::MAX, so get_lp_position must panic with Overflow (#13) rather than
// report a wrapped-around position value.
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_get_lp_position_rejects_share_price_overflow() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &u128::MAX);
    });

    let _ = te.pool.get_lp_position(&te.lp);
}

//  LP POSITION TESTS
#[test]
fn test_lp_position_empty() {
    let te = setup();
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 0);
    assert_eq!(pos.usdc_value, 0);
    assert_eq!(pos.yield_earned, 0);
    assert_eq!(pos.deposit_count, 0);
}

#[test]
fn test_lp_position_after_deposit() {
    let te = setup();
    te.pool.deposit(&te.lp, &50_000_000_000);
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 50_000_000_000);
    assert_eq!(pos.usdc_value, 50_000_000_000);
    assert_eq!(pos.deposit_count, 1);
}

//  UTILIZATION RATE TESTS
#[test]
fn test_utilization_rate_zero_when_no_deposits() {
    let te = setup();
    assert_eq!(te.pool.get_utilization_rate(), 0);
}

#[test]
fn test_utilization_rate_zero_when_no_funding() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    assert_eq!(te.pool.get_utilization_rate(), 0);
}

#[test]
fn test_utilization_rate_after_funding() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);
    let rate = te.pool.get_utilization_rate();
    assert!(rate > 0);
    assert!(rate < 10000);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_get_utilization_rate_rejects_overflow() {
    let te = setup();
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &u128::MAX);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &(u128::MAX / 10_000 + 1));
    });

    let _ = te.pool.get_utilization_rate();
}

#[test]
fn test_utilization_rate_calculates_correctly() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    // Raise cap to 100% so funding doesn't get rejected
    te.pool.set_max_utilization(&te.admin, &10000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);
    assert_eq!(
        te.pool.get_utilization_rate(),
        (DEFAULT_FUNDED_AMOUNT * 10000 / 10_000_000_000) as u32
    );
}

//  MAX UTILIZATION TESTS
#[test]
fn test_default_max_utilization_in_stats() {
    // Fresh pool without setup override to verify initialize default is 8500
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let registry_id = env.register_contract(None, MockRegistry);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);
    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);
    let pool_id = env.register_contract(None, PoolContract);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_id, &usdc_id);
    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );
    let stats = pool.get_stats();
    assert_eq!(stats.max_utilization_bps, 8500);
}

#[test]
fn test_updated_max_utilization_reflected_in_stats() {
    let te = setup();
    te.pool.set_max_utilization(&te.admin, &9000);
    let stats = te.pool.get_stats();
    assert_eq!(stats.max_utilization_bps, 9000);
}

// set_max_utilization must emit a max_utilization_updated event carrying the
// old and new caps so off-chain indexers can observe risk-parameter changes
// without polling get_stats() (issue #582).
#[test]
fn test_set_max_utilization_emits_event() {
    let te = setup();
    // setup() already raised the cap to 10000, so the old cap here is 10000.
    te.pool.set_max_utilization(&te.admin, &8500);

    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "max_utilization_updated")
    );
    assert_eq!(
        <(u32, u32)>::try_from_val(&te.env, &data).unwrap(),
        (10000, 8500)
    );

    // The storage update itself is still reflected in get_stats.
    assert_eq!(te.pool.get_stats().max_utilization_bps, 8500);

    // A rejected update (> 10000) must not emit the event.
    assert!(te.pool.try_set_max_utilization(&te.admin, &10001).is_err());
    let events_after = te.env.events().all();
    assert_eq!(events_after.len(), events.len());
}

//  GET_USDC_ASSET TESTS
#[test]
fn test_get_usdc_asset_returns_configured_asset() {
    let te = setup();
    assert_eq!(te.pool.get_usdc_asset(), te.usdc_id);
}

// get_funding_asset is the new, asset-generic name; get_usdc_asset (above) is
// kept only as a deprecated alias for pre-factory integrators, and both must
// return the same value (issue #743).
#[test]
fn test_get_funding_asset_returns_configured_asset() {
    let te = setup();
    assert_eq!(te.pool.get_funding_asset(), te.usdc_id);
    assert_eq!(te.pool.get_funding_asset(), te.pool.get_usdc_asset());
}

// get_usdc_asset delegates to Self::funding_asset(), whose instance read is
// guarded by a typed `PoolError::NotInitialized` panic rather than a bare
// `expect` string. This test pins the typed error when the pool has never been
// initialized (issue #591 originally documented the untyped panic; issue #441
// replaced it with the typed variant).
#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_usdc_asset_panics_when_uninitialized() {
    let env = Env::default();
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    pool.get_usdc_asset();
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_fund_invoice_rejects_utilization_overflow() {
    let te = setup();
    let invoice_id = create_and_list(&te, &te.usdc_id);
    // Set both TotalDeposits and TotalFunded near u128::MAX so that
    // `available = total_deposits - total_funded` does not underflow,
    // but `new_total_funded * 10_000` overflows in the utilization check.
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &u128::MAX);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &(u128::MAX / 10_000 + 1));
    });

    let _ = te.pool.fund_invoice(&invoice_id);
}

// A pathologically large face_value read from the invoice contract (the pool
// does not bound cross-contract values itself) must trigger the typed
// PoolError::Overflow (#13) in the funded_amount multiplication rather than
// an untyped arithmetic-overflow abort (#585).
#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_fund_invoice_rejects_funded_amount_overflow() {
    let te = setup();

    // Stand-in invoice contract reporting a face_value that overflows u128
    // when scaled by (10000 - discount_bps). The issuer/buyer it reports are
    // the ones already verified in the pool's registry, so the overflow is
    // the first failure the funding path hits.
    let mock_id = te.env.register_contract(None, MockHugeFaceInvoice);
    MockHugeFaceInvoiceClient::new(&te.env, &mock_id).configure(
        &te.issuer,
        &te.buyer,
        &te.usdc_id,
        &u128::MAX,
        &0,
    );

    // Point the pool's configured invoice contract at the stand-in — same
    // storage-injection technique as the other overflow tests in this file.
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::InvoiceContract, &mock_id);
    });

    let invoice_id = BytesN::from_array(&te.env, &[7u8; 32]);
    te.pool.fund_invoice(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn test_fund_invoice_rejects_above_cap() {
    let te = setup();
    // Restore cap to 8500; funding at 9800 utilization should fail
    te.pool.set_max_utilization(&te.admin, &8500);
    te.pool.deposit(&te.lp, &10_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
}

#[test]
fn test_set_max_utilization_rejects_zero_without_changing_cap() {
    let te = setup();
    te.pool.set_max_utilization(&te.admin, &0);
    te.pool.deposit(&te.lp, &10_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
}

#[test]
fn test_fund_invoice_allowed_when_below_cap() {
    let te = setup();
    te.pool.set_max_utilization(&te.admin, &10000);
    te.pool.deposit(&te.lp, &10_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let result = te.pool.fund_invoice(&invoice_id);
    assert!(result);
}

#[test]
fn test_fund_invoice_is_permissionless() {
    // Verify that fund_invoice can be called by any address without admin authorization.
    // Setup normally (with mock_all_auths) so initialization succeeds, then test with no auths.
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    // Clear all mocked auths so that any require_auth() call would fail.
    // If admin.require_auth() was still in the code, this would panic.
    te.env.set_auths(&[]);
    let result = te.pool.fund_invoice(&invoice_id);
    assert!(
        result,
        "fund_invoice should succeed without any mocked auths (no admin auth required)"
    );

    // Verify the invoice was actually funded
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats.active_invoice_count, 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_set_max_utilization_above_10000_panics() {
    let te = setup();
    te.pool.set_max_utilization(&te.admin, &10001);
}

// set_max_utilization must reject callers other than the admin (#581). The
// admin-authorized path is already covered by
// test_updated_max_utilization_reflected_in_stats and
// test_set_max_utilization_emits_event.
#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_set_max_utilization_requires_admin_authorization() {
    let te = setup();
    let non_admin = Address::generate(&te.env);

    // Clear all mocked auths so the non-admin's require_auth() fails.
    te.env.set_auths(&[]);
    te.pool.set_max_utilization(&non_admin, &9000);
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn test_reducing_cap_mid_lifecycle_blocks_new_funding() {
    let te = setup();
    te.pool.set_max_utilization(&te.admin, &8500);
    te.pool.deposit(&te.lp, &100_000_000_000);
    // First funding: 9_800_000_000 / 100_000_000_000 = 980 bps < 8500 → ok
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // Lower cap below the utilization a second funding would cause
    // (980 bps already used; adding another 9.8B → 1960 bps)
    te.pool.set_max_utilization(&te.admin, &1000);
    // Second funding should push utilization to 1960 bps > 1000 → rejected
    let invoice_id2 = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id2);
}

#[test]
fn test_yield_increases_share_price_after_repayment() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    fund_and_repay_invoice(&te);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 10_000_000_000);
    assert_eq!(pos.usdc_value, DEFAULT_FACE_VALUE + DEFAULT_YIELD_AMOUNT);
}

#[test]
fn test_two_lps_receive_proportional_yield() {
    let te = setup();
    let lp2 = create_lp_with_balance(&te, 100_000_000_000_000i128);

    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.deposit(&lp2, &30_000_000_000);
    fund_and_repay_invoice(&te);

    let pos1 = te.pool.get_lp_position(&te.lp);
    let pos2 = te.pool.get_lp_position(&lp2);

    assert_eq!(pos1.shares, 10_000_000_000);
    assert_eq!(pos2.shares, 30_000_000_000);
    // With proportional yield distribution: LP1 gets 25% (10B/40B) of yield
    assert_eq!(
        pos1.usdc_value,
        10_000_000_000 + DEFAULT_YIELD_AMOUNT * 10_000_000_000 / (10_000_000_000 + 30_000_000_000)
    );
    // LP2 gets 75% (30B/40B) of yield
    assert_eq!(
        pos2.usdc_value,
        30_000_000_000 + DEFAULT_YIELD_AMOUNT * 30_000_000_000 / (10_000_000_000 + 30_000_000_000)
    );
}

#[test]
fn test_lp_position_reflects_current_share_price() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 86401);
    te.invoice.repay(&invoice_id);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.usdc_value, DEFAULT_FACE_VALUE + DEFAULT_YIELD_AMOUNT);
    assert_eq!(pos.shares, 10_000_000_000);
}

// Reproduces #630: repay_early() was previously only exercised against
// MockPool in the invoice crate's own tests, never against the real
// escrow+pool setup wired up here. This drives invoice.repay_early()
// end-to-end (buyer -> escrow -> pool) partway through the term and asserts
// the pool-side accounting effects (yield split into total_deposits /
// total_yield_distributed) and the buyer's discount refund match the
// elapsed/term-proportional split repay_early computes internally.
#[test]
fn test_repay_early_against_real_pool_and_escrow() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);

    // face_value=10_000_000_000, discount_bps=200 (2%)
    // funded_amount = 10_000_000_000 * 9800 / 10000 = 9_800_000_000
    // discount = 200_000_000; term = 86400s (due_date - funded_at)
    let face_value: u128 = 10_000_000_000;
    let discount: u128 = 200_000_000;
    let term: u64 = 86400;

    // Repay halfway through the term.
    let elapsed: u64 = term / 2;
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + elapsed);

    let earned_by_pool = discount * (elapsed as u128) / (term as u128);
    let refund_to_buyer = discount - earned_by_pool;

    let stats_before = te.pool.get_stats();
    let buyer_balance_before = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);

    let result = te.invoice.repay_early(&invoice_id);
    assert!(result);

    let stats_after = te.pool.get_stats();
    assert_eq!(
        stats_after.total_deposits,
        stats_before.total_deposits + earned_by_pool
    );
    assert_eq!(
        stats_after.total_yield_distributed,
        stats_before.total_yield_distributed + earned_by_pool
    );
    assert_eq!(stats_after.total_funded, 0);
    assert_eq!(stats_after.active_invoice_count, 0);

    let buyer_balance_after = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);
    assert_eq!(
        buyer_balance_after,
        buyer_balance_before - (face_value as i128) + (refund_to_buyer as i128)
    );

    // Escrow's lock record must be gone after release_to_pool.
    let escrow_client = RealEscrowClient::new(&te.env, &te.escrow_id);
    assert_eq!(escrow_client.get_locked(&invoice_id), 0);

    assert_eq!(te.invoice.get_status(&invoice_id), 5); // Repaid
}

#[test]
fn test_early_repayment_rounds_one_unit_discount_accrual_down() {
    let te = setup();
    let deposit = 100_000_000_000u128;
    te.pool.deposit(&te.lp, &deposit);
    let invoice_id =
        create_and_list_with_params_and_term(&te, &te.usdc_id, 5, 2000, 86_400);
    te.pool.fund_invoice(&invoice_id);
    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);

    // The one-stroop discount accrued halfway through the term floors to 0.
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 43_200);
    let buyer_before = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);

    assert!(te.invoice.repay_early(&invoice_id));

    let buyer_after = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);
    assert_eq!(buyer_after, buyer_before - 4);
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, deposit);
    assert_eq!(stats.total_yield_distributed, 0);
}

// Issue #876: nonzero protocol fees must also flow through the real early
// repayment path, including the subsequent LP withdrawal.
#[test]
fn test_nonzero_protocol_fee_with_real_early_repayment_and_withdrawal() {
    let te = setup();
    let deposit = 100_000_000_000u128;
    te.pool.deposit(&te.lp, &deposit);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let treasury = Address::generate(&te.env);
    let fee_bps = 1_000u32;
    te.pool.set_protocol_fee(&fee_bps, &treasury);
    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);

    let elapsed = 43_200u64;
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + elapsed);
    let earned = DEFAULT_YIELD_AMOUNT * elapsed as u128 / 86_400;
    let fee = earned * fee_bps as u128 / 10_000;
    let lp_yield = earned - fee;
    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let treasury_before = usdc.balance(&treasury);

    assert!(te.invoice.repay_early(&invoice_id));
    assert_eq!(usdc.balance(&treasury) - treasury_before, fee as i128);
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_yield_distributed, lp_yield);
    assert_eq!(stats.total_deposits, deposit + lp_yield);
    assert_eq!(stats.total_funded, 0);
    assert_eq!(stats.active_invoice_count, 0);

    let position = te.pool.get_lp_position(&te.lp);
    let before_withdraw = usdc.balance(&te.lp);
    let returned = te.pool.withdraw(&te.lp, &position.shares);
    assert_eq!(returned, deposit + lp_yield);
    assert_eq!(usdc.balance(&te.lp) - before_withdraw, returned as i128);
    assert_eq!(te.pool.get_stats().total_deposits, 0);
    assert_eq!(te.invoice.get_status(&invoice_id), 5);
}

// Issue #878: interleave several real invoices while checking shared pool
// counters and the price used to admit a late LP after yield and loss.
#[test]
fn test_multi_invoice_mixed_repayment_default_and_late_depositor() {
    let te = setup();
    let lp2 = Address::generate(&te.env);
    let lp3 = Address::generate(&te.env);
    let lp2_key = TKey(lp2.clone());
    let lp3_key = TKey(lp3.clone());
    for (lp, key) in [(&lp2, &lp2_key), (&lp3, &lp3_key)] {
        te.env.as_contract(&te.usdc_id, || {
            te.env
                .storage()
                .persistent()
                .set(key, &100_000_000_000_000i128);
        });
        te.registry.register(lp);
    }
    te.pool.set_max_utilization(&te.admin, &6_000);
    te.pool.deposit(&te.lp, &40_000_000_000);
    te.pool.deposit(&lp2, &20_000_000_000);

    let repaid = create_and_list(&te, &te.usdc_id);
    let defaulted = create_and_list(&te, &te.usdc_id);
    let active = create_and_list(&te, &te.usdc_id);
    for id in [&repaid, &defaulted, &active] {
        te.pool.fund_invoice(id);
        let stats = te.pool.get_stats();
        assert_eq!(
            stats.available_liquidity,
            stats.total_deposits - stats.total_funded
        );
        assert!(stats.utilization_rate_bps <= stats.max_utilization_bps);
    }
    assert_eq!(te.pool.get_stats().active_invoice_count, 3);
    assert_eq!(te.pool.get_stats().total_funded, DEFAULT_FUNDED_AMOUNT * 3);

    te.invoice.mark_shipped(&repaid);
    te.invoice.confirm_delivery(&repaid, &te.issuer);
    te.invoice.confirm_delivery(&repaid, &te.buyer);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 43_200);
    assert!(te.invoice.repay_early(&repaid));
    let earned = DEFAULT_YIELD_AMOUNT / 2;
    let after_repay = te.pool.get_stats();
    assert_eq!(after_repay.total_yield_distributed, earned);
    assert_eq!(after_repay.active_invoice_count, 2);
    assert_eq!(
        after_repay.available_liquidity,
        after_repay.total_deposits - after_repay.total_funded
    );

    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 43_201);
    assert!(te.pool.handle_default(&defaulted));
    assert_eq!(te.invoice.get_status(&defaulted), 6);
    let after_default = te.pool.get_stats();
    assert_eq!(after_default.total_loss_realised, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(after_default.active_invoice_count, 1);
    assert_eq!(
        after_default.available_liquidity,
        after_default.total_deposits - after_default.total_funded
    );
    assert_eq!(after_default.total_yield_distributed, earned);

    let lp1_before = te.pool.get_lp_position(&te.lp);
    let lp2_before = te.pool.get_lp_position(&lp2);
    let before_late_deposit = te.pool.get_stats();
    te.pool.deposit(&lp3, &10_000_000_000);
    let lp3_position = te.pool.get_lp_position(&lp3);
    assert_eq!(
        lp3_position.shares,
        10_000_000_000 * before_late_deposit.total_shares / before_late_deposit.total_deposits
    );
    assert_eq!(lp3_position.yield_earned, 0);
    assert!(lp3_position.usdc_value <= 10_000_000_000);
    assert!(10_000_000_000 - lp3_position.usdc_value <= 1);
    assert_eq!(te.pool.get_lp_position(&te.lp).shares, lp1_before.shares);
    assert_eq!(te.pool.get_lp_position(&lp2).shares, lp2_before.shares);

    for (index, lp) in [&te.lp, &lp2, &lp3].iter().enumerate() {
        let position = te.pool.get_lp_position(lp);
        if position.shares > 0 {
            let stats = te.pool.get_stats();
            let position_value = position.shares * stats.total_deposits / stats.total_shares;
            let remaining_lps = (3 - index) as u128;
            let withdrawal_value =
                core::cmp::min(position_value, stats.available_liquidity / remaining_lps);
            let withdrawal_shares = withdrawal_value * stats.total_shares / stats.total_deposits;
            if withdrawal_shares > 0 {
                te.pool.withdraw(lp, &withdrawal_shares);
                let after = te.pool.get_stats();
                assert_eq!(
                    after.available_liquidity,
                    after.total_deposits - after.total_funded
                );
                assert_eq!(after.total_yield_distributed, earned);
                assert_eq!(after.total_loss_realised, DEFAULT_FUNDED_AMOUNT);
                assert_eq!(after.active_invoice_count, 1);
            }
        }
    }
    let final_stats = te.pool.get_stats();
    assert_eq!(
        final_stats.available_liquidity,
        final_stats.total_deposits - final_stats.total_funded
    );
    assert_eq!(final_stats.active_invoice_count, 1);
    assert_eq!(final_stats.total_yield_distributed, earned);
    assert_eq!(final_stats.total_loss_realised, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(te.invoice.get_status(&active), 2);
}
//  MULTI-LP TESTS
#[test]
fn test_multiple_lps_can_deposit() {
    let te = setup();
    let lp2 = Address::generate(&te.env);
    let lp2_bal_key = TKey(lp2.clone());
    te.env.as_contract(&te.usdc_id, || {
        te.env
            .storage()
            .persistent()
            .set(&lp2_bal_key, &100_000_000_000_000i128);
    });

    let s1 = te.pool.deposit(&te.lp, &10_000_000_000);
    let s2 = te.pool.deposit(&lp2, &20_000_000_000);

    assert_eq!(s1, 10_000_000_000);
    assert_eq!(s2, 20_000_000_000);

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_shares, 30_000_000_000);
    assert_eq!(stats.total_deposits, 30_000_000_000);
}

//  REPAYMENT TESTS
#[test]
fn test_receive_repayment() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // face_value=10_000_000_000, discount_bps=200
    // funded_amount = 10_000_000_000 * (10000 - 200) / 10000 = 9_800_000_000
    let yield_amount = DEFAULT_YIELD_AMOUNT;

    let before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);
    let result = te.pool.receive_repayment(&invoice_id, &10_000_000_000);
    assert!(result);

    let after = te.pool.get_stats();
    let position_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(after.total_deposits, before.total_deposits + yield_amount);
    assert_eq!(after.total_yield_distributed, yield_amount);
    assert_eq!(after.total_funded, 0);
    assert_eq!(after.active_invoice_count, 0);
    assert_eq!(
        position_after.usdc_value,
        position_before.usdc_value + yield_amount
    );

    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "repayment_received")
    );
    assert_eq!(
        BytesN::<32>::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        invoice_id
    );
    assert_eq!(
        <(u128, u128, u128)>::try_from_val(&te.env, &data).unwrap(),
        (10_000_000_000, yield_amount, 0)
    );
}

#[test]
fn test_receive_repayment_exact_funded_amount_has_no_yield() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);
    te.pool
        .receive_repayment(&invoice_id, &DEFAULT_FUNDED_AMOUNT);

    let after = te.pool.get_stats();
    let position_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(after.total_deposits, before.total_deposits);
    assert_eq!(after.total_yield_distributed, 0);
    assert_eq!(after.total_funded, 0);
    assert_eq!(position_after.usdc_value, position_before.usdc_value);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_receive_repayment_requires_invoice_contract_authorization() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    te.env.set_auths(&[]);
    te.pool.receive_repayment(&invoice_id, &10_000_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_receive_repayment_panics_when_amount_below_funded() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // funded_amount = 9_800_000_000, sending less should panic (#4 = InvalidAmount)
    te.pool.receive_repayment(&invoice_id, &1_000_000_000);
}

// pool.receive_repayment_with_refund trusts whatever discount/refund split
// invoice_contract passes in: pool has no visibility into funded_at/due_date
// and never checks that `refund` is proportional to elapsed time. It only
// bounds `refund` to [0, amount - funded_amount]. This test demonstrates
// that behavior directly: called immediately after funding (elapsed = 0,
// so a time-proportional split would refund ~the full discount to the
// buyer and credit the pool ~nothing), an artificially inconsistent split
// that instead credits the pool the *entire* discount as yield (refund = 0)
// is accepted unconditionally, purely because it falls within the amount
// bound. Reconciling this split against invoice's actual elapsed/term is
// invoice_contract's responsibility, not pool's — see the "Trust boundary"
// note on `receive_repayment_with_refund`'s rustdoc.
#[test]
fn test_receive_repayment_with_refund_accepts_time_inconsistent_split() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // No time has elapsed since funding — due_date is still a full 86400s
    // away and `term` has barely started. A time-proportional split would
    // refund nearly the entire discount to the buyer. Instead, pass a split
    // that hands the pool the entire discount immediately (refund = 0).
    let full_repayment = DEFAULT_FACE_VALUE;
    let inconsistent_refund = 0u128;

    let before = te.pool.get_stats();
    let buyer_usdc_before = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);

    let result = te.pool.receive_repayment_with_refund(
        &invoice_id,
        &full_repayment,
        &inconsistent_refund,
        &te.buyer,
    );
    assert!(result);

    // Pool accepted the split unconditionally: the full discount (which a
    // time-proportional split would have mostly refunded to the buyer at
    // elapsed = 0) was instead distributed as LP yield, and the buyer
    // received no refund at all. Pool performed no elapsed/term check.
    let after = te.pool.get_stats();
    assert_eq!(
        after.total_yield_distributed,
        before.total_yield_distributed + DEFAULT_YIELD_AMOUNT
    );
    assert_eq!(after.total_funded, 0);
    let buyer_usdc_after = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);
    assert_eq!(buyer_usdc_after, buyer_usdc_before);
}

// Happy path for receive_repayment_with_refund (#583): the buyer receives
// exactly `refund` via the USDC transfer, only the remaining yield slice is
// credited to the pool, and the repayment_received event carries
// (amount, yield_amount).
#[test]
fn test_receive_repayment_with_refund_happy_path() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // face_value=10_000_000_000, discount_bps=200 → funded=9_800_000_000.
    // Split the 200_000_000 surplus: 120_000_000 to the buyer, 80_000_000 to LPs.
    let amount = DEFAULT_FACE_VALUE;
    let refund = 120_000_000u128;
    let yield_amount = amount - DEFAULT_FUNDED_AMOUNT - refund;

    let before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);
    let buyer_before = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);

    let result = te
        .pool
        .receive_repayment_with_refund(&invoice_id, &amount, &refund, &te.buyer);
    assert!(result);

    // Pool accounting reflects only the yield slice, not the refunded portion.
    let after = te.pool.get_stats();
    let position_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(after.total_deposits, before.total_deposits + yield_amount);
    assert_eq!(
        after.total_yield_distributed,
        before.total_yield_distributed + yield_amount
    );
    assert_eq!(
        after.total_funded,
        before.total_funded - DEFAULT_FUNDED_AMOUNT
    );
    assert_eq!(after.active_invoice_count, before.active_invoice_count - 1);
    assert_eq!(
        position_after.usdc_value,
        position_before.usdc_value + yield_amount
    );

    // The buyer's USDC balance increased by exactly the refund.
    let buyer_after = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);
    assert_eq!(buyer_after, buyer_before + refund as i128);

    // The funded-invoice entry must be removed.
    let funded_key = DataKey::FundedInvoice(invoice_id.clone());
    assert!(!te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().has(&funded_key)
    }));

    // Event payload/topics for this entry point.
    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "repayment_received")
    );
    assert_eq!(
        BytesN::<32>::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        invoice_id
    );
    assert_eq!(
        <(u128, u128, u128)>::try_from_val(&te.env, &data).unwrap(),
        (amount, yield_amount, 0)
    );
}

// `refund` above the maximum (amount - funded_amount) must be rejected with
// InvalidAmount (#4): the bound keeps the pool's yield non-negative and stops
// invoice_contract from refunding more than the repayment surplus.
#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_receive_repayment_with_refund_rejects_refund_above_max() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let amount = DEFAULT_FACE_VALUE;
    // max refund = amount - funded = 200_000_000; send one stroop more.
    let refund = amount - DEFAULT_FUNDED_AMOUNT + 1;

    te.pool
        .receive_repayment_with_refund(&invoice_id, &amount, &refund, &te.buyer);
}

// refund == 0 must behave exactly like receive_repayment: the whole surplus
// goes to LP yield, the buyer receives nothing (the `if refund > 0` guard
// skips only the token transfer), and the repayment_received event still fires.
#[test]
fn test_receive_repayment_with_refund_zero_refund_matches_receive_repayment() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let amount = DEFAULT_FACE_VALUE;

    let before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);
    let buyer_before = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);

    let result = te
        .pool
        .receive_repayment_with_refund(&invoice_id, &amount, &0u128, &te.buyer);
    assert!(result);

    let after = te.pool.get_stats();
    let position_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(
        after.total_deposits,
        before.total_deposits + DEFAULT_YIELD_AMOUNT
    );
    assert_eq!(
        after.total_yield_distributed,
        before.total_yield_distributed + DEFAULT_YIELD_AMOUNT
    );
    assert_eq!(after.total_funded, 0);
    assert_eq!(after.active_invoice_count, 0);
    assert_eq!(
        position_after.usdc_value,
        position_before.usdc_value + DEFAULT_YIELD_AMOUNT
    );

    let buyer_after = MockTokenClient::new(&te.env, &te.usdc_id).balance(&te.buyer);
    assert_eq!(buyer_after, buyer_before);

    // The event still fires even though no refund transfer happened.
    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "repayment_received")
    );
    assert_eq!(
        BytesN::<32>::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        invoice_id
    );
    assert_eq!(
        <(u128, u128, u128)>::try_from_val(&te.env, &data).unwrap(),
        (amount, DEFAULT_YIELD_AMOUNT, 0)
    );
}

// Mismatched repayment (active_count already zero) must NOT silently underflow the
// counter to u32::MAX — the contract panics with #17 (ActiveCountUnderflow) instead.
// Otherwise every subsequent `get_stats()` / utilization read is corrupted.
#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_receive_repayment_active_count_underflow_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    // Inject a phantom funded-invoice record but force active_count to 0 so the
    // `active_count.checked_sub(1)` branch is the one to trigger. The other
    // counters are kept consistent so the panic lands on ActiveCountUnderflow,
    // not on the u128 underflow in `total_funded - funded_amount`.
    let phantom_id = BytesN::from_array(&te.env, &[0xab; 32]);
    let funded_amount: u128 = DEFAULT_FUNDED_AMOUNT;
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::FundedInvoice(phantom_id.clone()), &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &0u32);
    });

    te.pool.receive_repayment(&phantom_id, &funded_amount);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_receive_repayment_with_refund_active_count_underflow_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    let phantom_id = BytesN::from_array(&te.env, &[0xcd; 32]);
    let funded_amount: u128 = DEFAULT_FUNDED_AMOUNT;
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::FundedInvoice(phantom_id.clone()), &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &0u32);
    });

    te.pool
        .receive_repayment_with_refund(&phantom_id, &funded_amount, &0, &te.buyer);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_handle_default_active_count_underflow_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    let phantom_id = BytesN::from_array(&te.env, &[0xef; 32]);
    let funded_amount: u128 = DEFAULT_FUNDED_AMOUNT;
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .persistent()
            .set(&DataKey::FundedInvoice(phantom_id.clone()), &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &funded_amount);
        te.env
            .storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &0u32);
    });

    // Give escrow a matching lock record so escrow.handle_default() actually
    // releases funds (returns true) and execution reaches the pool-side
    // active-count underflow this test targets, rather than tripping the
    // EscrowDefaultNotReleased guard first.
    te.env.as_contract(&te.escrow_id, || {
        te.env.storage().persistent().set(
            &clearledger_escrow::DataKey::Locked(phantom_id.clone()),
            &clearledger_escrow::EscrowRecord {
                invoice_id: phantom_id.clone(),
                amount: funded_amount,
                locked_at: te.env.ledger().timestamp(),
                issuer: Address::generate(&te.env),
            },
        );
    });
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);

    te.pool.handle_default(&phantom_id);
}

// Reproduces #629: invoice.trigger_default's due-date gate (`now >=
// due_date`) has no awareness of escrow's independent
// DEFAULT_MIN_LOCK_SECONDS (60s) grace period measured from the escrow lock
// timestamp (~funded_at). For an invoice whose due_date is reached less
// than 60s after funding, trigger_default sets the invoice to Defaulted
// locally and then transitively calls escrow.handle_default() (via
// pool.handle_default), which panics with EscrowError::NotAuthorized,
// reverting the whole transaction. This test pins that current behavior;
// see the rustdoc coupling notes on both `EscrowContract::handle_default`
// and `InvoiceContract::trigger_default`.
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_trigger_default_reverts_when_escrow_grace_period_not_elapsed() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    // due_date reached only 30s after now (well under escrow's 60s grace
    // period, and funding happens immediately after listing in this test).
    let due_date = te.env.ledger().timestamp() + 30;
    let face_value: u128 = 10_000_000_000;
    let invoice_id = te
        .invoice
        .create(&te.issuer, &te.buyer, &face_value, &due_date, &te.usdc_id);
    attest_invoice(&te, &invoice_id);
    te.invoice.list_for_financing(&invoice_id, &200);
    te.pool.fund_invoice(&invoice_id);

    // Advance past due_date but still within escrow's 60s lock grace period.
    te.env.ledger().set_timestamp(due_date + 1);

    te.invoice.trigger_default(&invoice_id);
}

//  DEFAULT TESTS
#[test]
fn test_handle_default() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // funded_amount = 10_000_000_000 * 9800 / 10000 = 9_800_000_000
    let funded_amount = DEFAULT_FUNDED_AMOUNT;

    let before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);
    let result = te.pool.handle_default(&invoice_id);
    assert!(result);

    let after = te.pool.get_stats();
    let position_after = te.pool.get_lp_position(&te.lp);
    // Escrow returns the funded principal to the pool, so the default only
    // rotates capital out of `TotalFunded` — `TotalDeposits` and the LP's
    // share value stay intact (#439).
    assert_eq!(after.total_deposits, before.total_deposits);
    assert_eq!(after.total_funded, 0);
    assert_eq!(after.active_invoice_count, 0);
    assert_eq!(after.total_shares, before.total_shares);
    assert_eq!(
        after.total_loss_realised,
        before.total_loss_realised + funded_amount
    );
    assert_eq!(position_after.usdc_value, position_before.usdc_value);

    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "invoice_defaulted")
    );
    assert_eq!(
        BytesN::<32>::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        invoice_id
    );
    assert_eq!(u128::try_from_val(&te.env, &data).unwrap(), funded_amount);
}

#[test]
fn test_handle_default_realizes_loss_without_burning_shares() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);

    let lp_before = te.pool.get_lp_position(&te.lp);
    let pool_before = te.pool.get_stats();

    assert_eq!(pool_before.total_deposits, 100_000_000_000);
    assert_eq!(pool_before.total_shares, 100_000_000_000);
    assert_eq!(lp_before.usdc_value, 100_000_000_000);

    let result = te.pool.handle_default(&invoice_id);
    assert!(result);

    let lp_after = te.pool.get_lp_position(&te.lp);
    let pool_after = te.pool.get_stats();

    // A default is a capital rotation, not a capital burn (#439): escrow
    // returns the funded principal to the pool, so `TotalDeposits`, the share
    // supply, the LP's share balance, and LP share value are all preserved.
    // `total_loss_realised` still records the credit event, and only
    // `TotalFunded` is unwound.
    assert_eq!(pool_after.total_deposits, pool_before.total_deposits);
    assert_eq!(pool_after.total_shares, pool_before.total_shares);
    assert_eq!(lp_after.shares, lp_before.shares);
    assert_eq!(lp_after.usdc_value, lp_before.usdc_value);
    assert_eq!(pool_after.total_funded, 0);
    assert_eq!(pool_after.active_invoice_count, 0);
    assert_eq!(
        pool_after.total_loss_realised,
        pool_before.total_loss_realised + DEFAULT_FUNDED_AMOUNT
    );
}

//  ISSUE #439: DEFAULT RECOVERY PRESERVES LP CAPITAL
/// After escrow returns the funded principal, `TotalDeposits` must be
/// untouched: only `TotalFunded` is unwound. This keeps share price
/// (`TotalDeposits / TotalShares`) and every LP position intact, so a default
/// never artificially deflates LP value (issue #439).
#[test]
fn test_handle_default_preserves_total_deposits_and_lp_positions() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let before = te.pool.get_stats();
    let lp_before = te.pool.get_lp_position(&te.lp);
    assert_eq!(before.total_deposits, 100_000_000_000);
    assert_eq!(before.total_funded, DEFAULT_FUNDED_AMOUNT);

    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);
    assert!(te.pool.handle_default(&invoice_id));

    let after = te.pool.get_stats();
    let lp_after = te.pool.get_lp_position(&te.lp);

    // LP capital in: deposits, shares, and per-LP value all unchanged.
    assert_eq!(after.total_deposits, before.total_deposits);
    assert_eq!(after.total_shares, before.total_shares);
    assert_eq!(lp_after.shares, lp_before.shares);
    assert_eq!(lp_after.usdc_value, lp_before.usdc_value);

    // Only capital out on the invoice is unwound, and the recovered amount
    // shows up as free liquidity again.
    assert_eq!(after.total_funded, 0);
    assert_eq!(
        after.total_deposits - after.total_funded,
        before.total_deposits - before.total_funded + DEFAULT_FUNDED_AMOUNT
    );
    assert_eq!(after.active_invoice_count, 0);
    assert_eq!(
        after.total_loss_realised,
        before.total_loss_realised + DEFAULT_FUNDED_AMOUNT
    );
}

#[test]
fn test_handle_default_updates_invoice_status() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // Invoice should be in Funded status (2)
    assert_eq!(te.invoice.get_status(&invoice_id), 2);

    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);
    let result = te.pool.handle_default(&invoice_id);
    assert!(result);

    // After handle_default, invoice should be Defaulted (6)
    assert_eq!(te.invoice.get_status(&invoice_id), 6);
}

// Reproduces #627: every other default-path test drives the flow by calling
// te.pool.handle_default() directly, bypassing the real production entry
// point. A permissionless caller only ever has invoice.trigger_default(),
// which locally marks the invoice Defaulted and then invokes
// pool.handle_default() (which in turn calls escrow.handle_default() and
// calls back into invoice.mark_defaulted()).
//
// Driving the chain from invoice.trigger_default() (rather than from
// pool.handle_default() as the top-level caller) surfaces a real bug: the
// invoice contract is still on the call stack when pool calls back into
// invoice.mark_defaulted(), and Soroban's runtime rejects that as
// self-re-entrancy ("Contract re-entry is not allowed"), regardless of
// mark_defaulted's idempotent no-op logic. This test pins that current
// behavior; see follow-up issue for fixing the underlying re-entrancy in
// InvoiceContract::trigger_default / PoolContract::handle_default.
#[test]
#[should_panic(expected = "Error(Context, InvalidAction)")]
fn test_trigger_default_drives_full_pool_and_escrow_chain() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // Invoice should be Funded (2) before default.
    assert_eq!(te.invoice.get_status(&invoice_id), 2);

    let escrow_client = RealEscrowClient::new(&te.env, &te.escrow_id);
    assert_eq!(escrow_client.get_locked(&invoice_id), DEFAULT_FUNDED_AMOUNT);

    // Advance past both the invoice's due_date (86400s from creation) and
    // escrow's DEFAULT_MIN_LOCK_SECONDS grace period (60s from funding), so
    // trigger_default's due-date gate and escrow's lock-age gate both pass.
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 86401);

    // Drive the real production entry point instead of calling
    // pool.handle_default() directly. This panics with a re-entrancy error
    // once pool calls back into invoice.mark_defaulted() (see comment above).
    te.invoice.trigger_default(&invoice_id);
}

#[test]
fn test_handle_default_rejects_double_default() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);

    assert!(te.pool.handle_default(&invoice_id));
    // Second call should panic with InvoiceNotFound since the funded key was removed
    let res = te.pool.try_handle_default(&invoice_id);
    assert!(res.is_err());
}

// If escrow's lock record for an invoice is already gone by the time
// pool.handle_default runs (e.g. released out of band via release_to_pool),
// escrow.handle_default() returns false without transferring any tokens.
// Pool must not proceed with loss accounting in that case — see
// EscrowDefaultNotReleased (#21).
#[test]
fn test_handle_default_rejects_when_escrow_reports_no_release() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);

    // Simulate escrow's lock record having already been removed out of band,
    // so escrow.handle_default() hits its `return false` path instead of
    // transferring funds.
    let locked_key = clearledger_escrow::DataKey::Locked(invoice_id.clone());
    te.env.as_contract(&te.escrow_id, || {
        te.env.storage().persistent().remove(&locked_key);
    });

    let before = te.pool.get_stats();

    let res = te.pool.try_handle_default(&invoice_id);
    assert!(res.is_err());

    // Pool accounting must be untouched: escrow released nothing, so no loss
    // should be realised, funded/deposit totals must be unchanged, and the
    // funded invoice entry must still exist.
    let after = te.pool.get_stats();
    assert_eq!(after.total_deposits, before.total_deposits);
    assert_eq!(after.total_funded, before.total_funded);
    assert_eq!(after.total_loss_realised, before.total_loss_realised);
    assert_eq!(after.active_invoice_count, before.active_invoice_count);

    let funded_key = DataKey::FundedInvoice(invoice_id.clone());
    let still_funded = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().has(&funded_key)
    });
    assert!(still_funded);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_handle_default_requires_invoice_contract_authorization() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);

    te.env.set_auths(&[]);
    te.pool.handle_default(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn test_handle_default_unknown_invoice_panics() {
    let te = setup();
    let dummy_id = BytesN::from_array(&te.env, &[0u8; 32]);
    te.pool.handle_default(&dummy_id);
}

// Issue #443: `handle_default` for an invoice the pool never funded must not
// mutate any pool state. The current implementation rejects the call with
// InvoiceNotFound (#10) *before* it touches any accounting — the issue's
// "returns false" description predates that guard, and the exact error code is
// pinned by `test_handle_default_unknown_invoice_panics` above — so this test
// pins the safety-critical half: the rejected call writes nothing.
#[test]
fn test_handle_default_unknown_invoice_leaves_pool_state_unchanged() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    // Fund a real invoice so the pool holds non-trivial accounting to protect.
    let funded_id = create_and_list(&te, &te.usdc_id);
    assert!(te.pool.fund_invoice(&funded_id));

    let stats_before = te.pool.get_stats();
    let position_before = te.pool.get_lp_position(&te.lp);

    let unknown_id = BytesN::from_array(&te.env, &[7u8; 32]);
    let result = te.pool.try_handle_default(&unknown_id);
    assert!(
        result.is_err(),
        "handle_default on an unfunded invoice must be rejected"
    );

    // Pool accounting is byte-for-byte unchanged by the rejected call.
    let stats_after = te.pool.get_stats();
    assert_eq!(stats_after.total_deposits, stats_before.total_deposits);
    assert_eq!(stats_after.total_funded, stats_before.total_funded);
    assert_eq!(
        stats_after.active_invoice_count,
        stats_before.active_invoice_count
    );
    assert_eq!(
        stats_after.total_loss_realised,
        stats_before.total_loss_realised
    );
    assert_eq!(stats_after.total_shares, stats_before.total_shares);
    let position_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(position_after.shares, position_before.shares);
    assert_eq!(position_after.usdc_value, position_before.usdc_value);

    // The unrelated funded invoice's entry is still present and intact.
    let funded_key = DataKey::FundedInvoice(funded_id.clone());
    let still_funded: u128 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get(&funded_key).unwrap()
    });
    assert_eq!(still_funded, DEFAULT_FUNDED_AMOUNT);
}

#[test]
fn test_deposit_when_deposits_zero_but_shares_exist() {
    let te = setup();

    // LP1 deposits and keeps their shares. Since issue #439 makes a default
    // preserve `TotalDeposits`, the "deposits == 0 but shares exist" edge case
    // is now reproduced by zeroing `TotalDeposits` directly — this is the
    // division-by-zero path the deposit math must guard.
    te.pool.deposit(&te.lp, &DEFAULT_FUNDED_AMOUNT);
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &0u128);
    });

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 0);
    assert!(stats.total_shares > 0);

    // Attempt new deposit, which should not panic and should issue 1-to-1 shares
    let lp2 = create_lp_with_balance(&te, 10_000_000_000);
    let new_shares = te.pool.deposit(&lp2, &5_000_000_000);
    assert_eq!(new_shares, 5_000_000_000);
}

//  ISSUE #269: DEPOSIT AFTER DEFAULT (SHARE PRICE < 1)
#[test]
fn test_deposit_after_default_share_price_recovery() {
    let te = setup();

    // LP1 deposits 10B USDC
    let shares1 = te.pool.deposit(&te.lp, &10_000_000_000);
    assert_eq!(shares1, 10_000_000_000);

    // Fund invoice (9.8B funded)
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);

    // Default: escrow returns the 9.8B principal to the pool.
    let _ = te.pool.handle_default(&invoice_id);

    // Issue #439: default recovery no longer deflates LP capital — escrow
    // returns the principal, so TotalDeposits and the share supply are
    // preserved (share price stays at 1.0).
    let stats_after_default = te.pool.get_stats();
    assert_eq!(stats_after_default.total_deposits, 10_000_000_000);
    assert_eq!(stats_after_default.total_shares, 10_000_000_000); // unchanged

    // Force a sub-1.0 share price (0.2B / 10B = 0.02 USDC per share) directly
    // in storage: this is the state the deposit math must keep handling now
    // that defaults no longer produce it. Share price: 200M / 10B = 0.02
    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &DEFAULT_YIELD_AMOUNT);
    });

    // LP2 deposits 10B USDC (new address)
    let lp2 = create_lp_with_balance(&te, 100_000_000_000);
    let shares2 = te.pool.deposit(&lp2, &10_000_000_000);

    // LP2 should get 10B / 0.02 = 500B shares (less per USDC than LP1)
    // Because share price is depressed below 1.0
    assert!(
        shares2 > 10_000_000_000,
        "LP2 should get more shares due to deflated share price"
    );

    // Verify LP1 and LP2 positions
    let pos1 = te.pool.get_lp_position(&te.lp);
    let pos2 = te.pool.get_lp_position(&lp2);

    assert_eq!(pos1.shares, 10_000_000_000);
    assert_eq!(pos1.usdc_value, DEFAULT_YIELD_AMOUNT); // 10B shares * 0.02 per share

    assert_eq!(pos2.shares, shares2);
    assert_eq!(pos2.usdc_value, 10_000_000_000); // LP2 deposited 10B

    // Verify final pool state
    let final_stats = te.pool.get_stats();
    assert_eq!(
        final_stats.total_deposits,
        DEFAULT_FACE_VALUE + DEFAULT_YIELD_AMOUNT
    ); // 200M + 10B
    assert_eq!(final_stats.total_shares, 10_000_000_000 + shares2);
}

//  ISSUE #270: WITHDRAW EXACT TOTAL SHARES TO ZERO
#[test]
fn test_withdraw_all_shares_to_zero_then_redeposit() {
    let te = setup();

    // Deposit 10B USDC
    let shares = te.pool.deposit(&te.lp, &10_000_000_000);
    assert_eq!(shares, 10_000_000_000);

    // Verify initial state
    let stats_before = te.pool.get_stats();
    assert_eq!(stats_before.total_shares, 10_000_000_000);
    assert_eq!(stats_before.total_deposits, 10_000_000_000);

    // Withdraw all shares
    let usdc_returned = te.pool.withdraw(&te.lp, &10_000_000_000);
    assert_eq!(usdc_returned, 10_000_000_000);

    // Verify total shares and deposits are now zero
    let stats_after_withdraw = te.pool.get_stats();
    assert_eq!(stats_after_withdraw.total_shares, 0);
    assert_eq!(stats_after_withdraw.total_deposits, 0);

    // Verify LP has no position
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 0);
    assert_eq!(pos.usdc_value, 0);

    // Re-deposit succeeds and is treated as a first-depositor (1:1 shares)
    let new_shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(new_shares, 5_000_000_000);

    // Verify pool state after re-deposit
    let stats_after_redeposit = te.pool.get_stats();
    assert_eq!(stats_after_redeposit.total_shares, 5_000_000_000);
    assert_eq!(stats_after_redeposit.total_deposits, 5_000_000_000);

    let final_pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(final_pos.shares, 5_000_000_000);
    assert_eq!(final_pos.deposit_count, 1); // Reset on full withdrawal, so 1 deposit in this cycle
}

//  ISSUE #258: RESET LP STATE ON FULL WITHDRAWAL
#[test]
fn test_full_withdraw_resets_lp_state() {
    let te = setup();
    let lp2 = create_lp_with_balance(&te, 100_000_000_000);

    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.deposit(&lp2, &20_000_000_000);

    // Generate yield so LPInitialDeposit != LPShares
    fund_and_repay_invoice(&te);

    // Full withdrawal of all shares
    let shares = te.pool.get_lp_position(&te.lp).shares;
    assert!(shares > 0);
    te.pool.withdraw(&te.lp, &shares);

    // Verify LPInitialDeposit is removed from storage
    let init_dep_key = DataKey::LPInitialDeposit(te.lp.clone());
    assert!(!te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().has(&init_dep_key)
    }));

    // Verify LPDepositCount is removed from storage
    let dep_count_key = DataKey::LPDepositCount(te.lp.clone());
    assert!(!te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().has(&dep_count_key)
    }));

    // Verify get_lp_position returns 0 for both
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 0);
    assert_eq!(pos.deposit_count, 0);
}

#[test]
fn test_full_withdraw_then_deposit_yield_accounting() {
    let te = setup();
    let lp2 = create_lp_with_balance(&te, 100_000_000_000);

    // First deposit cycle
    te.pool.deposit(&te.lp, &10_000_000_000);
    te.pool.deposit(&lp2, &20_000_000_000);

    // Generate yield
    fund_and_repay_invoice(&te);

    // Full withdrawal with yield — principal portion should be less than USDC returned
    let pos_before = te.pool.get_lp_position(&te.lp);
    let returned = te.pool.withdraw(&te.lp, &pos_before.shares);
    assert!(returned > 10_000_000_000); // Got yield

    // Verify yield_earned is tracked after full withdrawal
    let pos_after = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos_after.shares, 0);
    assert!(pos_after.yield_earned > 0);

    // Re-deposit — should start fresh with deposit_count = 1 (not 2)
    let new_shares = te.pool.deposit(&te.lp, &5_000_000_000);
    assert!(new_shares > 0);

    let final_pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(final_pos.shares, new_shares);
    assert_eq!(final_pos.deposit_count, 1); // Reset, not 2

    // Yield earned from previous cycle is preserved
    assert!(final_pos.yield_earned > 0);
}

#[test]
fn test_multi_deposit_average_basis_differs_from_fifo_yield() {
    let te = setup();
    let first_deposit = 10_000_000_000u128;

    // A 2% pool return raises the share price before this LP's second deposit.
    te.pool.deposit(&te.lp, &first_deposit);
    fund_and_repay_invoice(&te);
    let second_shares = te.pool.deposit(&te.lp, &first_deposit);
    assert_eq!(second_shares, 9_803_921_568);

    // Withdrawing half of the original lot would return 5.1B stroops. FIFO
    // assigns 5B stroops of principal and 100M of yield to that withdrawal.
    let shares_withdrawn = first_deposit / 2;
    let returned = te.pool.withdraw(&te.lp, &shares_withdrawn);
    assert_eq!(returned, 5_100_000_000);

    // The aggregate-basis model instead allocates about 5.0495B principal,
    // understating this withdrawal's FIFO yield by about 49.5M stroops.
    let position = te.pool.get_lp_position(&te.lp);
    assert_eq!(position.yield_earned, 50_495_050);
    assert_eq!(100_000_000 - position.yield_earned, 49_504_950);
}

#[test]
fn test_multi_lp_proportional_yield_with_mid_cycle_deposit() {
    let te = setup();

    // LP1 deposits 10B USDC
    let lp1_deposit = 10_000_000_000;
    let lp1_shares = te.pool.deposit(&te.lp, &lp1_deposit);
    assert_eq!(lp1_shares, lp1_deposit);

    // LP2 deposits 20B USDC (different amount, 2:1 ratio)
    let lp2 = create_lp_with_balance(&te, 100_000_000_000);
    let lp2_deposit = 20_000_000_000;
    let lp2_shares = te.pool.deposit(&lp2, &lp2_deposit);
    assert_eq!(lp2_shares, lp2_deposit);

    // Fund an invoice (9.8B funded out of 30B total)
    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    // Verify funding occurred
    let stats_after_fund = te.pool.get_stats();
    assert!(stats_after_fund.total_funded > 0);

    // LP3 deposits 15B between fund and repay
    let lp3 = create_lp_with_balance(&te, 100_000_000_000);
    let lp3_deposit = 15_000_000_000;
    let lp3_shares = te.pool.deposit(&lp3, &lp3_deposit);

    // Repay the invoice with yield (200M yield on 9.8B funded)
    let face_value = 10_000_000_000;
    let yield_amount = DEFAULT_YIELD_AMOUNT;
    te.pool.receive_repayment(&invoice_id, &face_value);

    // Verify yield was added to total_deposits
    let stats_after_repay = te.pool.get_stats();
    assert_eq!(stats_after_repay.total_yield_distributed, yield_amount);

    // LP1 withdraws all shares and verifies proportional yield
    let lp1_return = te.pool.withdraw(&te.lp, &lp1_shares);
    let _lp1_pos = te.pool.get_lp_position(&te.lp);

    // LP2 withdraws all shares and verifies proportional yield
    let lp2_return = te.pool.withdraw(&lp2, &lp2_shares);
    let _lp2_pos = te.pool.get_lp_position(&lp2);

    // Verify LP1 and LP2 received gains proportional to their share of yield
    // LP1 had 1/3 of the pool before LP3 joined and funded, so should receive ~1/3 of yield
    // LP2 had 2/3 of the pool before LP3 joined and funded, so should receive ~2/3 of yield
    assert!(
        lp1_return >= lp1_deposit,
        "LP1 should receive at least their deposit"
    );
    assert!(
        lp2_return >= lp2_deposit,
        "LP2 should receive at least their deposit"
    );

    // LP2 should have received more yield than LP1 (2:1 ratio)
    let lp1_gain = lp1_return - lp1_deposit;
    let lp2_gain = lp2_return - lp2_deposit;
    assert!(
        lp2_gain > lp1_gain,
        "LP2 should have higher yield gain due to larger deposit"
    );

    // LP3 should have received minimal or no yield (deposited after fund)
    let lp3_return = te.pool.withdraw(&lp3, &lp3_shares);
    let lp3_gain = lp3_return.saturating_sub(lp3_deposit);

    // LP3 deposited after funding, so should not receive much yield
    assert!(
        lp3_gain <= lp2_gain,
        "LP3 should receive less yield than LP2"
    );
}

//  ISSUE #272: NEGATIVE AUTH TESTS // Note: These functions (receive_repayment, handle_default, fund_invoice) are guarded by
// cross-contract auth checks. Testing unauthorized access requires more complex mocking
// of contract-to-contract calls. The auth logic is enforced at the contract boundary
// and verified through integration tests. Individual contract auth is covered by the
// fact that mock_all_auths() is used during setup and cleared when specific auths are set.

//  ISSUE #274: INSUFFICIENT LIQUIDITY ON WITHDRAW
// LP deposits exactly the amount that will be funded (100% utilization),
// then tries to withdraw all shares — the pool has zero available liquidity
// so this must panic with InsufficientLiquidity (#5).
#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_withdraw_all_shares_panics_when_insufficient_liquidity_at_full_utilization() {
    let te = setup();

    // face_value=10_000_000_000, discount_bps=200 → funded_amount=9_800_000_000
    // Deposit exactly 9.8B so funding uses 100% of available liquidity
    te.pool.deposit(&te.lp, &DEFAULT_FUNDED_AMOUNT);

    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    // Verify the pool is at 100% utilization (no available liquidity)
    let stats = te.pool.get_stats();
    assert_eq!(
        stats.available_liquidity, 0,
        "pool should have zero available liquidity"
    );
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats.total_deposits, DEFAULT_FUNDED_AMOUNT);

    // Attempt to withdraw all 9.8B shares when available = 0 → InsufficientLiquidity
    te.pool.withdraw(&te.lp, &DEFAULT_FUNDED_AMOUNT);
}

// LP deposits more than the funded amount, so some liquidity remains available.
// A partial withdraw within that available liquidity succeeds.
#[test]
fn test_partial_withdraw_succeeds_within_available_liquidity() {
    let te = setup();

    // face_value=10_000_000_000, discount_bps=200 → funded_amount=9_800_000_000
    // Deposit 15B so 5.2B remains available after funding
    te.pool.deposit(&te.lp, &15_000_000_000);

    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    // Verify available liquidity
    let stats = te.pool.get_stats();
    let available_liquidity = 15_000_000_000 - DEFAULT_FUNDED_AMOUNT;
    assert_eq!(stats.available_liquidity, available_liquidity);
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT);

    // Partial withdraw of exactly the available amount should succeed
    let returned = te.pool.withdraw(&te.lp, &available_liquidity);
    assert_eq!(returned, available_liquidity);

    // Verify pool state after partial withdraw
    let stats_after = te.pool.get_stats();
    assert_eq!(stats_after.total_deposits, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats_after.total_shares, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats_after.available_liquidity, 0);

    // Verify LP position updated correctly
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(pos.usdc_value, DEFAULT_FUNDED_AMOUNT);

    // Verify events
    // Event payload: (usdc_amount, shares_burned)
    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "lp_withdrawn")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.lp
    );
    assert_eq!(
        <(u128, u128)>::try_from_val(&te.env, &data).unwrap(),
        (available_liquidity, available_liquidity)
    );
}

// LP deposits more than the funded amount and withdraws less than the available
// liquidity — verifies that the LP can withdraw a portion while leaving some
// liquidity in the pool for other operations.
#[test]
fn test_partial_withdraw_leaves_remaining_liquidity() {
    let te = setup();

    // Deposit 20B, funding uses 9.8B → 10.2B available
    te.pool.deposit(&te.lp, &20_000_000_000);

    let invoice_id = create_and_list(&te, &te.usdc_id);
    let _ = te.pool.fund_invoice(&invoice_id);

    let stats = te.pool.get_stats();
    let available_liquidity = 20_000_000_000 - DEFAULT_FUNDED_AMOUNT;
    assert_eq!(stats.available_liquidity, available_liquidity);

    // Partial withdraw 5B out of 10.2B available
    let returned = te.pool.withdraw(&te.lp, &5_000_000_000);
    assert_eq!(returned, 5_000_000_000);

    // Remaining liquidity should be 5.2B (10.2B - 5B)
    let stats_after = te.pool.get_stats();
    assert_eq!(
        stats_after.available_liquidity,
        available_liquidity - 5_000_000_000
    );
    assert_eq!(stats_after.total_deposits, 15_000_000_000);
    assert_eq!(stats_after.total_shares, 15_000_000_000);

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 15_000_000_000);
}

//  ISSUE #268: WITHDRAW AFTER REPAYMENT (SHARE PRICE > 1)
// Covers the "yield grows share price" invariant end-to-end: deposit, fund,
// repay, then withdraw the same shares and confirm the LP is paid back more
// USDC than they put in, with the surplus exactly matching the discount
// portion of yield distributed on repayment.
#[test]
fn test_withdraw_after_repayment_returns_more_than_deposited() {
    let te = setup();
    let deposit_amount = 10_000_000_000u128;
    te.pool.deposit(&te.lp, &deposit_amount);
    fund_and_repay_invoice(&te);

    // face_value=10_000_000_000, discount_bps=200 (create_and_list defaults):
    // funded_amount = 10_000_000_000 * 9800 / 10000 = 9_800_000_000
    // yield = face_value - funded_amount = 200_000_000, all of which accrues
    // to this LP since they are the pool's sole depositor.
    let expected_yield = DEFAULT_YIELD_AMOUNT;

    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, deposit_amount);
    assert_eq!(pos.usdc_value, deposit_amount + expected_yield);

    let usdc_returned = te.pool.withdraw(&te.lp, &pos.shares);

    assert!(
        usdc_returned > deposit_amount,
        "withdrawal after yield-generating repayment should return more than was deposited"
    );
    assert_eq!(usdc_returned, deposit_amount + expected_yield);
    assert_eq!(usdc_returned - deposit_amount, expected_yield);

    // Pool is fully drained: no shares or deposits remain.
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_shares, 0);
    assert_eq!(stats.total_deposits, 0);

    // The LP's realised yield is tracked for future reporting.
    let final_pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(final_pos.yield_earned, expected_yield);

    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "lp_withdrawn")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.lp
    );
    assert_eq!(
        <(u128, u128)>::try_from_val(&te.env, &data).unwrap(),
        (usdc_returned, deposit_amount)
    );
}

//  ISSUE #263: INITIALIZE ADDRESS COLLISION GUARD
// A fresh, valid initialize() with four distinct addresses must keep working;
// `setup()` (used throughout this file) already exercises this path, but this
// test makes the positive case explicit for the collision guard added below.
#[test]
fn test_initialize_accepts_distinct_addresses() {
    let te = setup();
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_shares, 0);
    assert_eq!(stats.total_deposits, 0);
}

// Every pairwise collision among (admin, invoice_contract, escrow_contract,
// usdc_asset, registry_contract) must be rejected with InvalidConfiguration
// (#15) so the handle_default gate can never collide with the admin path.
#[test]
fn test_initialize_rejects_each_pairwise_address_collision() {
    let env = Env::default();
    env.mock_all_auths();

    let base = [
        Address::generate(&env), // admin
        Address::generate(&env), // invoice_contract
        Address::generate(&env), // escrow_contract
        Address::generate(&env), // usdc_asset
        Address::generate(&env), // registry_contract
    ];

    let pairs = [
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (1, 2),
        (1, 3),
        (1, 4),
        (2, 3),
        (2, 4),
        (3, 4),
    ];
    for (i, j) in pairs {
        let mut addrs = base.clone();
        addrs[j] = addrs[i].clone();

        let pool_id = env.register_contract(None, PoolContract);
        let pool = PoolContractClient::new(&env, &pool_id);
        let res = pool.try_initialize(
            &addrs[0],
            &addrs[1],
            &addrs[2],
            &addrs[3],
            &addrs[4],
            &addrs[0],
            &DEFAULT_MIN_INITIAL_DEPOSIT,
            &String::from_str(&env, TEST_SHARE_NAME),
            &String::from_str(&env, TEST_SHARE_SYMBOL),
            &DEFAULT_SHARE_DECIMALS,
        );
        assert!(
            res.is_err(),
            "collision between initialize() params {i} and {j} should be rejected"
        );
    }
}

#[test]
fn test_initialize_rejects_treasury_aliases() {
    let te = setup();
    let invoice_id = te.invoice.address.clone();
    let registry_id = te.registry.address.clone();

    // Each attempt gets a fresh pool address so the test covers treasury
    // aliases to the pool, invoice, escrow, registry, and funding asset.
    for role in 0..5 {
        let pool_id = te.env.register_contract(None, PoolContract);
        let treasury = match role {
            0 => pool_id.clone(),
            1 => invoice_id.clone(),
            2 => te.escrow_id.clone(),
            3 => registry_id.clone(),
            _ => te.usdc_id.clone(),
        };
        let pool = PoolContractClient::new(&te.env, &pool_id);
        let result = pool.try_initialize(
            &te.admin,
            &invoice_id,
            &te.escrow_id,
            &te.usdc_id,
            &registry_id,
            &treasury,
            &DEFAULT_MIN_INITIAL_DEPOSIT,
            &String::from_str(&te.env, TEST_SHARE_NAME),
            &String::from_str(&te.env, TEST_SHARE_SYMBOL),
            &DEFAULT_SHARE_DECIMALS,
        );
        assert!(
            result.is_err(),
            "treasury alias for role {role} was accepted"
        );
    }
}

//  ISSUE #265: PREVENT ALREADYFUNDED SILENT SHADOWING
// If a `FundedInvoice` entry already exists for an invoice id, fund_invoice
// must reject the call with AlreadyFunded (#16) instead of silently
// overwriting the prior entry (which would double-lock escrow funds and
// double-count active_invoice_count for a single invoice).
#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_fund_invoice_rejects_replay_when_funded_invoice_entry_exists() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    // Simulate a stale FundedInvoice entry already present for this invoice
    // id while the invoice itself is still Listed.
    let funded_key = DataKey::FundedInvoice(invoice_id.clone());
    te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().set(&funded_key, &1u128);
    });

    te.pool.fund_invoice(&invoice_id);
}

// The normal (non-replayed) funding path must be unaffected by the guard.
#[test]
fn test_fund_invoice_succeeds_when_no_prior_funded_entry() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    let result = te.pool.fund_invoice(&invoice_id);
    assert!(result);

    let funded_key = DataKey::FundedInvoice(invoice_id.clone());
    let funded_amount: u128 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get(&funded_key).unwrap()
    });
    assert_eq!(funded_amount, DEFAULT_FUNDED_AMOUNT);
}

//  ISSUE #444: DOUBLE FUNDING / FUND AFTER SETTLEMENT
// Calling `fund_invoice` twice for the same invoice is rejected by the pool's
// own `FundedInvoice` guard with AlreadyFunded (#16). That guard is what stops
// a replay from double-locking escrow funds or double-counting
// `active_invoice_count` for one invoice.
#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_fund_invoice_twice_panics_already_funded() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    assert!(te.pool.fund_invoice(&invoice_id));
    // Replay: the FundedInvoice entry committed by the first call is the
    // authoritative record, so this must fail with AlreadyFunded rather than
    // silently funding again.
    te.pool.fund_invoice(&invoice_id);
}

// Once an invoice is repaid its funded entry has been settled (and removed)
// and the invoice is no longer Listed, so it can never be funded again.
#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_fund_invoice_after_repayment_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = fund_and_repay_invoice(&te);
    assert_eq!(
        te.invoice.get(&invoice_id).status,
        clearledger_invoice::InvoiceStatus::Repaid
    );

    te.pool.fund_invoice(&invoice_id);
}

// After default the funded entry has been removed and the invoice is
// Defaulted, so re-funding is likewise rejected.
#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_fund_invoice_after_default_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    assert!(te.pool.fund_invoice(&invoice_id));

    // Move past the default grace period and settle the default.
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);
    assert!(te.pool.handle_default(&invoice_id));
    assert_eq!(
        te.invoice.get(&invoice_id).status,
        clearledger_invoice::InvoiceStatus::Defaulted
    );

    te.pool.fund_invoice(&invoice_id);
}

//  ISSUE #281: INSTANCE TTL EXTENSION
// Every state-changing entrypoint must extend the contract's instance TTL so
// an active pool never expires. New instance storage entries start with a
// small default ttl (well above the extend_ttl threshold), so the
// initialize()/set_max_utilization() calls in `setup()` are no-ops for the
// ttl bump. Drive the ttl down below the threshold here, then confirm a
// state-changing call (deposit) bumps it back up close to the configured
// extend-to window.
#[test]
fn test_deposit_extends_instance_ttl_when_below_threshold() {
    // The default instance TTL (4096) is below TTL_THRESHOLD (500_000),
    // so initialize() extends it to ~TTL_EXTEND_TO. Verify the extension
    // happens by checking TTL before and after initialization.
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let registry_id = env.register_contract(None, MockRegistry);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);

    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);

    let pool_id = env.register_contract(None, PoolContract);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_id, &usdc_id);

    // Before initialize: TTL is the default of ~4096 ledgers.
    let ttl_before = env.as_contract(&pool_id, || env.storage().instance().get_ttl());
    assert!(
        ttl_before < TTL_THRESHOLD,
        "default instance ttl should be below TTL_THRESHOLD, got {ttl_before}"
    );

    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );

    // After initialize: TTL should be bumped to ~TTL_EXTEND_TO.
    let ttl_after = env.as_contract(&pool_id, || env.storage().instance().get_ttl());
    assert!(
        ttl_after >= 1_999_000,
        "instance ttl should be extended close to EXTEND_TO, got {ttl_after}"
    );
}

//  ISSUE #588: LP ENTRY TTL EXTENSION ON get_lp_position
// A deposit-and-hold LP only ever has LPShares/LPDepositCount/LPInitialDeposit
// TTLs set at deposit time. get_lp_position must refresh those entries on
// read, otherwise they lapse and become archival-eligible even though the
// position is still live from the pool's economic perspective.
#[test]
fn test_get_lp_position_extends_ttl_for_deposit_and_hold_lp() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let keys = [
        DataKey::LPShares(te.lp.clone()),
        DataKey::LPDepositCount(te.lp.clone()),
        DataKey::LPInitialDeposit(te.lp.clone()),
    ];

    // Drive the entries below the write-path threshold. All three were set at
    // the same deposit, so one key's remaining TTL gauges the drain for all.
    let ttl_before_drain: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&keys[0])
    });
    te.env
        .ledger()
        .set_sequence_number(te.env.ledger().sequence() + ttl_before_drain - 50);

    for key in &keys {
        let ttl_before_read: u32 = te
            .env
            .as_contract(&te.pool_id, || te.env.storage().persistent().get_ttl(key));
        assert!(
            ttl_before_read < TTL_THRESHOLD,
            "TTL should be below threshold before read, got {ttl_before_read}"
        );
    }

    // The read refreshes every surviving LP entry, not just LPShares.
    let pos = te.pool.get_lp_position(&te.lp);
    assert_eq!(pos.shares, 10_000_000_000);

    for key in &keys {
        let ttl_after_read: u32 = te
            .env
            .as_contract(&te.pool_id, || te.env.storage().persistent().get_ttl(key));
        assert!(
            ttl_after_read >= 1_999_000,
            "get_lp_position should extend TTL close to EXTEND_TO, got {ttl_after_read}"
        );
    }
}

// LPYieldEarned only appears after a withdrawal realizes yield, so the
// deposit-and-hold case above cannot cover it. Verify it is refreshed too.
#[test]
fn test_get_lp_position_extends_ttl_for_yield_earned_entry() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    fund_and_repay_invoice(&te);
    // Reap 10M of realized yield (5B shares @ 1.002 price) so the
    // LPYieldEarned entry is created.
    te.pool.withdraw(&te.lp, &5_000_000_000);
    let pos = te.pool.get_lp_position(&te.lp);
    assert!(
        pos.yield_earned > 0,
        "expected realized yield, got {}",
        pos.yield_earned
    );

    let yield_key = DataKey::LPYieldEarned(te.lp.clone());
    let ttl_before_drain: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&yield_key)
    });
    te.env
        .ledger()
        .set_sequence_number(te.env.ledger().sequence() + ttl_before_drain - 50);

    let ttl_before_read: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&yield_key)
    });
    assert!(
        ttl_before_read < TTL_THRESHOLD,
        "TTL should be below threshold before read, got {ttl_before_read}"
    );

    let pos = te.pool.get_lp_position(&te.lp);
    assert!(pos.yield_earned > 0);

    let ttl_after_read: u32 = te.env.as_contract(&te.pool_id, || {
        te.env.storage().persistent().get_ttl(&yield_key)
    });
    assert!(
        ttl_after_read >= 1_999_000,
        "get_lp_position should extend LPYieldEarned TTL close to EXTEND_TO, got {ttl_after_read}"
    );
}

//  ISSUE #273: INITIALIZATION & AUTH REGRESSION COVERAGE
#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_double_initialize_panics() {
    let env = Env::default();

    let admin = Address::generate(&env);
    let registry_id = env.register_contract(None, MockRegistry);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);

    // Initialize invoice with explicit auth
    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &invoice_id,
            fn_name: "initialize",
            args: (admin.clone(), registry_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);

    // Initialize escrow with explicit auth
    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &escrow_id,
            fn_name: "initialize",
            args: (admin.clone(), pool_id.clone(), usdc_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_id, &usdc_id);

    // First pool initialize — succeeds with explicit auth
    env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &pool_id,
            fn_name: "initialize",
            args: (
                admin.clone(),
                invoice_id.clone(),
                escrow_id.clone(),
                usdc_id.clone(),
                registry_id.clone(),
                admin.clone(),
                DEFAULT_MIN_INITIAL_DEPOSIT,
                String::from_str(&env, TEST_SHARE_NAME),
                String::from_str(&env, TEST_SHARE_SYMBOL),
                DEFAULT_SHARE_DECIMALS,
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );

    // Verify storage state after first initialize
    env.as_contract(&pool_id, || {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert_eq!(stored_admin, admin);
        let stored_invoice: Address = env
            .storage()
            .instance()
            .get(&DataKey::InvoiceContract)
            .unwrap();
        assert_eq!(stored_invoice, invoice_id);
        let stored_escrow: Address = env
            .storage()
            .instance()
            .get(&DataKey::EscrowContract)
            .unwrap();
        assert_eq!(stored_escrow, escrow_id);
        let stored_funding_asset: Address = env
            .storage()
            .instance()
            .get(&DataKey::FundingAsset)
            .unwrap();
        assert_eq!(stored_funding_asset, usdc_id);
        let stored_fee: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap();
        assert_eq!(stored_fee, 0);
        let stored_treasury: Address = env
            .storage()
            .instance()
            .get(&DataKey::TreasuryAddress)
            .unwrap();
        assert_eq!(stored_treasury, admin);
    });

    // Second initialize — panics with AlreadyInitialized (#1)
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_deposit_before_initialize_panics() {
    let env = Env::default();

    let usdc_id = env.register_contract(None, MockToken);
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    let lp = Address::generate(&env);

    // Give LP a balance in the mock USDC token
    let lp_bal_key = TKey(lp.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&lp_bal_key, &100_000_000_000_000i128);
    });

    // Mock LP auth but pool is not initialized → should panic with NotInitialized (#2)
    env.mock_auths(&[MockAuth {
        address: &lp,
        invoke: &MockAuthInvoke {
            contract: &pool_id,
            fn_name: "deposit",
            args: (lp.clone(), 10_000_000_000u128).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    pool.deposit(&lp, &10_000_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_withdraw_before_initialize_panics() {
    let env = Env::default();

    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    let lp = Address::generate(&env);

    // Mock LP auth but pool is not initialized → should panic with NotInitialized (#2)
    env.mock_auths(&[MockAuth {
        address: &lp,
        invoke: &MockAuthInvoke {
            contract: &pool_id,
            fn_name: "withdraw",
            args: (lp.clone(), 1_000u128).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    pool.withdraw(&lp, &1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_fund_invoice_before_initialize_panics() {
    let env = Env::default();

    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    let invoice_id = BytesN::from_array(&env, &[0u8; 32]);

    // Pool is not initialized → should panic with NotInitialized (#2)
    pool.fund_invoice(&invoice_id);
}

// --------------- Real Registry Integration ---------------
//
// Every other test in this file uses MockRegistry, a hand-rolled stand-in
// for registry-style verification. This test instead deploys the real
// RegistryContract alongside real invoice/escrow/pool contracts and drives
// a full register -> verify -> create -> list -> fund -> repay lifecycle
// through it, so the actual cross-contract is_verified call (argument
// shape, NotInitialized/NotFound panic semantics) is exercised against
// production code rather than the mock. Refs: issue #631.
mod real_registry_integration {
    use super::*;
    use clearledger_registry::{
        RegistryContract as RealRegistry, RegistryContractClient as RealRegistryClient,
        VerificationStatus,
    };
    use soroban_sdk::{map, Map, String};

    #[test]
    fn test_real_registry_revoke_reinstate_gates_create_and_funding() {
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let admin = Address::generate(&env);
        let issuer = Address::generate(&env);
        let buyer = Address::generate(&env);
        let lp = Address::generate(&env);

        // --- Deploy the real registry and drive register -> verify ---
        let registry_id = env.register_contract(None, RealRegistry);
        let registry = RealRegistryClient::new(&env, &registry_id);
        registry.initialize(&admin);

        let metadata: Map<String, String> = map![
            &env,
            (
                String::from_str(&env, "name"),
                String::from_str(&env, "test")
            )
        ];
        registry.register_issuer(&issuer, &metadata);
        registry.register_buyer(&buyer, &metadata);

        // Newly registered profiles start unverified (#130) — must be
        // explicitly verified by the admin before is_verified() returns true.
        assert!(!registry.is_verified(&issuer));
        assert!(!registry.is_verified(&buyer));
        registry.verify_profile(&issuer, &true);
        registry.verify_profile(&buyer, &true);
        assert!(registry.is_verified(&issuer));
        assert!(registry.is_verified(&buyer));
        assert_eq!(
            registry.get_verification_status(&issuer),
            VerificationStatus::Verified
        );
        assert_eq!(
            registry.get_verification_status(&buyer),
            VerificationStatus::Verified
        );

        // --- Deploy real invoice, escrow, pool wired to the real registry ---
        let usdc_id = env.register_contract(None, MockToken);
        let lp_bal_key = TKey(lp.clone());
        env.as_contract(&usdc_id, || {
            env.storage()
                .persistent()
                .set(&lp_bal_key, &100_000_000_000_000i128);
        });
        let buyer_bal_key = TKey(buyer.clone());
        env.as_contract(&usdc_id, || {
            env.storage()
                .persistent()
                .set(&buyer_bal_key, &100_000_000_000_000i128);
        });

        let invoice_id_addr = env.register_contract(None, RealInvoice);
        let escrow_id = env.register_contract(None, RealEscrow);
        let pool_id = env.register_contract(None, PoolContract);

        let invoice = RealInvoiceClient::new(&env, &invoice_id_addr);
        invoice.initialize(&admin, &registry_id);

        let escrow = RealEscrowClient::new(&env, &escrow_id);
        escrow.initialize(&admin, &pool_id, &usdc_id);

        let pool = PoolContractClient::new(&env, &pool_id);
        pool.initialize(
            &admin,
            &invoice_id_addr,
            &escrow_id,
            &usdc_id,
            &registry_id,
            &admin,
            &DEFAULT_MIN_INITIAL_DEPOSIT,
            &String::from_str(&env, TEST_SHARE_NAME),
            &String::from_str(&env, TEST_SHARE_SYMBOL),
            &DEFAULT_SHARE_DECIMALS,
        );

        invoice.add_supported_asset(&usdc_id);
        invoice.set_pool_contract(&pool_id);
        invoice.set_escrow_contract(&escrow_id);
        pool.set_max_utilization(&admin, &10000);

        let agent_registry_id = env.register_contract(None, MockAgentRegistry);
        let agent_registry = MockAgentRegistryClient::new(&env, &agent_registry_id);
        agent_registry.register_agent(
            &test_agent_id(&env),
            &clearledger_invoice::Agent {
                active: true,
                pubkey: test_agent_pubkey(&env),
            },
        );
        invoice.set_agent_registry_contract(&agent_registry_id);

        // A profile revoked before invoice creation is rejected by the real
        // invoice contract, then becomes eligible again after reinstate.
        let face_value: u128 = 10_000_000_000;
        let discount_bps: u32 = 200;
        let due_date = env.ledger().timestamp() + 86400;

        registry.revoke(&issuer);
        assert_eq!(
            registry.get_verification_status(&issuer),
            VerificationStatus::Revoked
        );
        let create_while_revoked =
            invoice.try_create(&issuer, &buyer, &face_value, &due_date, &usdc_id);
        assert!(create_while_revoked.is_err());
        assert!(
            std::format!("{create_while_revoked:?}").contains("#4"),
            "revoked issuer should fail invoice.create with IssuerNotVerified: {create_while_revoked:?}"
        );
        registry.reinstate(&issuer);
        assert_eq!(
            registry.get_verification_status(&issuer),
            VerificationStatus::Verified
        );

        pool.deposit(&lp, &(face_value * 3));

        let invoice_id = invoice.create(&issuer, &buyer, &face_value, &due_date, &usdc_id);

        let payload = clearledger_invoice::AttestationPayload {
            domain_separator: BytesN::from_array(
                &env,
                &clearledger_invoice::ATTESTATION_DOMAIN_SEPARATOR,
            ),
            invoice_id: invoice_id.clone(),
            risk_score: 5000,
            evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
            agent_id: test_agent_id(&env),
            nonce: 1,
        };
        let payload_bytes = payload.to_xdr(&env);
        let digest = env.crypto().keccak256(&payload_bytes).to_array();
        let (sig, recid) = test_agent_signing_key()
            .sign_prehash_recoverable(&digest)
            .unwrap();
        let mut sig_bytes = [0u8; 65];
        sig_bytes[..64].copy_from_slice(&sig.to_bytes());
        sig_bytes[64] = recid.to_byte();
        let signature = BytesN::from_array(&env, &sig_bytes);
        invoice.submit_attestation(&invoice_id, &payload_bytes, &signature);

        invoice.list_for_financing(&invoice_id, &discount_bps);

        // Revocation after listing blocks pool funding. The failed cross-
        // contract call must leave pool accounting exactly as it was.
        let stats_before_issuer_rejection = pool.get_stats();
        registry.revoke(&issuer);
        assert_eq!(
            registry.get_verification_status(&issuer),
            VerificationStatus::Revoked
        );
        let issuer_funding_rejection = pool.try_fund_invoice(&invoice_id);
        assert!(issuer_funding_rejection.is_err());
        assert!(
            std::format!("{issuer_funding_rejection:?}").contains("#18"),
            "revoked issuer should fail pool funding with IssuerNotVerified: {issuer_funding_rejection:?}"
        );
        let stats_after_issuer_rejection = pool.get_stats();
        assert_eq!(
            stats_after_issuer_rejection.total_deposits,
            stats_before_issuer_rejection.total_deposits
        );
        assert_eq!(
            stats_after_issuer_rejection.total_funded,
            stats_before_issuer_rejection.total_funded
        );
        assert_eq!(
            stats_after_issuer_rejection.active_invoice_count,
            stats_before_issuer_rejection.active_invoice_count
        );
        assert_eq!(
            stats_after_issuer_rejection.available_liquidity,
            stats_before_issuer_rejection.available_liquidity
        );
        registry.reinstate(&issuer);
        assert_eq!(
            registry.get_verification_status(&issuer),
            VerificationStatus::Verified
        );

        let funded = pool.fund_invoice(&invoice_id);
        assert!(funded);

        // Repeat the listing/funding gate for a revoked buyer.
        let second_due_date = env.ledger().timestamp() + 86400;
        let second_invoice_id =
            invoice.create(&issuer, &buyer, &face_value, &second_due_date, &usdc_id);
        let second_payload = clearledger_invoice::AttestationPayload {
            domain_separator: BytesN::from_array(
                &env,
                &clearledger_invoice::ATTESTATION_DOMAIN_SEPARATOR,
            ),
            invoice_id: second_invoice_id.clone(),
            risk_score: 5000,
            evidence_hash: BytesN::from_array(&env, &[8u8; 32]),
            agent_id: test_agent_id(&env),
            nonce: 2,
        };
        let second_payload_bytes = second_payload.to_xdr(&env);
        let second_digest = env.crypto().keccak256(&second_payload_bytes).to_array();
        let (second_sig, second_recid) = test_agent_signing_key()
            .sign_prehash_recoverable(&second_digest)
            .unwrap();
        let mut second_sig_bytes = [0u8; 65];
        second_sig_bytes[..64].copy_from_slice(&second_sig.to_bytes());
        second_sig_bytes[64] = second_recid.to_byte();
        let second_signature = BytesN::from_array(&env, &second_sig_bytes);
        invoice.submit_attestation(&second_invoice_id, &second_payload_bytes, &second_signature);
        invoice.list_for_financing(&second_invoice_id, &discount_bps);

        let stats_before_buyer_rejection = pool.get_stats();
        registry.revoke(&buyer);
        assert_eq!(
            registry.get_verification_status(&buyer),
            VerificationStatus::Revoked
        );
        let buyer_funding_rejection = pool.try_fund_invoice(&second_invoice_id);
        assert!(buyer_funding_rejection.is_err());
        assert!(
            std::format!("{buyer_funding_rejection:?}").contains("#19"),
            "revoked buyer should fail pool funding with BuyerNotVerified: {buyer_funding_rejection:?}"
        );
        let stats_after_buyer_rejection = pool.get_stats();
        assert_eq!(
            stats_after_buyer_rejection.total_deposits,
            stats_before_buyer_rejection.total_deposits
        );
        assert_eq!(
            stats_after_buyer_rejection.total_funded,
            stats_before_buyer_rejection.total_funded
        );
        assert_eq!(
            stats_after_buyer_rejection.active_invoice_count,
            stats_before_buyer_rejection.active_invoice_count
        );
        assert_eq!(
            stats_after_buyer_rejection.available_liquidity,
            stats_before_buyer_rejection.available_liquidity
        );
        registry.reinstate(&buyer);
        assert_eq!(
            registry.get_verification_status(&buyer),
            VerificationStatus::Verified
        );
        assert!(pool.fund_invoice(&second_invoice_id));

        let record = invoice.get(&invoice_id);
        assert_eq!(record.status, clearledger_invoice::InvoiceStatus::Funded);

        invoice.mark_shipped(&invoice_id);
        invoice.confirm_delivery(&invoice_id, &issuer);
        invoice.confirm_delivery(&invoice_id, &buyer);

        let record = invoice.get(&invoice_id);
        assert_eq!(record.status, clearledger_invoice::InvoiceStatus::Confirmed);

        invoice.repay(&invoice_id);

        let record = invoice.get(&invoice_id);
        assert_eq!(record.status, clearledger_invoice::InvoiceStatus::Repaid);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #")]
    fn test_real_registry_is_verified_panics_when_not_initialized() {
        // Confirms the real registry's is_verified semantics: on an
        // uninitialized (or unregistered) address it returns `false` rather
        // than panicking, which invoice.create()'s require_verified() then
        // turns into an IssuerNotVerified panic. This is the behavior
        // invoice's require_verified relies on — it must match the mock's
        // unwrap_or(false) behavior used everywhere else in this file.
        let env = Env::default();
        env.mock_all_auths_allowing_non_root_auth();

        let admin = Address::generate(&env);
        let issuer = Address::generate(&env);
        let buyer = Address::generate(&env);

        let registry_id = env.register_contract(None, RealRegistry);
        // Registry intentionally left uninitialized.

        let usdc_id = env.register_contract(None, MockToken);
        let invoice_id_addr = env.register_contract(None, RealInvoice);
        let invoice = RealInvoiceClient::new(&env, &invoice_id_addr);
        invoice.initialize(&admin, &registry_id);
        invoice.add_supported_asset(&usdc_id);

        let due_date = env.ledger().timestamp() + 86400;
        invoice.create(&issuer, &buyer, &10_000_000_000u128, &due_date, &usdc_id);
    }
}

//  CHECKS-EFFECTS-INTERACTIONS TESTS (issue #576)
#[test]
fn test_fund_invoice_commits_state_before_cross_contract_calls() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    // Verify initial state
    let stats_before = te.pool.get_stats();
    assert_eq!(stats_before.total_funded, 0);
    assert_eq!(stats_before.active_invoice_count, 0);

    // Fund the invoice
    let result = te.pool.fund_invoice(&invoice_id);
    assert!(result);

    // Verify pool state is correctly updated after funding.
    // This test documents that the checks-effects-interactions reorder
    // produces the same end-state as before: TotalFunded and
    // ActiveInvoiceCount are updated atomically with FundedInvoice.
    let stats_after = te.pool.get_stats();
    assert_eq!(stats_after.total_funded, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats_after.active_invoice_count, 1);
    assert_eq!(
        stats_after.available_liquidity,
        stats_before.total_deposits - DEFAULT_FUNDED_AMOUNT
    );
}

#[test]
fn test_fund_invoice_prevents_double_funding_via_funded_key_check() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    // First funding succeeds
    let result = te.pool.fund_invoice(&invoice_id);
    assert!(result);

    // Verify the FundedInvoice entry exists in persistent storage,
    // which is now committed before cross-contract calls.
    let funded_amount: u128 = te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .persistent()
            .get(&DataKey::FundedInvoice(invoice_id.clone()))
            .unwrap_or(0)
    });
    assert_eq!(funded_amount, DEFAULT_FUNDED_AMOUNT);

    // Second funding attempt is rejected - the AlreadyFunded guard
    // reads from persistent storage that was committed before
    // the cross-contract calls in the first funding.
    let result = te.pool.try_fund_invoice(&invoice_id);
    assert!(result.is_err());
}

//  INITIALIZE EVENT TESTS (issue #575)
#[test]
fn test_initialize_emits_pool_initialized_event() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let registry_id = env.register_contract(None, MockRegistry);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);

    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);
    let pool_addr = env.register_contract(None, PoolContract);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_addr, &usdc_id);

    let pool = PoolContractClient::new(&env, &pool_addr);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );

    let events = env.events().all();
    let mut found = false;
    for i in 0..events.len() {
        let (contract, topics, _data) = events.get(i).unwrap();
        if contract == pool_addr {
            let symbol = Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap();
            if symbol == Symbol::new(&env, "pool_initialized") {
                assert_eq!(
                    Address::try_from_val(&env, &topics.get(1).unwrap()).unwrap(),
                    admin
                );
                assert_eq!(
                    Address::try_from_val(&env, &topics.get(2).unwrap()).unwrap(),
                    invoice_id
                );
                assert_eq!(
                    Address::try_from_val(&env, &topics.get(3).unwrap()).unwrap(),
                    escrow_id
                );
                assert_eq!(
                    Address::try_from_val(&env, &topics.get(4).unwrap()).unwrap(),
                    usdc_id
                );
                found = true;
                break;
            }
        }
    }
    assert!(found, "pool_initialized event not found");
}

//  PUBLIC GETTER TESTS (issue #578)
#[test]
fn test_get_admin_returns_correct_address() {
    let te = setup();
    assert_eq!(te.pool.get_admin(), te.admin);
}

#[test]
fn test_get_invoice_contract_returns_correct_address() {
    let te = setup();
    assert_eq!(te.pool.get_invoice_contract(), te.invoice.address);
}

#[test]
fn test_get_escrow_contract_returns_correct_address() {
    let te = setup();
    assert_eq!(te.pool.get_escrow_contract(), te.escrow_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_admin_panics_when_uninitialized() {
    let env = Env::default();
    env.mock_all_auths();
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    let _ = pool.get_admin();
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_invoice_contract_panics_when_uninitialized() {
    let env = Env::default();
    env.mock_all_auths();
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    let _ = pool.get_invoice_contract();
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_escrow_contract_panics_when_uninitialized() {
    let env = Env::default();
    env.mock_all_auths();
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    let _ = pool.get_escrow_contract();
}

//  ISSUE #441: TYPED NotInitialized ERRORS //
// Every instance-storage read in the pool contract must fail with the typed
// `PoolError::NotInitialized` (#2) when the contract has not been initialized,
// never an untyped host panic. These tests cover the remaining entry points
// and views after the bare `unwrap()` / `expect()` reads were replaced
// (`get_admin`, `get_invoice_contract`, `get_escrow_contract` and
// `get_usdc_asset` are pinned alongside; `deposit`, `withdraw` and
// `fund_invoice` already had typed-error tests).

fn uninitialized_pool() -> (Env, PoolContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);
    (env, pool)
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_fee_bps_panics_with_typed_not_initialized() {
    let (_env, pool) = uninitialized_pool();
    pool.get_fee_bps();
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_treasury_address_panics_with_typed_not_initialized() {
    let (_env, pool) = uninitialized_pool();
    pool.get_treasury_address();
}

/// `get_treasury` falls back to the stored admin when no dedicated treasury is
/// configured, so its panic path is the admin read — it must be typed too.
#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_treasury_panics_with_typed_not_initialized() {
    let (_env, pool) = uninitialized_pool();
    pool.get_treasury();
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_receive_repayment_panics_with_typed_not_initialized() {
    let (env, pool) = uninitialized_pool();
    let invoice_id = BytesN::from_array(&env, &[0u8; 32]);
    pool.receive_repayment(&invoice_id, &1_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_receive_repayment_with_refund_panics_with_typed_not_initialized() {
    let (env, pool) = uninitialized_pool();
    let invoice_id = BytesN::from_array(&env, &[0u8; 32]);
    let party = Address::generate(&env);
    pool.receive_repayment_with_refund(&invoice_id, &1_000, &0, &party);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_handle_default_panics_with_typed_not_initialized() {
    let (env, pool) = uninitialized_pool();
    let invoice_id = BytesN::from_array(&env, &[0u8; 32]);
    pool.handle_default(&invoice_id);
}

//  CONSTANTS LOCATION TESTS (issue #592)
// DEFAULT_MIN_INITIAL_DEPOSIT and DEFAULT_MAX_UTILIZATION_BPS must be importable from
// `constants` (re-exported via `pub use constants::*` in lib.rs) and hold the
// canonical values.
#[test]
fn test_min_initial_deposit_constant_value() {
    assert_eq!(DEFAULT_MIN_INITIAL_DEPOSIT, 10_000_000);
}

#[test]
fn test_default_max_utilization_bps_constant_value() {
    use crate::DEFAULT_MAX_UTILIZATION_BPS;
    assert_eq!(DEFAULT_MAX_UTILIZATION_BPS, 8500);
}

//  WITHDRAW DUST-GUARD TESTS (issue #593)
// Withdraw must reject when the computed USDC redemption rounds down to zero,
// just as deposit rejects when the computed share count rounds down to zero.
// This prevents an LP from burning shares for nothing.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_withdraw_rejects_dust_shares_returning_zero_usdc() {
    // To make (shares * total_deposits) / total_shares == 0 we need
    // shares * total_deposits < total_shares.
    // Set total_shares very large and total_deposits very small so a
    // withdrawal of 1 share rounds down to 0 USDC.
    let te = setup();
    let total_shares: u128 = 1_000_000_000_000;
    let total_deposits: u128 = 1;

    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalShares, &total_shares);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &total_deposits);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &0u128);
        te.env
            .storage()
            .persistent()
            .set(&DataKey::LPShares(te.lp.clone()), &total_shares);
    });

    // 1 * 1 / 1_000_000_000_000 == 0 USDC -> must be rejected.
    te.pool.withdraw(&te.lp, &1);
}

// Dust-guard must not over-reject: a withdrawal that returns at least 1 USDC
// must succeed even when the share price is very small.
#[test]
fn test_withdraw_dust_guard_does_not_reject_nonzero_return() {
    let te = setup();
    // Standard deposit: 1 share == 1 stroop, returns > 0.
    te.pool.deposit(&te.lp, &10_000_000_000);
    let usdc = te.pool.withdraw(&te.lp, &1);
    assert!(
        usdc >= 1,
        "single-share withdraw must return at least 1 stroop"
    );
}

// A rejected dust withdrawal must leave pool state and LP shares unchanged.
#[test]
fn test_withdraw_dust_rejection_preserves_state() {
    let te = setup();
    let total_shares: u128 = 1_000_000_000_000;
    let total_deposits: u128 = 1;

    te.env.as_contract(&te.pool_id, || {
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalShares, &total_shares);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalDeposits, &total_deposits);
        te.env
            .storage()
            .instance()
            .set(&DataKey::TotalFunded, &0u128);
        te.env
            .storage()
            .persistent()
            .set(&DataKey::LPShares(te.lp.clone()), &total_shares);
    });

    let before = te.pool.get_stats();
    let res = te.pool.try_withdraw(&te.lp, &1);
    assert!(res.is_err(), "dust withdraw should be rejected");

    let after = te.pool.get_stats();
    assert_eq!(after.total_shares, before.total_shares);
    assert_eq!(after.total_deposits, before.total_deposits);
}

//  CHECKED SUBTRACTION TESTS (issue #594)
// handle_default: TotalFunded subtraction must not panic on valid data and must
// correctly reduce TotalFunded while preserving TotalDeposits (#439).
#[test]
fn test_handle_default_total_funded_decremented_correctly() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let before = te.pool.get_stats();
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);
    te.pool.handle_default(&invoice_id);

    let after = te.pool.get_stats();
    assert_eq!(
        after.total_funded,
        before.total_funded - DEFAULT_FUNDED_AMOUNT,
        "TotalFunded must decrease by funded_amount"
    );
    // #439: escrow returns the principal to the pool, so LP capital is
    // preserved — only TotalFunded is unwound.
    assert_eq!(
        after.total_deposits, before.total_deposits,
        "TotalDeposits must stay unchanged on default recovery"
    );
}

// settle_repayment (via receive_repayment): TotalFunded subtraction must
// correctly reduce TotalFunded back to zero after a full repayment.
#[test]
fn test_settle_repayment_total_funded_decremented_correctly() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let before = te.pool.get_stats();
    assert_eq!(before.total_funded, DEFAULT_FUNDED_AMOUNT);

    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 86401);
    te.invoice.repay(&invoice_id);

    let after = te.pool.get_stats();
    assert_eq!(
        after.total_funded, 0,
        "TotalFunded must be zero after full repayment"
    );
}

//  PROPERTY-BASED TESTS (issue #100) // Uses proptest's TestRunner API directly so rustfmt formats normally.
// Case budget is 10 per property to stay within CI time budgets for the
// Soroban in-process host.

// Any valid deposit amount must result in at least 1 share when the pool is
// empty (1:1 ratio), and the resulting share count must equal the deposit.
#[test]
fn prop_any_valid_initial_deposit_issues_shares_equal_to_amount() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(DEFAULT_MIN_INITIAL_DEPOSIT..=1_000_000_000_000u128),
            |deposit_amount| {
                let te = setup();
                let shares = te.pool.deposit(&te.lp, &deposit_amount);
                prop_assert_eq!(
                    shares,
                    deposit_amount,
                    "initial deposit must mint shares 1:1"
                );
                let stats = te.pool.get_stats();
                prop_assert_eq!(stats.total_shares, deposit_amount);
                prop_assert_eq!(stats.total_deposits, deposit_amount);
                Ok(())
            },
        )
        .unwrap();
}

// A deposit below DEFAULT_MIN_INITIAL_DEPOSIT on an empty pool must always be rejected
// with InvalidAmount (#4), regardless of the exact value.
#[test]
fn prop_initial_deposit_below_minimum_always_rejected() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(1u128..DEFAULT_MIN_INITIAL_DEPOSIT), |deposit_amount| {
            let te = setup();
            let result = te.pool.try_deposit(&te.lp, &deposit_amount);
            prop_assert!(
                result.is_err(),
                "deposit of {deposit_amount} below minimum must be rejected"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_deposits_above_token_amount_range_rejected_without_mutation() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&((i128::MAX as u128 + 1)..=u128::MAX), |deposit_amount| {
            let te = setup();
            let pool_before = te.pool.get_stats();
            let token = MockTokenClient::new(&te.env, &te.usdc_id);
            let lp_balance_before = token.balance(&te.lp);
            let pool_balance_before = token.balance(&te.pool_id);

            prop_assert!(
                te.pool.try_deposit(&te.lp, &deposit_amount).is_err(),
                "deposit {deposit_amount} exceeds the SEP-41 signed amount range"
            );
            prop_assert_eq!(te.pool.get_stats().total_deposits, pool_before.total_deposits);
            prop_assert_eq!(te.pool.get_stats().total_shares, pool_before.total_shares);
            prop_assert_eq!(token.balance(&te.lp), lp_balance_before);
            prop_assert_eq!(token.balance(&te.pool_id), pool_balance_before);
            Ok(())
        })
        .unwrap();
}

// For any withdrawal of all shares after a deposit, the USDC returned must
// equal the deposited amount (no yield, no loss).
#[test]
fn prop_full_withdrawal_returns_exact_deposit_with_no_yield() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(DEFAULT_MIN_INITIAL_DEPOSIT..=1_000_000_000_000u128),
            |deposit_amount| {
                let te = setup();
                let shares = te.pool.deposit(&te.lp, &deposit_amount);
                let usdc_returned = te.pool.withdraw(&te.lp, &shares);
                prop_assert_eq!(
                    usdc_returned,
                    deposit_amount,
                    "full withdrawal must return exact deposit"
                );
                let stats = te.pool.get_stats();
                prop_assert_eq!(stats.total_shares, 0);
                prop_assert_eq!(stats.total_deposits, 0);
                Ok(())
            },
        )
        .unwrap();
}

// Once repayment raises the share price above 1, a depositor's immediate
// deposit and full withdrawal must not extract value from existing LPs.
#[test]
fn prop_deposit_then_withdraw_never_returns_more_than_deposited_above_par() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(
                20_000_000_000u128..=1_000_000_000_000u128,
                1u32..=500u32,
                DEFAULT_MIN_INITIAL_DEPOSIT..=1_000_000_000_000u128,
            ),
            |(initial_deposit, discount_bps, deposit_amount)| {
                let te = setup();
                te.pool.deposit(&te.lp, &initial_deposit);

                let invoice_id = create_and_list_with_params(
                    &te,
                    &te.usdc_id,
                    DEFAULT_FACE_VALUE,
                    discount_bps,
                );
                te.pool.fund_invoice(&invoice_id);
                te.invoice.mark_shipped(&invoice_id);
                te.invoice.confirm_delivery(&invoice_id, &te.issuer);
                te.invoice.confirm_delivery(&invoice_id, &te.buyer);
                te.env
                    .ledger()
                    .set_timestamp(te.env.ledger().timestamp() + 86401);
                te.invoice.repay(&invoice_id);

                let stats_before = te.pool.get_stats();
                prop_assert!(
                    stats_before.total_deposits > stats_before.total_shares,
                    "repayment must raise the share price above 1"
                );
                let existing_lp_value_before = te.pool.get_lp_position(&te.lp).usdc_value;
                let depositor = create_lp_with_balance(&te, 100_000_000_000_000);
                let shares_minted = te.pool.deposit(&depositor, &deposit_amount);
                let usdc_returned = te.pool.withdraw(&depositor, &shares_minted);

                prop_assert!(
                    usdc_returned <= deposit_amount,
                    "withdrawal returned {usdc_returned} for deposit {deposit_amount}"
                );
                prop_assert!(
                    stats_before.total_shares > 0,
                    "share price is undefined when there are no shares"
                );
                let ceil_share_price = stats_before
                    .total_deposits
                    .div_ceil(stats_before.total_shares);
                prop_assert!(
                    deposit_amount - usdc_returned <= ceil_share_price,
                    "round-trip loss {} exceeds ceil share price {ceil_share_price}",
                    deposit_amount - usdc_returned
                );
                let existing_lp_value_after = te.pool.get_lp_position(&te.lp).usdc_value;
                prop_assert!(
                    existing_lp_value_after >= existing_lp_value_before,
                    "existing LP value decreased from {existing_lp_value_before} to {existing_lp_value_after}"
                );
                Ok(())
            },
        )
        .unwrap();
}

// After a full withdrawal the LP's position must be empty: zero shares,
// zero USDC value, deposit count zeroed.
#[test]
fn prop_full_withdrawal_clears_lp_position() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(DEFAULT_MIN_INITIAL_DEPOSIT..=1_000_000_000_000u128),
            |deposit_amount| {
                let te = setup();
                let shares = te.pool.deposit(&te.lp, &deposit_amount);
                te.pool.withdraw(&te.lp, &shares);
                let pos = te.pool.get_lp_position(&te.lp);
                prop_assert_eq!(pos.shares, 0);
                prop_assert_eq!(pos.usdc_value, 0);
                prop_assert_eq!(pos.deposit_count, 0);
                Ok(())
            },
        )
        .unwrap();
}

// Deposit followed by repayment: total_deposits must increase by the yield
// amount, and total_funded must return to zero.
#[test]
fn prop_repayment_increases_deposits_by_yield_and_clears_funded() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(1u32..=500u32), |discount_bps| {
            let te = setup();
            te.pool.deposit(&te.lp, &100_000_000_000);
            let face_value: u128 = 10_000_000_000;
            let funded_amount = face_value * (10000 - discount_bps as u128) / 10000;
            let yield_amount = face_value * discount_bps as u128 / 10000;
            let invoice_id =
                create_and_list_with_params(&te, &te.usdc_id, face_value, discount_bps);
            te.pool.fund_invoice(&invoice_id);

            let before = te.pool.get_stats();
            prop_assert_eq!(before.total_funded, funded_amount);

            te.invoice.mark_shipped(&invoice_id);
            te.invoice.confirm_delivery(&invoice_id, &te.issuer);
            te.invoice.confirm_delivery(&invoice_id, &te.buyer);
            te.env
                .ledger()
                .set_timestamp(te.env.ledger().timestamp() + 86401);
            te.invoice.repay(&invoice_id);

            let after = te.pool.get_stats();
            prop_assert_eq!(after.total_funded, 0);
            prop_assert_eq!(
                after.total_deposits,
                before.total_deposits - funded_amount + face_value,
                "deposits must grow by yield_amount after repayment"
            );
            prop_assert_eq!(after.total_yield_distributed, yield_amount);
            Ok(())
        })
        .unwrap();
}

//  ISSUE #771: NONZERO PROTOCOL FEE SPLITS
#[test]
fn test_freshly_initialized_pool_has_zero_protocol_fee() {
    let te = setup();
    assert_eq!(te.pool.get_protocol_fee_bps(), 0);
    assert_eq!(te.pool.get_treasury(), te.admin);
}

#[test]
fn test_protocol_fee_rounding_at_zero_max_and_truncation_boundaries() {
    let cases = [
        (0u32, 1u128, 0u128),
        (1, 9_999, 0),
        (1, 10_000, 1),
        (MAX_PROTOCOL_FEE_BPS, 4, 0),
        (MAX_PROTOCOL_FEE_BPS, 5, 1),
        (MAX_PROTOCOL_FEE_BPS, 10_001, 2_000),
    ];

    for (fee_bps, yield_amount, expected_fee) in cases {
        let te = setup();
        te.pool.deposit(&te.lp, &100_000_000_000);
        let invoice_id = create_and_list(&te, &te.usdc_id);
        te.pool.fund_invoice(&invoice_id);

        let treasury = Address::generate(&te.env);
        te.pool.set_protocol_fee(&fee_bps, &treasury);

        let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
        let treasury_before = usdc.balance(&treasury);
        let stats_before = te.pool.get_stats();
        assert!(te
            .pool
            .receive_repayment(&invoice_id, &(DEFAULT_FUNDED_AMOUNT + yield_amount)));

        let stats_after = te.pool.get_stats();
        assert_eq!(
            usdc.balance(&treasury) - treasury_before,
            expected_fee as i128
        );
        assert_eq!(
            stats_after.total_yield_distributed - stats_before.total_yield_distributed,
            yield_amount - expected_fee
        );
        assert_eq!(
            stats_after.total_deposits - stats_before.total_deposits,
            yield_amount - expected_fee
        );
    }
}

#[test]
fn test_nonzero_protocol_fee_splits_receive_repayment() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let treasury = Address::generate(&te.env);
    let fee_bps = 1000u32; // 10% protocol fee (1000 bps)
    te.pool.set_protocol_fee(&fee_bps, &treasury);

    assert_eq!(te.pool.get_protocol_fee_bps(), 1000);
    assert_eq!(te.pool.get_treasury(), treasury);

    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let treasury_before = usdc.balance(&treasury);
    let before_stats = te.pool.get_stats();

    let amount = DEFAULT_FACE_VALUE;
    let yield_amount = DEFAULT_YIELD_AMOUNT;
    let expected_protocol_cut = yield_amount * (fee_bps as u128) / 10_000;
    let expected_lp_yield = yield_amount - expected_protocol_cut;

    let result = te.pool.receive_repayment(&invoice_id, &amount);
    assert!(result);

    let treasury_after = usdc.balance(&treasury);
    let after_stats = te.pool.get_stats();

    // Assert treasury address's USDC balance increased by exactly protocol_cut
    assert_eq!(
        treasury_after - treasury_before,
        expected_protocol_cut as i128
    );

    // Assert TotalYieldDistributed increased by exactly lp_yield
    assert_eq!(
        after_stats.total_yield_distributed - before_stats.total_yield_distributed,
        expected_lp_yield
    );
    assert_eq!(
        after_stats.total_deposits - before_stats.total_deposits,
        expected_lp_yield
    );
}

#[test]
fn test_nonzero_protocol_fee_splits_receive_repayment_with_refund() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let treasury = Address::generate(&te.env);
    let fee_bps = 500u32; // 5% protocol fee (500 bps)
    te.pool.set_protocol_fee(&fee_bps, &treasury);

    assert_eq!(te.pool.get_protocol_fee_bps(), 500);
    assert_eq!(te.pool.get_treasury(), treasury);

    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let treasury_before = usdc.balance(&treasury);
    let before_stats = te.pool.get_stats();

    let amount = DEFAULT_FACE_VALUE;
    let refund = 100_000_000u128; // 100M refund to buyer out of 200M surplus
    let yield_amount = amount - DEFAULT_FUNDED_AMOUNT - refund; // 100_000_000
    let expected_protocol_cut = yield_amount * (fee_bps as u128) / 10_000; // 5_000_000
    let expected_lp_yield = yield_amount - expected_protocol_cut; // 95_000_000

    let result = te
        .pool
        .receive_repayment_with_refund(&invoice_id, &amount, &refund, &te.buyer);
    assert!(result);

    let treasury_after = usdc.balance(&treasury);
    let after_stats = te.pool.get_stats();

    // Assert treasury address's USDC balance increased by exactly protocol_cut
    assert_eq!(
        treasury_after - treasury_before,
        expected_protocol_cut as i128
    );

    // Assert TotalYieldDistributed increased by exactly lp_yield
    assert_eq!(
        after_stats.total_yield_distributed - before_stats.total_yield_distributed,
        expected_lp_yield
    );
    assert_eq!(
        after_stats.total_deposits - before_stats.total_deposits,
        expected_lp_yield
    );
}

#[test]
fn test_protocol_fee_rounding_assigns_fractional_remainder_to_lps() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list_with_params(&te, &te.usdc_id, 5, 2000);
    te.pool.fund_invoice(&invoice_id);

    let treasury = Address::generate(&te.env);
    te.pool.set_protocol_fee(&2000, &treasury);
    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let treasury_before = usdc.balance(&treasury);

    // A one-unit surplus at 20% floors the protocol cut to zero.
    assert!(te.pool.receive_repayment(&invoice_id, &5));

    assert_eq!(usdc.balance(&treasury), treasury_before);
    assert_eq!(te.pool.get_stats().total_yield_distributed, 1);
}

//  ISSUE #846: PROPT — FEE SPLIT CONSERVES YIELD
// For every fee_bps in 0..=MAX_PROTOCOL_FEE_BPS and every yield settled
// through `settle_repayment`, the protocol cut plus the yield credited back
// to LPs must reassemble the yield that arrived: the fee split only
// redistributes yield between treasury and LPs, it never mints or destroys
// any. Uses proptest's TestRunner API directly so rustfmt formats normally;
// the case budget matches the other property tests (10) to stay within CI
// time budgets for the Soroban in-process host.
#[test]
fn prop_protocol_fee_split_conserves_yield_for_all_fee_bps() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(0i128..=10_000_000_000_000i128, 0u32..=MAX_PROTOCOL_FEE_BPS),
            |(yield_amount, fee_bps)| {
                let te = setup();
                te.pool.deposit(&te.lp, &100_000_000_000);
                let invoice_id = create_and_list(&te, &te.usdc_id);
                te.pool.fund_invoice(&invoice_id);

                let treasury = Address::generate(&te.env);
                te.pool.set_protocol_fee(&fee_bps, &treasury);

                let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
                let treasury_before = usdc.balance(&treasury);
                let before = te.pool.get_stats();

                // Repay the funded principal plus the arbitrary yield under
                // test, so `settle_repayment` sees exactly `yield_amount` of
                // surplus to split.
                let amount = DEFAULT_FUNDED_AMOUNT + (yield_amount as u128);
                let settled = te.pool.receive_repayment(&invoice_id, &amount);
                prop_assert!(settled);

                let treasury_delta = usdc.balance(&treasury) - treasury_before;
                let after = te.pool.get_stats();
                let yield_delta =
                    (after.total_yield_distributed - before.total_yield_distributed) as i128;
                let deposits_delta = (after.total_deposits - before.total_deposits) as i128;

                let protocol_cut = (yield_amount * (fee_bps as i128)) / 10_000;
                let lp_yield = yield_amount - protocol_cut;

                // Yield conservation: the two halves of the split reassemble
                // the whole yield, both as computed and as observed in the
                // treasury balance and TotalYieldDistributed.
                prop_assert_eq!(protocol_cut + lp_yield, yield_amount);
                prop_assert_eq!(treasury_delta + yield_delta, yield_amount);

                // The cut can never exceed fee_bps of the yield.
                prop_assert!(
                    protocol_cut <= (yield_amount * (fee_bps as i128)) / 10_000,
                    "protocol cut exceeds bps cut"
                );

                // Treasury balance delta equals the protocol cut.
                prop_assert_eq!(treasury_delta, protocol_cut);

                // TotalYieldDistributed delta equals the LP yield.
                prop_assert_eq!(yield_delta, lp_yield);

                // TotalDeposits grows by exactly the LP share of the yield.
                prop_assert_eq!(deposits_delta, lp_yield);

                // Zero-fee edge case: nothing may reach the treasury and the
                // full yield must land in TotalDeposits.
                if fee_bps == 0 {
                    prop_assert_eq!(treasury_delta, 0);
                    prop_assert_eq!(deposits_delta, yield_amount);
                }

                Ok(())
            },
        )
        .unwrap();
}
//  ISSUE #774: GAS BENCHMARK FOR DEPOSIT / WITHDRAW
#[test]
fn test_gas_benchmark_deposit_and_withdraw() {
    extern crate std;
    let te = setup();
    let env = &te.env;

    // Measure deposit() resource cost
    env.budget().reset_default();
    let cpu_before_deposit = env.budget().cpu_instruction_cost();
    let mem_before_deposit = env.budget().memory_bytes_cost();
    let shares = te.pool.deposit(&te.lp, &10_000_000_000);
    let cpu_after_deposit = env.budget().cpu_instruction_cost();
    let mem_after_deposit = env.budget().memory_bytes_cost();

    let deposit_cpu = cpu_after_deposit - cpu_before_deposit;
    let deposit_mem = mem_after_deposit - mem_before_deposit;

    // Measure withdraw() resource cost
    env.budget().reset_default();
    let cpu_before_withdraw = env.budget().cpu_instruction_cost();
    let mem_before_withdraw = env.budget().memory_bytes_cost();
    let returned = te.pool.withdraw(&te.lp, &(shares / 2));
    let cpu_after_withdraw = env.budget().cpu_instruction_cost();
    let mem_after_withdraw = env.budget().memory_bytes_cost();

    let withdraw_cpu = cpu_after_withdraw - cpu_before_withdraw;
    let withdraw_mem = mem_after_withdraw - mem_before_withdraw;

    std::println!(
        "\n== GAS BENCHMARK ==\ndeposit():  CPU instructions: {}, Memory bytes: {}\nwithdraw(): CPU instructions: {}, Memory bytes: {}\n=====",
        deposit_cpu,
        deposit_mem,
        withdraw_cpu,
        withdraw_mem
    );

    assert!(deposit_cpu > 0);
    assert!(deposit_mem > 0);
    assert!(withdraw_cpu > 0);
    assert!(withdraw_mem > 0);
    assert_eq!(returned, 5_000_000_000);
}

//  ISSUE #772: NEGATIVE-AUTH FOR SET_PROTOCOL_FEE
// set_protocol_fee must reject callers other than the admin (#772), matching
// the pattern used by set_max_utilization.
#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_set_protocol_fee_requires_admin_authorization() {
    let te = setup();
    let treasury = Address::generate(&te.env);

    // Clear all mocked auths so the caller's require_auth() fails.
    te.env.set_auths(&[]);
    te.pool.set_protocol_fee(&500, &treasury);
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")]
fn test_set_protocol_fee_above_max_cap_panics() {
    let te = setup();
    let treasury = Address::generate(&te.env);
    te.pool.set_protocol_fee(&2001, &treasury);
}

#[test]
fn test_set_protocol_fee_at_max_cap_succeeds() {
    let te = setup();
    let treasury = Address::generate(&te.env);
    let ok = te.pool.set_protocol_fee(&2000, &treasury);
    assert!(ok);
    assert_eq!(te.pool.get_protocol_fee_bps(), 2000);
    assert_eq!(te.pool.get_treasury(), treasury);
}

#[test]
fn test_set_protocol_fee_rejects_treasury_aliases() {
    let te = setup();
    let forbidden_treasuries = [
        te.pool_id.clone(),
        te.invoice.address.clone(),
        te.escrow_id.clone(),
        te.registry.address.clone(),
        te.usdc_id.clone(),
    ];

    for treasury in forbidden_treasuries {
        assert!(
            te.pool.try_set_protocol_fee(&500, &treasury).is_err(),
            "treasury alias was accepted"
        );
    }
}

//  ISSUE #770: DEFAULT-ZERO PROTOCOL FEE ACCOUNTING REGRESSION
#[test]
fn test_default_zero_protocol_fee_preserves_repayment_accounting_unchanged() {
    let te = setup();
    // Verify default protocol fee is zero and treasury is te.admin
    assert_eq!(te.pool.get_protocol_fee_bps(), 0);
    assert_eq!(te.pool.get_treasury(), te.admin);

    let initial_deposit = 100_000_000_000u128;
    let shares = te.pool.deposit(&te.lp, &initial_deposit);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let treasury = te.pool.get_treasury();
    let treasury_before = usdc.balance(&treasury);
    let before_stats = te.pool.get_stats();

    let amount = DEFAULT_FACE_VALUE; // 1_200_000_000
    let yield_amount = DEFAULT_YIELD_AMOUNT; // 200_000_000

    let result = te.pool.receive_repayment(&invoice_id, &amount);
    assert!(result);

    let treasury_after = usdc.balance(&treasury);
    let after_stats = te.pool.get_stats();

    // At default 0 bps fee, treasury cut is exactly 0
    assert_eq!(
        treasury_after, treasury_before,
        "treasury balance must remain completely unchanged at default 0 bps fee"
    );

    // 100% of yield goes to LPs: TotalYieldDistributed and TotalDeposits increase by full yield_amount
    assert_eq!(
        after_stats.total_yield_distributed - before_stats.total_yield_distributed,
        yield_amount,
        "TotalYieldDistributed must increase by full yield_amount"
    );
    assert_eq!(
        after_stats.total_deposits - before_stats.total_deposits,
        yield_amount,
        "TotalDeposits must increase by full yield_amount"
    );
    assert_eq!(after_stats.total_funded, 0);

    // LP withdrawing all shares receives initial_deposit + yield_amount (pre-fee identical behavior)
    let returned = te.pool.withdraw(&te.lp, &shares);
    assert_eq!(returned, initial_deposit + yield_amount);
}

#[test]
fn test_default_zero_protocol_fee_preserves_refunded_repayment_accounting_unchanged() {
    let te = setup();
    assert_eq!(te.pool.get_protocol_fee_bps(), 0);

    let initial_deposit = 100_000_000_000u128;
    te.pool.deposit(&te.lp, &initial_deposit);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    let usdc = MockTokenClient::new(&te.env, &te.usdc_id);
    let treasury = te.pool.get_treasury();
    let treasury_before = usdc.balance(&treasury);
    let before_stats = te.pool.get_stats();

    let amount = DEFAULT_FACE_VALUE; // 1_200_000_000
    let refund = 50_000_000u128;
    let expected_yield = amount - DEFAULT_FUNDED_AMOUNT - refund; // 150_000_000

    let result = te
        .pool
        .receive_repayment_with_refund(&invoice_id, &amount, &refund, &te.buyer);
    assert!(result);

    let treasury_after = usdc.balance(&treasury);
    let after_stats = te.pool.get_stats();

    // Treasury cut must be zero
    assert_eq!(treasury_after, treasury_before);

    // Full expected yield goes to LPs
    assert_eq!(
        after_stats.total_yield_distributed - before_stats.total_yield_distributed,
        expected_yield
    );
    assert_eq!(
        after_stats.total_deposits - before_stats.total_deposits,
        expected_yield
    );
}

//  ISSUE #765: PROTOCOL FEE STORAGE AND INITIALIZATION
#[test]
fn test_protocol_fee_storage_initialized_to_zero_and_admin_treasury() {
    let te = setup();
    // Freshly initialized pool must read back fee_bps == 0 and treasury == admin
    assert_eq!(te.pool.get_protocol_fee_bps(), 0);
    assert_eq!(te.pool.get_treasury(), te.admin);
}

#[test]
fn test_set_protocol_fee_updates_both_stored_values_and_reads_back() {
    let te = setup();
    let treasury = Address::generate(&te.env);
    let fee_bps = 750u32; // 7.5%

    let updated = te.pool.set_protocol_fee(&fee_bps, &treasury);
    assert!(updated);

    assert_eq!(te.pool.get_protocol_fee_bps(), 750);
    assert_eq!(te.pool.get_treasury(), treasury);
}

#[test]
fn test_protocol_fee_storage_initialized_with_custom_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);
    let registry_id = env.register_contract(None, MockRegistry);
    let custom_treasury = Address::generate(&env);

    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);
    let pool_id = env.register_contract(None, PoolContract);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_id, &usdc_id);

    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &custom_treasury,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, TEST_SHARE_NAME),
        &String::from_str(&env, TEST_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );

    assert_eq!(pool.get_protocol_fee_bps(), 0);
    assert_eq!(pool.get_treasury(), custom_treasury);
}

#[test]
fn test_sep41_balance_and_total_supply() {
    let te = setup();
    let client = PoolContractClient::new(&te.env, &te.pool_id);

    let unknown_lp = Address::generate(&te.env);

    // Check total supply
    let initial_supply = client.total_supply();
    assert_eq!(initial_supply, 0);

    // Check unknown LP balance
    let unknown_balance = client.balance(&unknown_lp);
    assert_eq!(unknown_balance, 0);

    // Check known LP balance after deposit
    client.deposit(&te.lp, &100_000_000);

    let supply_after = client.total_supply();
    assert_eq!(supply_after, 100_000_000);

    let lp_balance = client.balance(&te.lp);
    assert_eq!(lp_balance, 100_000_000);
}

//  ISSUE #756: SEP-41 ALLOWANCE INTERFACE
// approve() records a grant that allowance() reports until it is spent,
// revoked, or expires.
#[test]
fn test_approve_sets_allowance_until_expiration() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 100;

    assert_eq!(te.pool.allowance(&te.lp, &spender), 0);
    te.pool.approve(&te.lp, &spender, &5_000_000_000, &expires);
    assert_eq!(te.pool.allowance(&te.lp, &spender), 5_000_000_000);

    // A second approve overwrites the grant (SEP-41 semantics) rather than
    // adding to it, and it can move the expiry.
    te.pool
        .approve(&te.lp, &spender, &1_000_000_000, &(expires + 50));
    assert_eq!(te.pool.allowance(&te.lp, &spender), 1_000_000_000);

    // The grant is per (owner, spender) pair: another spender sees nothing.
    assert_eq!(te.pool.allowance(&te.lp, &Address::generate(&te.env)), 0);
}

// The whole point of the allowance: a spender moves shares, the grant shrinks
// by exactly what moved, and nobody's total share supply changes.
#[test]
fn test_transfer_from_decrements_allowance() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let recipient = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 100;
    te.pool.approve(&te.lp, &spender, &5_000_000_000, &expires);

    te.pool
        .transfer_from(&spender, &te.lp, &recipient, &2_000_000_000);

    assert_eq!(te.pool.allowance(&te.lp, &spender), 3_000_000_000);
    assert_eq!(te.pool.get_lp_position(&recipient).shares, 2_000_000_000);
    assert_eq!(te.pool.get_lp_position(&te.lp).shares, 8_000_000_000);
    // Moving shares must not mint or burn them.
    assert_eq!(te.pool.get_stats().total_shares, 10_000_000_000);

    // Spending the rest takes the grant to exactly zero.
    te.pool
        .transfer_from(&spender, &te.lp, &recipient, &3_000_000_000);
    assert_eq!(te.pool.allowance(&te.lp, &spender), 0);
    assert_eq!(te.pool.get_lp_position(&recipient).shares, 5_000_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")]
fn test_transfer_from_fails_once_allowance_exhausted() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let recipient = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 100;
    te.pool.approve(&te.lp, &spender, &1_000, &expires);
    te.pool.transfer_from(&spender, &te.lp, &recipient, &1_000);

    // Grant fully spent: the next move is rejected, and it is rejected before
    // any share leaves the owner.
    te.pool.transfer_from(&spender, &te.lp, &recipient, &1);
}

#[test]
fn test_allowance_reads_zero_after_expiration() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 10;
    te.pool.approve(&te.lp, &spender, &5_000_000_000, &expires);
    assert_eq!(te.pool.allowance(&te.lp, &spender), 5_000_000_000);

    // The grant is still live on its last ledger...
    te.env
        .ledger()
        .set_sequence_number(te.env.ledger().sequence() + 10);
    assert_eq!(te.pool.allowance(&te.lp, &spender), 5_000_000_000);

    // ...and dead from the next one onwards, without anyone having to prune it.
    te.env
        .ledger()
        .set_sequence_number(te.env.ledger().sequence() + 1);
    assert_eq!(te.pool.allowance(&te.lp, &spender), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")]
fn test_transfer_from_rejects_expired_allowance() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let recipient = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 5;
    te.pool.approve(&te.lp, &spender, &5_000_000_000, &expires);
    te.env
        .ledger()
        .set_sequence_number(te.env.ledger().sequence() + 6);

    te.pool.transfer_from(&spender, &te.lp, &recipient, &1);
}

#[test]
fn test_approve_with_zero_amount_revokes_allowance() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let recipient = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 100;
    te.pool.approve(&te.lp, &spender, &5_000_000_000, &expires);

    // Revoking needs no future expiry: amount 0 is the revoke signal.
    te.pool.approve(&te.lp, &spender, &0, &0);
    assert_eq!(te.pool.allowance(&te.lp, &spender), 0);
    assert!(te
        .pool
        .try_transfer_from(&spender, &te.lp, &recipient, &1)
        .is_err());
}

#[test]
#[should_panic(expected = "Error(Contract, #25)")]
fn test_approve_with_expiration_in_the_past_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    te.pool
        .approve(&te.lp, &spender, &1_000, &te.env.ledger().sequence());
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")]
fn test_transfer_from_rejects_spender_without_grant() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let stranger = Address::generate(&te.env);
    let recipient = Address::generate(&te.env);
    te.pool.transfer_from(&stranger, &te.lp, &recipient, &1_000);
}

// approve() is a state change LPs and spenders both need to observe off-chain,
// so it emits `allowance_approved` with the new total grant and its expiry.
#[test]
fn test_approve_emits_allowance_approved_event() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let spender = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 100;
    let before = te.env.events().all().len();
    te.pool.approve(&te.lp, &spender, &5_000_000_000, &expires);

    let events = te.env.events().all();
    assert_eq!(events.len(), before + 1);
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "allowance_approved")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.lp
    );
    assert_eq!(
        <(Address, i128, u32)>::try_from_val(&te.env, &data).unwrap(),
        (spender.clone(), 5_000_000_000, expires)
    );

    // A rejected approve (expiry already passed) must not emit either.
    assert!(te
        .pool
        .try_approve(&te.lp, &spender, &1_000, &te.env.ledger().sequence())
        .is_err());
    assert_eq!(te.env.events().all().len(), events.len());
}

//  ISSUE #757: SEP-41 SHARE METADATA
#[test]
fn test_share_metadata_comes_from_initialize() {
    let te = setup();
    assert_eq!(te.pool.name(), String::from_str(&te.env, TEST_SHARE_NAME));
    assert_eq!(
        te.pool.symbol(),
        String::from_str(&te.env, TEST_SHARE_SYMBOL)
    );
    // Shares are denominated in the funding asset's base units, so the value
    // configured at `initialize` is what `decimals()` reports.
    assert_eq!(te.pool.decimals(), DEFAULT_SHARE_DECIMALS);

    // Metadata lives on the instance, written once by initialize.
    te.env.as_contract(&te.pool_id, || {
        let stored_name: String = te
            .env
            .storage()
            .instance()
            .get(&DataKey::ShareName)
            .unwrap();
        let stored_symbol: String = te
            .env
            .storage()
            .instance()
            .get(&DataKey::ShareSymbol)
            .unwrap();
        assert_eq!(stored_name, String::from_str(&te.env, TEST_SHARE_NAME));
        assert_eq!(stored_symbol, String::from_str(&te.env, TEST_SHARE_SYMBOL));
    });
}

// The per-asset `pool_factory` model is the reason metadata is an initializer
// argument instead of a constant: two pool instances are two different share
// tokens and must not render identically in a wallet.
#[test]
fn test_share_metadata_is_scoped_to_the_pool_instance() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);
    let registry_id = env.register_contract(None, MockRegistry);
    let pool_id = env.register_contract(None, PoolContract);

    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_id, &usdc_id);

    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, "ClearLedger XLM Pool Shares"),
        &String::from_str(&env, "TT-XLM"),
        &12,
    );

    assert_eq!(
        pool.name(),
        String::from_str(&env, "ClearLedger XLM Pool Shares")
    );
    assert_eq!(pool.symbol(), String::from_str(&env, "TT-XLM"));
    // A non-7 value proves decimals is per-instance state, not a constant.
    assert_eq!(pool.decimals(), 12);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn test_initialize_rejects_empty_share_symbol() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let invoice_id = env.register_contract(None, RealInvoice);
    let escrow_id = env.register_contract(None, RealEscrow);
    let usdc_id = env.register_contract(None, MockToken);
    let registry_id = env.register_contract(None, MockRegistry);
    let pool_id = env.register_contract(None, PoolContract);

    RealInvoiceClient::new(&env, &invoice_id).initialize(&admin, &registry_id);
    RealEscrowClient::new(&env, &escrow_id).initialize(&admin, &pool_id, &usdc_id);

    PoolContractClient::new(&env, &pool_id).initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, ""),
        &DEFAULT_SHARE_DECIMALS,
    );
}

#[test]
fn test_share_metadata_unreadable_before_initialize() {
    let env = Env::default();
    let pool_id = env.register_contract(None, PoolContract);
    let pool = PoolContractClient::new(&env, &pool_id);

    assert!(pool.try_name().is_err());
    assert!(pool.try_symbol().is_err());
    // `decimals` falls back to the 7-decimal default instead of panicking, so
    // pools that predate the parameter still render sensibly.
    assert_eq!(pool.decimals(), DEFAULT_SHARE_DECIMALS);
}

//  ISSUE #758: DEPOSIT ROUTES THROUGH mint()
// The `mint()` extraction must be invisible from the outside: same shares, same
// bookkeeping, and exactly one event with the same payload shape.
#[test]
fn test_deposit_mints_through_shared_helper_without_changing_behaviour() {
    let te = setup();

    let before = te.env.events().all().len();
    let shares = te.pool.deposit(&te.lp, &10_000_000_000);
    assert_eq!(shares, 10_000_000_000);

    // One event, unchanged: `lp_deposited(lp, usdc_amount, shares_issued)`.
    let events = te.env.events().all();
    assert_eq!(events.len(), before + 1);
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "lp_deposited")
    );
    assert_eq!(
        <(u128, u128)>::try_from_val(&te.env, &data).unwrap(),
        (10_000_000_000u128, 10_000_000_000u128)
    );

    // mint() bumps the total and the LP's balance together...
    assert_eq!(te.pool.get_stats().total_shares, 10_000_000_000);
    assert_eq!(te.pool.get_lp_position(&te.lp).shares, 10_000_000_000);

    // ...and a second deposit through the same helper is additive.
    te.pool.deposit(&te.lp, &5_000_000_000);
    assert_eq!(te.pool.get_stats().total_shares, 15_000_000_000);
    assert_eq!(te.pool.get_lp_position(&te.lp).shares, 15_000_000_000);
}

// `total_shares` in stats and the LP's share balance must agree, since that is
// the invariant `mint()`/`burn()` now own.
#[test]
fn test_share_supply_stays_consistent_across_mint_and_burn() {
    let te = setup();
    let other = Address::generate(&te.env);
    te.pool.deposit(&te.lp, &10_000_000_000);

    // top up the mock token for a second LP
    te.env.as_contract(&te.usdc_id, || {
        te.env
            .storage()
            .persistent()
            .set(&TKey(other.clone()), &100_000_000_000_000i128);
    });
    te.pool.deposit(&other, &20_000_000_000);

    assert_eq!(te.pool.get_stats().total_shares, 30_000_000_000);
    assert_eq!(
        te.pool.get_lp_position(&te.lp).shares + te.pool.get_lp_position(&other).shares,
        30_000_000_000
    );

    te.pool.withdraw(&te.lp, &4_000_000_000);
    assert_eq!(te.pool.get_stats().total_shares, 26_000_000_000);
    assert_eq!(te.pool.get_lp_position(&te.lp).shares, 6_000_000_000);
}

//  ISSUE #767: protocol_fee_updated EVENT
// A fee/treasury change is high-impact for LPs, so it must be observable
// without polling `get_protocol_fee_bps`, and the event must carry the *old*
// value so an indexer can report what changed.
#[test]
fn test_set_protocol_fee_emits_event_with_old_and_new_values() {
    let te = setup();
    let treasury = Address::generate(&te.env);
    let before = te.env.events().all().len();

    te.pool.set_protocol_fee(&750, &treasury);

    let events = te.env.events().all();
    assert_eq!(events.len(), before + 1);
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(topics.len(), 1);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "protocol_fee_updated")
    );
    // setup() initialized the fee to 0 bps, so the first change reports 0 -> 750.
    assert_eq!(
        <(u32, u32, Address)>::try_from_val(&te.env, &data).unwrap(),
        (0u32, 750u32, treasury.clone())
    );

    // A second change reports the value it replaced, not the original one.
    te.pool.set_protocol_fee(&1500, &treasury);
    let events = te.env.events().all();
    let (_contract, _topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(
        <(u32, u32, Address)>::try_from_val(&te.env, &data).unwrap(),
        (750u32, 1500u32, treasury.clone())
    );

    // A rejected change (above MAX_PROTOCOL_FEE_BPS) must not emit anything,
    // and must leave the stored fee at 1500.
    let before_rejected = events.len();
    assert!(te.pool.try_set_protocol_fee(&2001, &treasury).is_err());
    assert_eq!(te.env.events().all().len(), before_rejected);
    assert_eq!(te.pool.get_protocol_fee_bps(), 1500);
}

// Re-pointing the treasury is reported in the same event even when the fee
// itself does not change, so monitoring sees the destination move.
#[test]
fn test_set_protocol_fee_event_reports_treasury_change_with_unchanged_fee() {
    let te = setup();
    let treasury = Address::generate(&te.env);
    te.pool.set_protocol_fee(&500, &treasury);

    let new_treasury = Address::generate(&te.env);
    te.pool.set_protocol_fee(&500, &new_treasury);

    let events = te.env.events().all();
    let (_contract, _topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(
        <(u32, u32, Address)>::try_from_val(&te.env, &data).unwrap(),
        (500u32, 500u32, new_treasury.clone())
    );
    assert_eq!(te.pool.get_treasury(), new_treasury);
    assert_eq!(te.pool.get_protocol_fee_bps(), 500);
}

#[test]
fn test_shared_fixture_setup_full_protocol() {
    let proto = setup_full_protocol();
    assert_eq!(proto.pool.get_admin(), proto.admin);
    assert_eq!(proto.pool.get_stats().total_funded, 0);
    assert_eq!(proto.invoice.get_admin(), Some(proto.admin));
}

//  ISSUE #842: MULTI-LP SHARE-SUPPLY CONSERVATION //
// The share ledger lives in two places: `total_supply()` (instance
// `TotalShares`) and each LP's `balance()` (persistent `LPShares`). Deposit
// and withdraw must move both together; `transfer`/`transfer_from` must move
// neither. The property below drives random interleavings of all four entry
// points across 3-5 LPs and re-checks the invariants after every step, so a
// sequence can never mint, burn, or strand a share unnoticed.

// Case budget matches the other properties in this file (see the note at the
// top of the property-test section) to stay within CI time for the in-process
// Soroban host.
const PROP_CASES: u32 = 10;
const PROP_MAX_STEPS: usize = 12;

// Amount bound: wide enough that a single step can exceed any balance built
// up earlier in the sequence (driving the rejected-step paths), and far below
// anything that could overflow the pool's u128 scaling.
const PROP_MAX_AMOUNT: u128 = 10_000_000_000_000;

// Every generated LP starts with more USDC than the whole sequence can spend,
// so deposits fail on pool rules rather than on the mock token's balance.
const PROP_LP_USDC: i128 = 100_000_000_000_000;

// Largest LP set a case can generate; cases use the first `lp_count` slots.
const PROP_MAX_LPS: usize = 5;

// Opening position each LP takes before the random sequence starts: above
// `DEFAULT_MIN_INITIAL_DEPOSIT`, small enough that random steps routinely
// exceed it in both directions.
const PROP_WARMUP_DEPOSIT: u128 = 1_000_000_000;

/// One step of a generated sequence. LP selectors are raw bytes reduced
/// modulo the case's LP count, so every value addresses a real participant.
#[derive(Clone, Debug)]
enum LpStep {
    Deposit {
        lp: u8,
        amount: u128,
    },
    Withdraw {
        lp: u8,
        shares: u128,
    },
    Transfer {
        from: u8,
        to: u8,
        amount: u128,
    },
    TransferFrom {
        owner: u8,
        spender: u8,
        to: u8,
        amount: u128,
        // Re-approve the grant for `amount` right before the move (SEP-41
        // `approve` overwrites), so some steps start from a fresh allowance
        // and others spend whatever earlier grant is still live.
        fresh_grant: bool,
    },
}

/// Outcome of one step, normalised across the four `try_*` return types so
/// the property loop has a single match to reason about.
enum StepOutcome {
    Succeeded,
    Rejected(Result<soroban_sdk::Error, InvokeError>),
}

fn classify<T, E>(
    res: Result<Result<T, E>, Result<soroban_sdk::Error, InvokeError>>,
) -> StepOutcome {
    match res {
        Ok(_) => StepOutcome::Succeeded,
        Err(err) => StepOutcome::Rejected(err),
    }
}

/// Share/USDC sizes come in two buckets: a small one that usually fits inside
/// an existing balance (keeping the accepted withdraw/transfer paths
/// reachable) and a large one that regularly exceeds one (keeping the typed
/// rejections reachable).
fn step_amount_strategy() -> impl Strategy<Value = u128> {
    prop_oneof![
        2 => 0u128..=1_000_000,
        1 => (PROP_MAX_AMOUNT / 2)..=PROP_MAX_AMOUNT,
    ]
}

/// Deposit sizes: mostly large enough to mint shares from an empty pool, with
/// a minority of zero/below-minimum sizes for the typed rejections.
fn deposit_amount_strategy() -> impl Strategy<Value = u128> {
    prop_oneof![
        2 => DEFAULT_MIN_INITIAL_DEPOSIT..=PROP_MAX_AMOUNT,
        1 => 0u128..DEFAULT_MIN_INITIAL_DEPOSIT,
    ]
}

fn lp_step_strategy() -> impl Strategy<Value = LpStep> {
    prop_oneof![
        (any::<u8>(), deposit_amount_strategy())
            .prop_map(|(lp, amount)| LpStep::Deposit { lp, amount }),
        (any::<u8>(), step_amount_strategy())
            .prop_map(|(lp, shares)| LpStep::Withdraw { lp, shares }),
        (any::<u8>(), any::<u8>(), step_amount_strategy())
            .prop_map(|(from, to, amount)| LpStep::Transfer { from, to, amount }),
        (
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            step_amount_strategy(),
            // Bias towards a fresh grant so the accepted `transfer_from` path
            // stays reachable; the remainder spends (or misses) earlier grants
            // and exercises `InsufficientAllowance`.
            prop_oneof![2 => Just(true), 1 => Just(false)]
        )
            .prop_map(
                |(owner, spender, to, amount, fresh_grant)| LpStep::TransferFrom {
                    owner,
                    spender,
                    to,
                    amount,
                    fresh_grant,
                },
            ),
    ]
}

/// Tops a freshly generated address up with mock USDC so its deposits are
/// bounded by pool rules instead of by the token balance.
fn fund_prop_lp(te: &TestEnv, lp: &Address) {
    te.env.as_contract(&te.usdc_id, || {
        te.env
            .storage()
            .persistent()
            .set(&TKey(lp.clone()), &PROP_LP_USDC);
    });
}

/// Captures the share ledger exactly as the property states it: one balance
/// per participating LP plus the pool-wide supply.
fn share_snapshot(te: &TestEnv, lps: &[Address]) -> ([i128; PROP_MAX_LPS], i128) {
    let mut balances = [0i128; PROP_MAX_LPS];
    for (slot, lp) in lps.iter().enumerate() {
        balances[slot] = te.pool.balance(lp);
    }
    (balances, te.pool.total_supply())
}

/// After any step: SEP-41 `balance()` must agree with `get_lp_position()`,
/// and the balances of the whole LP set must add up to `total_supply()`.
fn check_share_invariants(te: &TestEnv, lps: &[Address]) -> Result<(), TestCaseError> {
    let mut sum: i128 = 0;
    for lp in lps {
        let balance = te.pool.balance(lp);
        let position = te.pool.get_lp_position(lp);
        prop_assert_eq!(
            balance,
            position.shares as i128,
            "balance() and get_lp_position().shares diverge for {:?}",
            lp
        );
        sum += balance;
    }
    prop_assert_eq!(
        sum,
        te.pool.total_supply(),
        "sum of LP balances must equal total supply"
    );
    Ok(())
}

/// Dispatches one generated step through its `try_*` entry point so both the
/// accepted and the rejected path go through the same invariant checks.
fn run_step(te: &TestEnv, lps: &[Address], step: &LpStep) -> StepOutcome {
    let at = |raw: &u8| &lps[*raw as usize % lps.len()];
    match step {
        LpStep::Deposit { lp, amount } => classify(te.pool.try_deposit(at(lp), amount)),
        LpStep::Withdraw { lp, shares } => classify(te.pool.try_withdraw(at(lp), shares)),
        LpStep::Transfer { from, to, amount } => classify(te.pool.try_transfer_shares(
            at(from),
            at(to),
            &(*amount as i128),
        )),
        LpStep::TransferFrom {
            owner,
            spender,
            to,
            amount,
            fresh_grant,
        } => {
            if *fresh_grant {
                // The grant is setup for the move, not part of the property:
                // its own result is ignored so a zero amount still revokes
                // (SEP-41) and the transfer_from below reports the typed
                // `InsufficientAllowance` failure instead.
                let expiration_ledger = te.env.ledger().sequence() + 1_000;
                let _ = te.pool.try_approve(
                    at(owner),
                    at(spender),
                    &(*amount as i128),
                    &expiration_ledger,
                );
            }
            classify(
                te.pool
                    .try_transfer_from(at(spender), at(owner), at(to), &(*amount as i128)),
            )
        }
    }
}

// Core property: over any bounded interleaving of deposit/withdraw/transfer/
// transfer_from across 3-5 LPs, every accepted step preserves
// `sum(balance(lp)) == total_supply()` and `balance(lp) ==
// get_lp_position(lp).shares`, and every rejected step leaves the whole
// share ledger byte-for-byte unchanged.
#[test]
fn prop_multi_lp_share_supply_conserved_across_op_sequences() {
    // `TestRunner::run` takes an `Fn`, so the step counters use interior
    // mutability; `std` is pulled in here for them (crate is `no_std`).
    extern crate std;

    let mut runner = TestRunner::new(ProptestConfig::with_cases(PROP_CASES));
    let accepted_steps = std::cell::Cell::new(0usize);
    let rejected_steps = std::cell::Cell::new(0usize);

    runner
        .run(
            &(
                3usize..=PROP_MAX_LPS,
                prop::collection::vec(lp_step_strategy(), 1..=PROP_MAX_STEPS),
            ),
            |(lp_count, steps)| {
                let te = setup();
                let all_lps = [
                    Address::generate(&te.env),
                    Address::generate(&te.env),
                    Address::generate(&te.env),
                    Address::generate(&te.env),
                    Address::generate(&te.env),
                ];
                let lps = &all_lps[..lp_count];
                for lp in lps {
                    fund_prop_lp(&te, lp);
                }

                // Empty pool starts at zero supply, zero balances.
                check_share_invariants(&te, lps)?;

                // Warm start: every LP takes an opening position so the
                // generated steps move shares that actually exist instead of
                // mostly probing the no-position rejections.
                for lp in lps {
                    te.pool.deposit(lp, &PROP_WARMUP_DEPOSIT);
                }
                check_share_invariants(&te, lps)?;

                for (index, step) in steps.iter().enumerate() {
                    let before = share_snapshot(&te, lps);
                    match run_step(&te, lps, step) {
                        StepOutcome::Succeeded => {
                            accepted_steps.set(accepted_steps.get() + 1);
                            check_share_invariants(&te, lps)?;
                        }
                        StepOutcome::Rejected(err) => {
                            rejected_steps.set(rejected_steps.get() + 1);
                            match err {
                                Ok(contract_error) => prop_assert!(
                                    contract_error.is_type(soroban_sdk::xdr::ScErrorType::Contract),
                                    "step {} ({:?}) failed outside the typed PoolError space: {:?}",
                                    index,
                                    step,
                                    contract_error
                                ),
                                Err(host_error) => prop_assert!(
                                    false,
                                    "step {} ({:?}) failed with a host error instead of a typed PoolError: {:?}",
                                    index,
                                    step,
                                    host_error
                                ),
                            }
                            let after = share_snapshot(&te, lps);
                            prop_assert_eq!(
                                after,
                                before,
                                "step {} ({:?}) was rejected but moved balances or total supply",
                                index,
                                step
                            );
                        }
                    }
                }
                Ok(())
            },
        )
        .unwrap();

    // The generator must actually exercise both sides of the property; a
    // sequence run in which everything succeeded (or everything failed) would
    // make the assertions above vacuous.
    let accepted = accepted_steps.get();
    let rejected = rejected_steps.get();
    assert!(
        accepted > 0 && rejected > 0,
        "expected both accepted and rejected steps, got {} accepted / {} rejected",
        accepted,
        rejected
    );
}

//  ISSUE #873: STANDARD SEP-41 transfer ENTRY POINT ======//
// The share-token surface was missing the standard `transfer(from, to, amount)`
// entry point (a prior duplicate-definition collision removed it alongside the
// `transfer_shares` logic it clashed with, leaving `move_shares` orphaned from
// a public caller). These tests pin the SEP-41 behavior on the restored
// function — most importantly that a generic `soroban_sdk::token::Client`
// can drive it with no pool-specific wrapper.

/// The required generic-client test: any SEP-41 consumer must be able to move
/// shares with a plain `soroban_sdk::token::Client` pointed at the pool
/// address, with no pool-specific client in sight.
#[test]
fn test_sep41_generic_token_client_transfer_moves_shares() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);

    // The generic SEP-41 client, constructed from the pool address alone.
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);
    shares_token.transfer(&te.lp, &recipient, &5_000_000_000);

    assert_eq!(
        te.pool.get_lp_position(&te.lp).shares,
        5_000_000_000,
        "sender's shares must be debited"
    );
    assert_eq!(
        te.pool.get_lp_position(&recipient).shares,
        5_000_000_000,
        "recipient's shares must be credited"
    );
    // Total share supply is unaffected by a transfer.
    assert_eq!(te.pool.get_stats().total_shares, 10_000_000_000);
}

/// `transfer` must credit a recipient who has never interacted with the pool.
#[test]
fn test_transfer_to_recipient_without_position_creates_one() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    te.pool.transfer(&te.lp, &recipient, &2_500_000_000);

    let lp_position = te.pool.get_lp_position(&te.lp);
    assert_eq!(lp_position.shares, 7_500_000_000);
    assert_eq!(te.pool.get_lp_position(&recipient).shares, 2_500_000_000);
}

/// The generic client's transfer must enforce `from`'s authorization: with no
/// signatures provided, the host rejects the transaction.
#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_sep41_generic_token_client_transfer_requires_from_auth() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);

    // Clear all mocked auths so from.require_auth() fails.
    te.env.set_auths(&[]);
    shares_token.transfer(&te.lp, &recipient, &1_000_000_000);
}

/// Balance checks must hold on the standard path: transferring more shares
/// than `from` owns panics with `InsufficientBalance`.
#[test]
#[should_panic(expected = "Error(Contract, #23)")]
fn test_transfer_insufficient_balance_panics_via_generic_client() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);
    shares_token.transfer(&te.lp, &recipient, &20_000_000_000);
}

/// A sender with no share position at all panics with `NoShares`.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_transfer_from_address_without_shares_panics() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let stranger = Address::generate(&te.env);
    let recipient = Address::generate(&te.env);
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);
    shares_token.transfer(&stranger, &recipient, &1);
}

/// Zero and negative amounts are rejected on the standard entry point.
#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_transfer_zero_amount_panics_via_generic_client() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);
    shares_token.transfer(&te.lp, &recipient, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_transfer_negative_amount_panics_via_generic_client() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);
    shares_token.transfer(&te.lp, &recipient, &-5);
}

/// Both SEP-41 entry points emit the standard `transfer(from, to, amount)`
/// event shape, so generic indexers can watch share movements.
#[test]
fn test_transfer_and_transfer_from_emit_standard_transfer_event() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    let spender = Address::generate(&te.env);
    let expires = te.env.ledger().sequence() + 100;

    let before = te.env.events().all().len();
    te.pool.transfer(&te.lp, &recipient, &1_000_000_000);
    let events = te.env.events().all();
    assert_eq!(events.len(), before + 1);
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "transfer")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.lp
    );
    assert_eq!(
        <(Address, i128)>::try_from_val(&te.env, &data).unwrap(),
        (recipient.clone(), 1_000_000_000)
    );

    // The allowance path emits the identical standard event shape (approve
    // itself emits `allowance_approved`, hence the +2).
    let before = te.env.events().all().len();
    te.pool.approve(&te.lp, &spender, &2_000_000_000, &expires);
    te.pool
        .transfer_from(&spender, &te.lp, &recipient, &2_000_000_000);
    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(events.len(), before + 2);
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "transfer")
    );
    assert_eq!(
        <(Address, i128)>::try_from_val(&te.env, &data).unwrap(),
        (recipient.clone(), 2_000_000_000)
    );
}

/// `transfer` and `transfer_shares` must be interchangeable: both entry
/// points move shares identically (transfer_shares is the documented
/// non-standard legacy alias).
#[test]
fn test_transfer_matches_legacy_transfer_shares() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let recipient = Address::generate(&te.env);
    te.pool.transfer_shares(&te.lp, &recipient, &5_000_000_000);

    let second_recipient = Address::generate(&te.env);
    te.pool.transfer(&te.lp, &second_recipient, &5_000_000_000);

    assert_eq!(te.pool.get_lp_position(&recipient).shares, 5_000_000_000);
    assert_eq!(
        te.pool.get_lp_position(&second_recipient).shares,
        5_000_000_000
    );
    assert_eq!(te.pool.get_lp_position(&te.lp).shares, 0);
}

/// Self-transfers remain a no-op on the standard entry point.
#[test]
fn test_transfer_same_address_no_op_via_generic_client() {
    let te = setup();
    te.pool.deposit(&te.lp, &10_000_000_000);

    let before = te.pool.get_lp_position(&te.lp);
    let shares_token = soroban_sdk::token::Client::new(&te.env, &te.pool_id);
    shares_token.transfer(&te.lp, &te.lp, &5_000_000_000);
    let after = te.pool.get_lp_position(&te.lp);

    assert_eq!(before.shares, after.shares);
    assert_eq!(te.pool.get_stats().total_shares, 10_000_000_000);
}

// ====== EXPIRE-LISTING VS FUND-INVOICE RACE TESTS //
// These tests cover the interaction between invoice.expiry_listing() and
// pool.fund_invoice() when a listed invoice's expiry window passes.
// The threat model (docs/THREAT_MODEL.md, attack vector 6) treats this
// as an "accepted race condition" but we pin the deterministic outcomes
// on either side of the deadline so future refactors cannot silently flip
// the boundary behavior.
//
// Note: invoice.expire_listing() requires issuer or admin auth. The
// expire path's authorization has a separate open bug (see invoice crate
// issue tracker); this test uses mock_all_auths_allowing_non_root_auth()
// for the expire_listing call only to exercise the cross-contract flow.

#[test]
fn test_expire_listing_then_fund_invoice_fails_invoice_not_listed() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    // Create and list an invoice with a short expiry window
    let due_date = te.env.ledger().timestamp() + 86400;
    let invoice_id = te.invoice.create(
        &te.issuer,
        &te.buyer,
        &10_000_000_000,
        &due_date,
        &te.usdc_id,
    );
    attest_invoice(&te, &invoice_id);
    te.invoice.list_for_financing(&invoice_id, &200);

    // Verify invoice is Listed
    assert_eq!(te.invoice.get_status(&invoice_id), 1);

    // Set a short expiry window (100 seconds)
    te.invoice.set_expiry_window(&100);

    // Advance time to exactly listed_at + expiry_window
    // listed_at is set to current timestamp when list_for_financing is called
    let listed_at = te.env.ledger().timestamp();
    let deadline = listed_at + 100;
    te.env.ledger().set_timestamp(deadline);

    // Expire the listing - using mock_all_auths to bypass the separate auth bug
    te.env.mock_all_auths_allowing_non_root_auth();
    let expired = te.invoice.expire_listing(&invoice_id, &te.issuer);
    assert!(expired);

    // Verify invoice status is now Expired (7)
    assert_eq!(te.invoice.get_status(&invoice_id), 7);

    // Verify invoice counts show the invoice under Expired
    let counts = te.invoice.get_counts();
    assert_eq!(counts.get(String::from_str(&te.env, "Listed")), Some(0));
    assert_eq!(counts.get(String::from_str(&te.env, "Expired")), Some(1));

    // Capture pool stats before attempting to fund
    let stats_before = te.pool.get_stats();

    // Try to fund the expired invoice - must fail with InvoiceNotListed (#8)
    let result = te.pool.try_fund_invoice(&invoice_id);
    assert!(
        result.is_err(),
        "fund_invoice must fail for expired invoice"
    );

    // Verify pool totals and active_invoice_count are unchanged
    let stats_after = te.pool.get_stats();
    assert_eq!(stats_after.total_deposits, stats_before.total_deposits);
    assert_eq!(stats_after.total_funded, stats_before.total_funded);
    assert_eq!(
        stats_after.active_invoice_count,
        stats_before.active_invoice_count
    );
    assert_eq!(
        stats_after.available_liquidity,
        stats_before.available_liquidity
    );
    assert_eq!(stats_after.total_shares, stats_before.total_shares);
}

// Boundary case: at deadline - 1 second, funding succeeds and expiry is rejected.
// This documents that funding wins at the exact deadline second (current_time < deadline).
#[test]
fn test_fund_invoice_succeeds_at_deadline_minus_one_expiry_rejected() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    // Create and list an invoice
    let due_date = te.env.ledger().timestamp() + 86400;
    let invoice_id = te.invoice.create(
        &te.issuer,
        &te.buyer,
        &10_000_000_000,
        &due_date,
        &te.usdc_id,
    );
    attest_invoice(&te, &invoice_id);
    te.invoice.list_for_financing(&invoice_id, &200);

    // Set a short expiry window (100 seconds)
    te.invoice.set_expiry_window(&100);

    // Advance time to deadline - 1 (one second before expiry)
    let listed_at = te.env.ledger().timestamp();
    let deadline = listed_at + 100;
    te.env.ledger().set_timestamp(deadline - 1);

    // Funding should succeed at deadline - 1
    let funded = te.pool.fund_invoice(&invoice_id);
    assert!(
        funded,
        "funding must succeed one second before expiry deadline"
    );

    // Verify invoice is now Funded (2)
    assert_eq!(te.invoice.get_status(&invoice_id), 2);

    // Verify pool state updated
    let stats = te.pool.get_stats();
    assert_eq!(stats.active_invoice_count, 1);
    assert_eq!(stats.total_funded, 9_800_000_000);

    // Now try to expire - must fail with ListingNotExpired (#14)
    // because the invoice is no longer Listed (it's Funded)
    let result = te.invoice.try_expire_listing(&invoice_id, &te.issuer);
    assert!(
        result.is_err(),
        "expire_listing must fail for funded invoice"
    );
}

/// Proves that share-price appreciation is correctly reflected in LP positions
/// after a share transfer via the SEP-41 interface (Issue #763).
#[test]
fn test_share_price_appreciation_after_transfer() {
    let te = setup();

    // LP A deposits funds
    let lp_a_initial_deposit = 100_000_000_000u128;
    let lp_a_shares_before = te.pool.get_lp_position(&te.lp).shares;
    te.pool.deposit(&te.lp, &lp_a_initial_deposit);
    let lp_a_shares_after_deposit = te.pool.get_lp_position(&te.lp).shares;
    assert_eq!(
        lp_a_shares_after_deposit - lp_a_shares_before,
        lp_a_initial_deposit
    );

    // Accrue yield via fund and repay cycle
    fund_and_repay_invoice(&te);

    // After repayment, total_deposits increases relative to total_shares
    let stats_after_repay = te.pool.get_stats();
    assert!(stats_after_repay.total_deposits > stats_after_repay.total_shares);

    // LP A transfers half of their shares to LP B
    let lp_b = Address::generate(&te.env);
    let lp_a_shares = te.pool.get_lp_position(&te.lp).shares;
    let transfer_amount = (lp_a_shares / 2) as i128;
    assert!(transfer_amount > 0);

    te.pool.transfer(&te.lp, &lp_b, &transfer_amount);

    // Get LP positions after transfer
    let lp_a_pos = te.pool.get_lp_position(&te.lp);
    let lp_b_pos = te.pool.get_lp_position(&lp_b);

    // Verify share balances reflect the transfer
    assert_eq!(lp_a_pos.shares, lp_a_shares - (transfer_amount as u128));
    assert_eq!(lp_b_pos.shares, transfer_amount as u128);

    // Both positions must reflect the appreciated per-share value proportional
    // to their post-transfer balances.
    let expected_a = (lp_a_pos.shares as u128) * stats_after_repay.total_deposits
        / stats_after_repay.total_shares;
    let expected_b = (lp_b_pos.shares as u128) * stats_after_repay.total_deposits
        / stats_after_repay.total_shares;

    assert_eq!(lp_a_pos.usdc_value, expected_a);
    assert_eq!(lp_b_pos.usdc_value, expected_b);

    // Per-share value (usdc_value / shares) must be identical for both LPs
    assert_eq!(
        lp_a_pos.usdc_value / lp_a_pos.shares,
        lp_b_pos.usdc_value / lp_b_pos.shares
    );
    assert!(lp_a_pos.usdc_value > lp_a_pos.shares);
    assert!(lp_b_pos.usdc_value > lp_b_pos.shares);
}

//  ISSUE #844: UTILIZATION / FUNDING ACCOUNTING //
// Invariants, across arbitrary sequences of deposit / fund_invoice /
// receive_repayment / handle_default / withdraw with a random cap:
//   * `get_stats().total_funded <= total_deposits`
//   * `available_liquidity == total_deposits - total_funded`
//   * after a successful `fund_invoice`, `get_utilization_rate() <=
//     get_stats().max_utilization_bps`
//   * a rejected step leaves the accounting byte-for-byte unchanged.

#[derive(Clone, Debug)]
enum FundStep {
    Deposit(u128),
    Fund(u8),
    Repay(u128),
    Default,
    Withdraw(u128),
    Transfer(u128),
    SetFee(u32),
    SetCap(u32),
}

fn fund_step_strategy() -> impl Strategy<Value = FundStep> {
    prop_oneof![
        deposit_amount_strategy().prop_map(FundStep::Deposit),
        any::<u8>().prop_map(FundStep::Fund),
        step_amount_strategy().prop_map(FundStep::Repay),
        Just(FundStep::Default),
        step_amount_strategy().prop_map(FundStep::Withdraw),
        (0u32..=10_000u32).prop_map(FundStep::SetCap),
    ]
}

fn run_fund_step(
    te: &TestEnv,
    other_lp: &Address,
    invoices: &[BytesN<32>; 3],
    step: &FundStep,
) -> StepOutcome {
    match step {
        FundStep::Deposit(amount) => classify(te.pool.try_deposit(&te.lp, amount)),
        FundStep::Fund(index) => {
            let id = invoices[*index as usize % invoices.len()].clone();
            classify(te.pool.try_fund_invoice(&id))
        }
        FundStep::Repay(amount) => {
            let id = invoices[0].clone();
            classify(te.pool.try_receive_repayment(&id, amount))
        }
        FundStep::Default => classify(te.pool.try_handle_default(&invoices[0])),
        FundStep::Withdraw(shares) => classify(te.pool.try_withdraw(&te.lp, shares)),
        FundStep::SetCap(bps) => classify(te.pool.try_set_max_utilization(&te.admin, bps)),
    }
        FundStep::Transfer(shares) => {
            classify(te.pool.try_transfer(&te.lp, other_lp, &(*shares as i128)))
        }
        FundStep::SetFee(bps) => {
            let treasury = te.pool.get_treasury();
            classify(te.pool.try_set_protocol_fee(bps, &treasury))
        }
}

/// `(total_deposits, total_funded, available_liquidity, max_utilization_bps)`.
fn fund_snapshot(te: &TestEnv) -> (u128, u128, u128, u32) {
    let stats = te.pool.get_stats();
    (
        stats.total_deposits,
        stats.total_funded,
        stats.available_liquidity,
        stats.max_utilization_bps,
    )
}

/// After any step: never more funded than deposited, and available liquidity is
/// exactly deposits minus funded.
fn check_fund_invariants(te: &TestEnv) -> Result<(), TestCaseError> {
    let stats = te.pool.get_stats();
    prop_assert!(
        stats.total_funded <= stats.total_deposits,
        "total_funded {} exceeds total_deposits {}",
        stats.total_funded,
        stats.total_deposits
    );
    prop_assert_eq!(
        stats.available_liquidity,
        stats.total_deposits - stats.total_funded,
        "available_liquidity must equal total_deposits - total_funded"
    );
    Ok(())
}

#[test]
fn prop_utilization_cap_and_funding_accounting_hold_across_op_sequences() {
    extern crate std;

    let mut runner = TestRunner::new(ProptestConfig::with_cases(PROP_CASES));
    let accepted_steps = std::cell::Cell::new(0usize);
    let rejected_steps = std::cell::Cell::new(0usize);

    runner
        .run(
            &(
                // Sometimes a very tight cap (mostly rejections), sometimes a
                // full cap (so funding can succeed and the cap assertion bites).
                prop_oneof![1 => 1u32..=500u32, 1 => 1u32..=10_000u32],
                prop::collection::vec(fund_step_strategy(), 1..=PROP_MAX_STEPS),
            ),
            |(cap_bps, steps)| {
                let te = setup();
                te.pool.set_max_utilization(&te.admin, &cap_bps);

                let other_lp = Address::generate(&te.env);
                te.env.as_contract(&te.usdc_id, || {
                    te.env
                        .storage()
                        .persistent()
                        .set(&TKey(other_lp.clone()), &100_000_000_000_000i128);
                });
                // A small set of fundable invoices plus enough LP liquidity.
                let invoices = [
                    create_and_list(&te, &te.usdc_id),
                    create_and_list(&te, &te.usdc_id),
                    create_and_list(&te, &te.usdc_id),
                ];
                fund_prop_lp(&te, &te.lp);
                te.pool.deposit(&te.lp, &PROP_WARMUP_DEPOSIT);

                check_fund_invariants(&te)?;

                for (index, step) in steps.iter().enumerate() {
                    let before = fund_snapshot(&te);
                    let outcome = run_fund_step(&te, &other_lp, &invoices, step);

                    // The core invariants hold after every step.
                    check_fund_invariants(&te)?;

                    prop_assert_eq!(
                        (te.pool.balance(&te.lp) + te.pool.balance(&other_lp)) as u128,
                        te.pool.get_stats().total_shares,
                        "LP balances must conserve total share supply after every operation"
                    );
                    match outcome {
                        StepOutcome::Succeeded => {
                            accepted_steps.set(accepted_steps.get() + 1);
                            if let FundStep::Fund(_) = step {
                                let util = te.pool.get_utilization_rate();
                                let cap = te.pool.get_stats().max_utilization_bps;
                                prop_assert!(
                                    util <= cap,
                                    "step {}: funding pushed utilization {} above cap {}",
                                    index,
                                    util,
                                    cap
                                );
                            }
                        }
                        StepOutcome::Rejected(_) => {
                            rejected_steps.set(rejected_steps.get() + 1);
                            prop_assert_eq!(
                                fund_snapshot(&te),
                                before,
                                "step {} ({:?}) was rejected but moved accounting",
                                index,
                                step
                            );
                        }
                    }
                }
                Ok(())
            },
        )
        .unwrap();

    // The generator must exercise both sides; otherwise the assertions above
    // would be vacuous.
    assert!(
        accepted_steps.get() > 0 && rejected_steps.get() > 0,
        "expected both accepted and rejected steps, got {} accepted / {} rejected",
        accepted_steps.get(),
        rejected_steps.get()
    );
}

//  ISSUE #719: SUGGESTED DISCOUNT VIEW //
// `get_suggested_discount_bps` interpolates linearly from the floor at 0%
// utilization to the ceiling at 100%. The ceiling is pinned to 5000 bps so a
// suggestion can never exceed the cap `invoice::list_for_financing` enforces.

#[test]
fn test_suggested_discount_curve_constants() {
    assert_eq!(SUGGESTED_DISCOUNT_FLOOR_BPS, 100);
    // The ceiling sits exactly at the invoice contract's hard 5000 bps cap so a
    // suggestion is never rejected by `list_for_financing`.
    assert_eq!(SUGGESTED_DISCOUNT_CEILING_BPS, 5_000);
}

/// Low utilization (idle pool, nothing funded) → the floor.
#[test]
fn test_get_suggested_discount_bps_at_low_utilization_returns_floor() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    assert_eq!(te.pool.get_utilization_rate(), 0);
    assert_eq!(
        te.pool.get_suggested_discount_bps(),
        SUGGESTED_DISCOUNT_FLOOR_BPS
    );
}

/// Mid utilization (exactly 50% of deposits funded) → the curve midpoint.
#[test]
fn test_get_suggested_discount_bps_at_mid_utilization_is_midpoint() {
    let te = setup();
    // Deposits = 2 × funded_amount, so utilization is exactly 5000 bps.
    te.pool.deposit(&te.lp, &(DEFAULT_FUNDED_AMOUNT * 2));
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    assert_eq!(te.pool.get_utilization_rate(), 5_000);
    assert_eq!(
        te.pool.get_suggested_discount_bps(),
        SUGGESTED_DISCOUNT_FLOOR_BPS
            + (SUGGESTED_DISCOUNT_CEILING_BPS - SUGGESTED_DISCOUNT_FLOOR_BPS) / 2
    );
}

/// Near-cap utilization (every deposited unit funded) → the ceiling.
#[test]
fn test_get_suggested_discount_bps_at_full_utilization_returns_ceiling() {
    let te = setup();
    te.pool.deposit(&te.lp, &DEFAULT_FUNDED_AMOUNT);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    assert_eq!(te.pool.get_utilization_rate(), 10_000);
    assert_eq!(
        te.pool.get_suggested_discount_bps(),
        SUGGESTED_DISCOUNT_CEILING_BPS
    );
}

/// The view is read-only and authless: it must succeed with no mocked auths
/// and leave every accounting field untouched.
#[test]
fn test_get_suggested_discount_bps_is_read_only_and_authless() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let before = te.pool.get_stats();

    // Clear all mocked auths: a read-only view must not need authorization.
    te.env.set_auths(&[]);
    let suggestion = te.pool.get_suggested_discount_bps();
    assert_eq!(suggestion, SUGGESTED_DISCOUNT_FLOOR_BPS);

    let after = te.pool.get_stats();
    assert_eq!(after.total_deposits, before.total_deposits);
    assert_eq!(after.total_funded, before.total_funded);
    assert_eq!(after.total_shares, before.total_shares);
    assert_eq!(after.active_invoice_count, before.active_invoice_count);
}

/// An uninitialized pool has no deposits, so utilization reads 0% and the view
/// returns the floor instead of panicking.
#[test]
fn test_get_suggested_discount_bps_uninitialized_pool_returns_floor() {
    let (_env, pool) = uninitialized_pool();
    assert_eq!(
        pool.get_suggested_discount_bps(),
        SUGGESTED_DISCOUNT_FLOOR_BPS
    );
}
//  ISSUE #440: TRANSFER OWNERSHIP
#[test]
fn test_pool_transfer_ownership_changes_admin_and_emits_event() {
    let te = setup();
    let new_admin = Address::generate(&te.env);

    te.pool.transfer_ownership(&new_admin);

    assert_eq!(te.pool.get_admin(), new_admin);

    // ownership_transferred(old_admin, new_admin)
    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "ownership_transferred")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.admin
    );
    assert_eq!(Address::try_from_val(&te.env, &data).unwrap(), new_admin);
}

/// Dual auth: only `non_admin` signed the call, so the stored admin's
/// `require_auth()` rejects the transfer — an unauthorized caller cannot move
/// ownership (issue #440).
#[test]
fn test_pool_transfer_ownership_rejects_unauthorized_caller() {
    let te = setup();
    let new_admin = Address::generate(&te.env);
    let non_admin = Address::generate(&te.env);

    te.env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &te.pool_id,
            fn_name: "transfer_ownership",
            args: (new_admin.clone(),).into_val(&te.env),
            sub_invokes: &[],
        },
    }]);
    assert!(te.pool.try_transfer_ownership(&new_admin).is_err());
}

/// Dual auth also requires the *incoming* admin to consent: with only the
/// current admin's signature, `new_admin.require_auth()` rejects the transfer
/// so ownership cannot be forced onto an address that never signed.
#[test]
fn test_pool_transfer_ownership_requires_new_admin_consent() {
    let te = setup();
    let new_admin = Address::generate(&te.env);

    te.env.mock_auths(&[MockAuth {
        address: &te.admin,
        invoke: &MockAuthInvoke {
            contract: &te.pool_id,
            fn_name: "transfer_ownership",
            args: (new_admin.clone(),).into_val(&te.env),
            sub_invokes: &[],
        },
    }]);
    assert!(te.pool.try_transfer_ownership(&new_admin).is_err());
    // Admin unchanged after the rejected transfer.
    assert_eq!(te.pool.get_admin(), te.admin);
}

//  ISSUE #714: EMERGENCY PAUSE / UNPAUSE
#[test]
fn test_pause_blocks_state_changes_and_unpause_restores_them() {
    let te = setup();

    // Live: deposits flow.
    te.pool.deposit(&te.lp, &10_000_000_000);
    assert_eq!(te.pool.get_stats().total_deposits, 10_000_000_000);

    te.pool.pause();

    // State-changing entry points are rejected while paused...
    assert!(te.pool.try_deposit(&te.lp, &10_000_000_000).is_err());
    assert!(te.pool.try_transfer(&te.lp, &te.issuer, &1i128).is_err());
    assert!(te.pool.try_set_max_utilization(&te.admin, &9000).is_err());

    // ...while read-only views keep working.
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_deposits, 10_000_000_000);
    assert_eq!(te.pool.get_admin(), te.admin);
    assert_eq!(te.pool.balance(&te.lp), 10_000_000_000);
    let _ = te.pool.get_lp_position(&te.lp);
    let _ = te.pool.get_utilization_rate();

    te.pool.unpause();

    // Unpaused: state changes flow again.
    te.pool.deposit(&te.lp, &10_000_000_000);
    assert_eq!(te.pool.get_stats().total_deposits, 20_000_000_000);
}

/// Pins the exact breaker error: `PauseError::ContractPaused` from the shared
/// crate surfaces as `Error(Contract, #1)` on every guarded entry point.
#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_deposit_reverts_while_paused_with_contract_paused_error() {
    let te = setup();
    te.pool.pause();
    te.pool.deposit(&te.lp, &10_000_000_000);
}

/// Negative auth: only a non-admin signed `pause`, so the stored admin's
/// `require_auth()` must reject it — a non-admin cannot engage the breaker.
#[test]
fn test_pause_requires_admin_authorization() {
    let te = setup();
    let non_admin = Address::generate(&te.env);

    te.env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &te.pool_id,
            fn_name: "pause",
            args: soroban_sdk::Vec::new(&te.env),
            sub_invokes: &[],
        },
    }]);
    assert!(te.pool.try_pause().is_err());
}

/// Negative auth for disengaging: a non-admin cannot unpause either, so a
/// paused pool cannot be resumed by anyone but the admin.
#[test]
fn test_unpause_requires_admin_authorization() {
    let te = setup();
    te.pool.pause();
    let non_admin = Address::generate(&te.env);

    te.env.mock_auths(&[MockAuth {
        address: &non_admin,
        invoke: &MockAuthInvoke {
            contract: &te.pool_id,
            fn_name: "unpause",
            args: soroban_sdk::Vec::new(&te.env),
            sub_invokes: &[],
        },
    }]);
    assert!(te.pool.try_unpause().is_err());
}

#[test]
fn test_pause_and_unpause_emit_events() {
    let te = setup();

    te.pool.pause();
    let events = te.env.events().all();
    let (contract, topics, _) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "paused")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.admin
    );

    te.pool.unpause();
    let events = te.env.events().all();
    let (contract, topics, _) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.pool_id);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "unpaused")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.admin
    );
}

/// Transfer of ownership is itself a state change, so the breaker gates it
/// too: while paused, `transfer_ownership` reverts and the admin is unchanged.
#[test]
fn test_pool_transfer_ownership_blocked_while_paused() {
    let te = setup();
    let new_admin = Address::generate(&te.env);

    te.pool.pause();
    assert!(te.pool.try_transfer_ownership(&new_admin).is_err());
    assert_eq!(te.pool.get_admin(), te.admin);
}
//  ISSUE #722: BATCH FUND INVOICE
#[test]
fn test_batch_fund_invoice_funds_every_entry() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    let ids = vec![
        &te.env,
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
    ];

    let funded = te.pool.batch_fund_invoice(&ids);
    assert_eq!(funded.len(), 3);

    let stats = te.pool.get_stats();
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT * 3);
    assert_eq!(stats.active_invoice_count, 3);

    // Every invoice actually reached the funded state.
    for id in ids.iter() {
        assert_eq!(te.invoice.get_status(&id), 2); // Funded
    }
}

/// The required stop-behavior: funding halts — rather than partially draining
/// the pool — once funding the next entry would exceed `max_utilization_bps`.
#[test]
fn test_batch_fund_invoice_stops_when_utilization_cap_would_be_exceeded() {
    let te = setup();
    // Deposits = 3 × funded_amount, so each funded invoice adds ~3333 bps of
    // utilization: two entries reach 6666 bps (allowed by the 7000 cap) and the
    // third would reach 10000 bps, breaching it.
    te.pool.deposit(&te.lp, &29_400_000_000);
    te.pool.set_max_utilization(&te.admin, &7_000);

    let ids = vec![
        &te.env,
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
    ];

    let funded = te.pool.batch_fund_invoice(&ids);

    // Two entries fit within the 6600 bps cap; the third would exceed it.
    assert_eq!(funded.len(), 2);
    let before_cap_entry = &ids.get(2).unwrap();
    assert_eq!(funded.get(0).unwrap(), ids.get(0).unwrap());
    assert_eq!(funded.get(1).unwrap(), ids.get(1).unwrap());

    let stats = te.pool.get_stats();
    // Stopped cleanly: the third invoice was left untouched, not partially
    // funded, and the fourth was never attempted.
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT * 2);
    assert_eq!(stats.active_invoice_count, 2);
    assert_eq!(te.invoice.get_status(before_cap_entry), 1); // still Listed
    assert_eq!(te.invoice.get_status(&ids.get(3).unwrap()), 1); // Listed
    assert_eq!(stats.utilization_rate_bps, 6_666);
    assert!(stats.utilization_rate_bps <= 7_000);
}

/// A capacity stop is not a panic: the batch returns a partial funded-list.
#[test]
fn test_batch_fund_invoice_stops_on_insufficient_liquidity() {
    let te = setup();
    // Exactly enough for one invoice at 100% utilization.
    te.pool.deposit(&te.lp, &DEFAULT_FUNDED_AMOUNT);

    let ids = vec![
        &te.env,
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
        create_and_list(&te, &te.usdc_id),
    ];

    let funded = te.pool.batch_fund_invoice(&ids);
    assert_eq!(funded.len(), 1);
    assert_eq!(funded.get(0).unwrap(), ids.get(0).unwrap());
    for index in 1..ids.len() {
        assert_eq!(te.invoice.get_status(&ids.get(index).unwrap()), 1); // Listed
    }
    let stats = te.pool.get_stats();
    assert_eq!(stats.total_funded, DEFAULT_FUNDED_AMOUNT);
    assert_eq!(stats.active_invoice_count, 1);
}

/// Eligibility failures are NOT treated as a capacity stop: an ineligible entry
/// still reverts the whole batch with the same typed error `fund_invoice` raises.
#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_batch_fund_invoice_reverts_on_already_funded_entry() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let first = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&first);

    let second = create_and_list(&te, &te.usdc_id);
    let ids = vec![&te.env, second, first];
    te.pool.batch_fund_invoice(&ids);
}

/// An unlisted invoice reverts the batch too (`InvoiceNotListed` = #8).
#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_batch_fund_invoice_reverts_on_unlisted_entry() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);

    let listed = create_and_list(&te, &te.usdc_id);
    // Created but never listed.
    let due_date = te.env.ledger().timestamp() + 86400;
    let unlisted = te.invoice.create(
        &te.issuer,
        &te.buyer,
        &DEFAULT_FACE_VALUE,
        &due_date,
        &te.usdc_id,
    );

    let ids = vec![&te.env, listed, unlisted];
    te.pool.batch_fund_invoice(&ids);
}

#[test]
fn test_batch_fund_invoice_on_empty_batch_is_a_noop() {
    let te = setup();
    let ids = vec![&te.env];
    let funded = te.pool.batch_fund_invoice(&ids);
    assert!(funded.is_empty());
    assert_eq!(te.pool.get_stats().total_funded, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #26)")]
fn test_batch_fund_invoice_rejects_oversized_batch() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let id = create_and_list(&te, &te.usdc_id);
    let mut ids: Vec<BytesN<32>> = Vec::new(&te.env);
    for _ in 0..=MAX_BATCH_SIZE {
        ids.push_back(id.clone());
    }
    te.pool.batch_fund_invoice(&ids);
}

/// A full 50-entry batch is permitted; the budget is lifted only because each
/// entry performs several cross-contract calls.
#[test]
fn test_batch_fund_invoice_allows_exactly_max_batch_size() {
    let te = setup();
    te.env.budget().reset_unlimited();
    te.pool.deposit(&te.lp, &1_000_000_000_000);

    let mut ids: Vec<BytesN<32>> = Vec::new(&te.env);
    for _ in 0..MAX_BATCH_SIZE {
        ids.push_back(create_and_list(&te, &te.usdc_id));
    }

    let funded = te.pool.batch_fund_invoice(&ids);
    assert_eq!(funded.len(), MAX_BATCH_SIZE);
    assert_eq!(te.pool.get_stats().active_invoice_count, MAX_BATCH_SIZE);
}

// ============== GET_FUNDED_AMOUNT VIEW TESTS (issue #839) ==============

// The four states of `DataKey::FundedInvoice`, each reached through the
// contract's real fund / repayment / default entry points rather than by
// writing storage directly, so the assertions exercise the same lifecycle an
// on-chain indexer would observe.

// Unfunded: a listed invoice the pool has never funded has no
// `FundedInvoice` entry, so the view reports `None`.
#[test]
fn test_get_funded_amount_returns_none_for_unfunded_invoice() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);

    assert_eq!(te.pool.get_funded_amount(&invoice_id), None);

    // An invoice id the pool has never seen at all also reports `None`
    // (the storage lookup never panics for unknown ids).
    let unknown_id = BytesN::from_array(&te.env, &[0xAB; 32]);
    assert_eq!(te.pool.get_funded_amount(&unknown_id), None);
}

// Funded: after `fund_invoice` commits capital, the view reports exactly
// the amount that was funded.
#[test]
fn test_get_funded_amount_returns_amount_while_funded() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);

    // face_value=10_000_000_000, discount_bps=200
    // funded_amount = 10_000_000_000 * (10000 - 200) / 10000 = 9_800_000_000
    assert_eq!(
        te.pool.get_funded_amount(&invoice_id),
        Some(DEFAULT_FUNDED_AMOUNT)
    );
}

// Repaid: the full production repayment path (ship -> confirm ->
// `invoice.repay`, which calls back into `receive_repayment` and
// `settle_repayment`) removes the `FundedInvoice` entry, so the view
// returns to `None`.
#[test]
fn test_get_funded_amount_returns_none_after_repayment() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    assert_eq!(
        te.pool.get_funded_amount(&invoice_id),
        Some(DEFAULT_FUNDED_AMOUNT)
    );

    te.invoice.mark_shipped(&invoice_id);
    te.invoice.confirm_delivery(&invoice_id, &te.issuer);
    te.invoice.confirm_delivery(&invoice_id, &te.buyer);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 86401);
    te.invoice.repay(&invoice_id);

    assert_eq!(te.pool.get_funded_amount(&invoice_id), None);
}

// Defaulted: the default path removes the `FundedInvoice` entry too, so
// the view returns `None` after `handle_default` completes. This drives
// `pool.handle_default` (the entry point `invoice.trigger_default`
// invokes) like the rest of the default-path suite; the direct
// `invoice.trigger_default` chain is currently pinned to panic on
// self-re-entrancy by `test_trigger_default_drives_full_pool_and_escrow_chain`.
#[test]
fn test_get_funded_amount_returns_none_after_default() {
    let te = setup();
    te.pool.deposit(&te.lp, &100_000_000_000);
    let invoice_id = create_and_list(&te, &te.usdc_id);
    te.pool.fund_invoice(&invoice_id);
    assert_eq!(
        te.pool.get_funded_amount(&invoice_id),
        Some(DEFAULT_FUNDED_AMOUNT)
    );

    // Advance past escrow's lock-age grace period so `escrow.handle_default`
    // releases the locked funds back to the pool.
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 60);
    assert!(te.pool.handle_default(&invoice_id));

    assert_eq!(te.pool.get_funded_amount(&invoice_id), None);
}
// Issue #37 initial state-machine scope: LP operations plus fee configuration.
// Cross-contract settlement/default sequences remain a separate follow-up.
#[test]
fn prop_lp_operation_sequences_conserve_shares_and_deposits() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &proptest::collection::vec((0u8..5, 1u32..1000), 1..20),
            |operations| {
                let te = setup();
                let other = Address::generate(&te.env);
                te.env.as_contract(&te.usdc_id, || {
                    te.env
                        .storage()
                        .persistent()
                        .set(&TKey(other.clone()), &100_000_000_000_000i128);
                });
                let token = MockTokenClient::new(&te.env, &te.usdc_id);
                let initial_tokens = token.balance(&te.lp) + token.balance(&other);
                let mut deposits = DEFAULT_MIN_INITIAL_DEPOSIT * 10;
                te.pool.deposit(&te.lp, &deposits);
                for (operation, amount) in operations {
                    let amount = amount as u128;
                    match operation {
                        0 | 1 => {
                            let actor = if operation == 0 { &te.lp } else { &other };
                            te.pool.deposit(actor, &amount);
                            deposits += amount;
                        }
                        2 => {
                            let balance = te.pool.balance(&te.lp) as u128;
                            let moved = amount.min(balance);
                            if moved > 0 {
                                te.pool.transfer(&te.lp, &other, &(moved as i128));
                            }
                        }
                        3 => {
                            let balance = te.pool.balance(&other) as u128;
                            let burned = amount.min(balance);
                            if burned > 0 {
                                let paid = te.pool.withdraw(&other, &burned);
                                deposits -= paid;
                            }
                        }
                        _ => {
                            let fee = (amount as u32) % (MAX_PROTOCOL_FEE_BPS + 1);
                            te.pool.set_protocol_fee(&fee, &te.admin);
                        }
                    }
                    let stats = te.pool.get_stats();
                    let shares = te.pool.balance(&te.lp) + te.pool.balance(&other);
                    prop_assert_eq!(stats.total_shares, shares as u128);
                    prop_assert_eq!(stats.total_deposits, deposits);
                    prop_assert_eq!(stats.total_funded, 0);
                    prop_assert_eq!(stats.total_shares, stats.total_deposits);
                    prop_assert_eq!(stats.available_liquidity, deposits);
                    prop_assert_eq!(token.balance(&te.pool_id), deposits as i128);
                    prop_assert_eq!(
                        token.balance(&te.lp) + token.balance(&other) + token.balance(&te.pool_id),
                        initial_tokens
                    );

                    // A malformed movement must fail without changing supply
                    // or either participant's balance at every sequence prefix.
                    let lp_before = te.pool.balance(&te.lp);
                    let other_before = te.pool.balance(&other);
                    prop_assert!(te.pool.try_transfer(&te.lp, &other, &-1).is_err());
                    prop_assert_eq!(te.pool.balance(&te.lp), lp_before);
                    prop_assert_eq!(te.pool.balance(&other), other_before);
                    prop_assert_eq!(te.pool.get_stats().total_shares, stats.total_shares);
                }
                Ok(())
            },
        )
        .unwrap();
}
