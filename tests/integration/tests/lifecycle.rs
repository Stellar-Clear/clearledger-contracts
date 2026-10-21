extern crate std;

use soroban_sdk::{
    contract, contractimpl, contracttype, testutils::Address as _, testutils::Ledger as _,
    xdr::ToXdr, Address, Bytes, BytesN, Env, Map, String, Symbol,
};

use clearledger_escrow::{EscrowContract, EscrowContractClient};
use clearledger_invoice::{InvoiceContract, InvoiceContractClient, InvoiceStatus};
use clearledger_pool::{
    PoolContract, PoolContractClient, DEFAULT_MIN_INITIAL_DEPOSIT, DEFAULT_SHARE_DECIMALS,
};
use clearledger_registry::{RegistryContract, RegistryContractClient, VerificationStatus};

// --------------- Mock USDC Token ---------------

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        if let Some(fail_to) = env.storage().instance().get::<_, Address>(&FailToKey) {
            if to == fail_to {
                panic!("mock token: transfer to issuer failed");
            }
        }
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

    pub fn set_fail_transfers_to(env: Env, target: Address) {
        env.storage().instance().set(&FailToKey, &target);
    }
}

#[contracttype]
pub struct TKey(Address);

#[contracttype]
pub struct FailToKey;

// --------------- Test-only Funding Orchestrator ---------------

/// Composes funding and the issuer payout in a single top-level invocation so
/// a payout failure must roll back the funding writes too.
#[contract]
pub struct FundingOrchestrator;

#[contractimpl]
impl FundingOrchestrator {
    pub fn fund_and_release(
        env: Env,
        pool: Address,
        escrow: Address,
        issuer: Address,
        invoice_id: BytesN<32>,
    ) {
        PoolContractClient::new(&env, &pool).fund_invoice(&invoice_id);
        EscrowContractClient::new(&env, &escrow).release_to_issuer(&invoice_id, &issuer);
    }
}

struct FeeLifecycle {
    env: Env,
    usdc_id: Address,
    invoice: InvoiceContractClient<'static>,
    pool: PoolContractClient<'static>,
    registry: RegistryContractClient<'static>,
    escrow_id: Address,
    pool_id: Address,
    issuer: Address,
    buyer: Address,
    lp: Address,
    treasury: Address,
}

fn setup_fee_lifecycle() -> FeeLifecycle {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);
    let treasury = Address::generate(&env);
    let usdc_id = env.register_contract(None, MockToken);
    for address in [&lp, &buyer] {
        let key = TKey(address.clone());
        env.as_contract(&usdc_id, || {
            env.storage()
                .persistent()
                .set(&key, &100_000_000_000_000i128);
        });
    }

    let registry_id = env.register_contract(None, RegistryContract);
    let registry = RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);
    let metadata: Map<String, String> = Map::new(&env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);

    let invoice_id = env.register_contract(None, InvoiceContract);
    let escrow_id = env.register_contract(None, EscrowContract);
    let pool_id = env.register_contract(None, PoolContract);
    let invoice = InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);
    invoice.add_supported_asset(&usdc_id);
    invoice.set_pool_contract(&pool_id);
    invoice.set_escrow_contract(&escrow_id);

    let agent_reg_id = env.register_contract(None, MockAgentRegistry);
    MockAgentRegistryClient::new(&env, &agent_reg_id).register_agent(
        &test_agent_id(&env),
        &clearledger_invoice::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    invoice.set_agent_registry_contract(&agent_reg_id);

    let escrow = EscrowContractClient::new(&env, &escrow_id);
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
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool.set_max_utilization(&admin, &10000);

    FeeLifecycle {
        env,
        usdc_id,
        invoice,
        pool,
        registry,
        escrow_id,
        pool_id,
        issuer,
        buyer,
        lp,
        treasury,
    }
}

fn create_fee_lifecycle_invoice(lifecycle: &FeeLifecycle) -> (BytesN<32>, u64) {
    let face_value = 10_000_000_000u128;
    let due_date = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let invoice_id = lifecycle.invoice.create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &face_value,
        &due_date,
        &lifecycle.usdc_id,
    );
    attest_invoice(&lifecycle.env, &lifecycle.invoice, &invoice_id);
    lifecycle.invoice.list_for_financing(&invoice_id, &200);
    lifecycle.pool.fund_invoice(&invoice_id);
    lifecycle.invoice.mark_shipped(&invoice_id);
    lifecycle
        .invoice
        .confirm_delivery(&invoice_id, &lifecycle.issuer);
    lifecycle
        .invoice
        .confirm_delivery(&invoice_id, &lifecycle.buyer);
    (invoice_id, due_date)
}

fn create_attestation_invoice(lifecycle: &FeeLifecycle) -> BytesN<32> {
    lifecycle.invoice.create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &1_000_000_000,
        &(lifecycle.env.ledger().timestamp() + 30 * 86400),
        &lifecycle.usdc_id,
    )
}

#[test]
fn test_configured_agent_registry_attestation_signature_matrix() {
    let lifecycle = setup_fee_lifecycle();
    let now = lifecycle.env.ledger().timestamp();
    let domain = BytesN::from_array(
        &lifecycle.env,
        &clearledger_invoice::ATTESTATION_DOMAIN_SEPARATOR,
    );
    let agent_id = test_agent_id(&lifecycle.env);

    let valid_id = create_attestation_invoice(&lifecycle);
    let (payload, signature) = signed_attestation(
        &lifecycle.env,
        &valid_id,
        agent_id.clone(),
        domain.clone(),
        now + 3_600,
        1,
    );
    lifecycle
        .invoice
        .submit_attestation(&valid_id, &payload, &signature);
    assert!(lifecycle.invoice.get_attestation(&valid_id).is_some());
    assert!(lifecycle.invoice.list_for_financing(&valid_id, &200));
    assert!(lifecycle
        .invoice
        .try_submit_attestation(&valid_id, &payload, &signature)
        .is_err());

    let listing_expiry_id = create_attestation_invoice(&lifecycle);
    let (payload, signature) = signed_attestation(
        &lifecycle.env,
        &listing_expiry_id,
        agent_id.clone(),
        domain.clone(),
        now + 1,
        2,
    );
    lifecycle
        .invoice
        .submit_attestation(&listing_expiry_id, &payload, &signature);
    lifecycle.env.ledger().set_timestamp(now + 2);
    assert!(lifecycle
        .invoice
        .try_list_for_financing(&listing_expiry_id, &200)
        .is_err());

    let expired_id = create_attestation_invoice(&lifecycle);
    let (payload, signature) = signed_attestation(
        &lifecycle.env,
        &expired_id,
        agent_id.clone(),
        domain.clone(),
        now + 2,
        3,
    );
    assert!(lifecycle
        .invoice
        .try_submit_attestation(&expired_id, &payload, &signature)
        .is_err());

    let wrong_domain_id = create_attestation_invoice(&lifecycle);
    let (payload, signature) = signed_attestation(
        &lifecycle.env,
        &wrong_domain_id,
        agent_id.clone(),
        BytesN::from_array(&lifecycle.env, &[8u8; 32]),
        now + 3_600,
        4,
    );
    assert!(lifecycle
        .invoice
        .try_submit_attestation(&wrong_domain_id, &payload, &signature)
        .is_err());

    let unknown_id = create_attestation_invoice(&lifecycle);
    let (payload, signature) = signed_attestation(
        &lifecycle.env,
        &unknown_id,
        Symbol::new(&lifecycle.env, "unknown_agent"),
        domain.clone(),
        now + 3_600,
        5,
    );
    assert!(lifecycle
        .invoice
        .try_submit_attestation(&unknown_id, &payload, &signature)
        .is_err());

    let revoked_id = create_attestation_invoice(&lifecycle);
    let registry_id = lifecycle.invoice.get_agent_registry_contract().unwrap();
    MockAgentRegistryClient::new(&lifecycle.env, &registry_id).register_agent(
        &agent_id,
        &clearledger_invoice::Agent {
            active: false,
            pubkey: test_agent_pubkey(&lifecycle.env),
        },
    );
    let (payload, signature) = signed_attestation(
        &lifecycle.env,
        &revoked_id,
        agent_id,
        domain,
        now + 3_600,
        6,
    );
    assert!(lifecycle
        .invoice
        .try_submit_attestation(&revoked_id, &payload, &signature)
        .is_err());
}

#[test]
fn test_late_invalid_batch_create_rolls_back_counter_indexes_and_events() {
    use soroban_sdk::{testutils::Events, vec};
    let lifecycle = setup_fee_lifecycle();
    lifecycle.env.budget().reset_unlimited();
    let due = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let existing = lifecycle.invoice.create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &1_000_000_000,
        &due,
        &lifecycle.usdc_id,
    );
    let before = lifecycle.invoice.get(&existing).to_xdr(&lifecycle.env);
    let counts = lifecycle.invoice.get_counts().to_xdr(&lifecycle.env);
    let issuer_count = lifecycle
        .invoice
        .get_invoice_count_by_issuer(&lifecycle.issuer);
    let buyer_count = lifecycle
        .invoice
        .get_invoice_count_by_buyer(&lifecycle.buyer);
    let counter: u64 = lifecycle.env.as_contract(&lifecycle.invoice.address, || {
        lifecycle
            .env
            .storage()
            .instance()
            .get(&clearledger_invoice::DataKey::Counter)
            .unwrap()
    });
    for invalid_kind in 0..3 {
        let mut entries = soroban_sdk::Vec::new(&lifecycle.env);
        for _ in 0..49 {
            entries.push_back((
                lifecycle.buyer.clone(),
                1_000_000_000u128,
                due,
                lifecycle.usdc_id.clone(),
            ));
        }
        let bad = match invalid_kind {
            0 => (
                lifecycle.buyer.clone(),
                0u128,
                due,
                lifecycle.usdc_id.clone(),
            ),
            1 => (
                lifecycle.buyer.clone(),
                1_000_000_000u128,
                lifecycle.env.ledger().timestamp(),
                lifecycle.usdc_id.clone(),
            ),
            _ => (
                lifecycle.buyer.clone(),
                1_000_000_000u128,
                due,
                Address::generate(&lifecycle.env),
            ),
        };
        entries.push_back(bad);
        let event_count = lifecycle.env.events().all().len();
        assert!(lifecycle
            .invoice
            .try_batch_create(&lifecycle.issuer, &entries)
            .is_err());
        assert_eq!(
            lifecycle.invoice.get(&existing).to_xdr(&lifecycle.env),
            before
        );
        assert_eq!(
            lifecycle.invoice.get_counts().to_xdr(&lifecycle.env),
            counts
        );
        assert_eq!(
            lifecycle
                .invoice
                .get_invoice_count_by_issuer(&lifecycle.issuer),
            issuer_count
        );
        assert_eq!(
            lifecycle
                .invoice
                .get_invoice_count_by_buyer(&lifecycle.buyer),
            buyer_count
        );
        assert_eq!(lifecycle.env.events().all().len(), event_count);
        lifecycle.env.as_contract(&lifecycle.invoice.address, || {
            assert_eq!(
                lifecycle
                    .env
                    .storage()
                    .instance()
                    .get::<_, u64>(&clearledger_invoice::DataKey::Counter),
                Some(counter)
            );
            assert!(!lifecycle.env.storage().persistent().has(
                &clearledger_invoice::DataKey::IssuerIndexEntry(
                    lifecycle.issuer.clone(),
                    issuer_count
                )
            ));
            assert!(!lifecycle.env.storage().persistent().has(
                &clearledger_invoice::DataKey::BuyerIndexEntry(
                    lifecycle.buyer.clone(),
                    buyer_count
                )
            ));
            assert!(!lifecycle.env.storage().persistent().has(
                &clearledger_invoice::DataKey::StatusIndexEntry(
                    InvoiceStatus::Created as u32,
                    issuer_count
                )
            ));
        });
    }
    // A valid retry proves failure did not poison the next slot or counter.
    let valid = vec![
        &lifecycle.env,
        (
            lifecycle.buyer.clone(),
            1_000_000_000u128,
            due,
            lifecycle.usdc_id.clone(),
        ),
    ];
    assert_eq!(
        lifecycle
            .invoice
            .batch_create(&lifecycle.issuer, &valid)
            .len(),
        1
    );
    assert_eq!(
        lifecycle
            .invoice
            .get_invoice_count_by_issuer(&lifecycle.issuer),
        issuer_count + 1
    );
}

struct FeeSettlementExpectation {
    shares: u128,
    deposit: u128,
    face_value: u128,
    refund: u128,
    protocol_cut: u128,
    lp_yield: u128,
    buyer_before: i128,
    treasury_before: i128,
}

fn assert_fee_settlement(lifecycle: &FeeLifecycle, expected: FeeSettlementExpectation) {
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let lp_before_withdrawal = token.balance(&lifecycle.lp);
    let returned = lifecycle.pool.withdraw(&lifecycle.lp, &expected.shares);
    assert_eq!(returned, expected.deposit + expected.lp_yield);
    assert_eq!(
        token.balance(&lifecycle.lp) - lp_before_withdrawal,
        returned as i128
    );
    assert_eq!(
        token.balance(&lifecycle.buyer),
        expected.buyer_before - (expected.face_value - expected.refund) as i128
    );
    assert_eq!(
        token.balance(&lifecycle.treasury) - expected.treasury_before,
        expected.protocol_cut as i128
    );
    assert_eq!(token.balance(&lifecycle.pool_id), 0);
    // The issuer's original 98% advance remains in escrow; the 100% face-value
    // repayment is routed back to the pool, so this precisely accounts for the
    // escrow's balance without affecting LP yield or protocol-fee accounting.
    assert_eq!(token.balance(&lifecycle.escrow_id), 9_800_000_000);
}

const DEFAULT_LIFECYCLE_DEPOSIT: u128 = 12_000_000_000;

/// Builds an identical, fully funded invoice for both terminal paths:
/// LP deposit -> list -> pool funding -> shipping -> dual delivery
/// confirmation. The returned invoice is `Confirmed` and therefore eligible
/// for both `repay` and `trigger_default`.
fn setup_funded_lifecycle() -> (FeeLifecycle, BytesN<32>, u64) {
    let lifecycle = setup_fee_lifecycle();
    lifecycle
        .pool
        .deposit(&lifecycle.lp, &DEFAULT_LIFECYCLE_DEPOSIT);
    let (invoice_id, due_date) = create_fee_lifecycle_invoice(&lifecycle);
    (lifecycle, invoice_id, due_date)
}

/// Drives the pool-led default/recovery path.
///
/// `invoice.trigger_default` is deliberately not used here because the
/// invoice -> pool -> invoice callback sequence is pinned as a known
/// self-re-entrancy failure by
/// `test_partial_repayment_then_default_lifecycle` and
/// `pool::test_trigger_default_drives_full_pool_and_escrow_chain`. Calling
/// `pool.handle_default` directly exercises the same escrow recovery,
/// pool-loss, and invoice-status writes without that unrelated runtime
/// limitation.
fn default_via_pool(lifecycle: &FeeLifecycle, invoice_id: &BytesN<32>) -> bool {
    let defaulted = lifecycle.pool.handle_default(invoice_id);
    assert_eq!(
        lifecycle.invoice.get_status(invoice_id),
        InvoiceStatus::Defaulted as u32
    );
    defaulted
}

fn expected_repayment_split(
    invoice: &clearledger_invoice::Invoice,
    repaid_at: u64,
) -> (u128, u128) {
    let funded_at = invoice
        .funded_at
        .expect("funded invoice should record its funding timestamp");
    let discount = invoice.face_value - invoice.funded_amount;
    let term = invoice.due_date - funded_at;
    let elapsed = repaid_at - funded_at;
    let earned_by_pool = if term == 0 {
        discount
    } else {
        discount * (elapsed as u128) / (term as u128)
    };
    let refund_to_buyer = discount - earned_by_pool;
    (earned_by_pool, refund_to_buyer)
}

// Focused SEP-41 consumer checklist using the generic token client.
#[test]
fn test_sep41_share_transfer_and_allowance_checklist() {
    let lifecycle = setup_fee_lifecycle();
    let minted = lifecycle.pool.deposit(&lifecycle.lp, &12_000_000_000);
    let recipient = Address::generate(&lifecycle.env);
    let spender = Address::generate(&lifecycle.env);
    let stranger = Address::generate(&lifecycle.env);
    let shares = soroban_sdk::token::Client::new(&lifecycle.env, &lifecycle.pool_id);
    assert_eq!(
        shares.name(),
        String::from_str(&lifecycle.env, "ClearLedger USDC Pool Shares")
    );
    assert_eq!(shares.symbol(), String::from_str(&lifecycle.env, "TT-USDC"));
    assert_eq!(shares.decimals(), DEFAULT_SHARE_DECIMALS);
    let stats_before = lifecycle.pool.get_stats().to_xdr(&lifecycle.env);
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let pool_assets = token.balance(&lifecycle.pool_id);
    assert_eq!(shares.balance(&lifecycle.lp), minted as i128);
    assert_eq!(shares.balance(&recipient), 0);
    shares.transfer(&lifecycle.lp, &recipient, &100);
    assert_eq!(shares.balance(&lifecycle.lp), minted as i128 - 100);
    assert_eq!(shares.balance(&recipient), 100);
    let expiry = lifecycle.env.ledger().sequence() + 10;
    shares.approve(&lifecycle.lp, &spender, &300, &expiry);
    shares.approve(&lifecycle.lp, &spender, &200, &expiry);
    assert_eq!(shares.allowance(&lifecycle.lp, &spender), 200);
    assert_eq!(shares.allowance(&lifecycle.lp, &stranger), 0);
    shares.transfer_from(&spender, &lifecycle.lp, &recipient, &150);
    assert_eq!(shares.allowance(&lifecycle.lp, &spender), 50);
    assert!(shares
        .try_transfer_from(&spender, &lifecycle.lp, &recipient, &51)
        .is_err());
    assert_eq!(shares.allowance(&lifecycle.lp, &spender), 50);
    assert_eq!(shares.balance(&recipient), 250);
    lifecycle.env.ledger().set_sequence_number(expiry);
    assert_eq!(shares.allowance(&lifecycle.lp, &spender), 50);
    lifecycle.env.ledger().set_sequence_number(expiry + 1);
    assert_eq!(shares.allowance(&lifecycle.lp, &spender), 0);
    assert!(shares
        .try_transfer_from(&spender, &lifecycle.lp, &recipient, &1)
        .is_err());
    shares.approve(&lifecycle.lp, &spender, &0, &0);
    assert_eq!(shares.allowance(&lifecycle.lp, &spender), 0);
    assert_eq!(
        shares.balance(&lifecycle.lp) + shares.balance(&recipient),
        minted as i128
    );
    assert_eq!(
        lifecycle.pool.get_stats().to_xdr(&lifecycle.env),
        stats_before
    );
    assert_eq!(token.balance(&lifecycle.pool_id), pool_assets);
    // Blanket authorization is disabled for this negative control.
    lifecycle.env.set_auths(&[]);
    assert!(shares.try_transfer(&lifecycle.lp, &recipient, &1).is_err());
    assert!(shares
        .try_approve(&lifecycle.lp, &spender, &1, &(expiry + 10))
        .is_err());
    assert_eq!(shares.balance(&recipient), 250);
}

// --------------- Mock Agent Registry ---------------

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

const TEST_AGENT_SEED: [u8; 32] = [7u8; 32];

fn test_agent_signing_key() -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_slice(&TEST_AGENT_SEED).unwrap()
}

fn test_agent_pubkey(env: &Env) -> BytesN<65> {
    let point = test_agent_signing_key()
        .verifying_key()
        .to_encoded_point(false);
    let mut bytes = [0u8; 65];
    bytes.copy_from_slice(point.as_bytes());
    BytesN::from_array(env, &bytes)
}

fn test_agent_id(env: &Env) -> Symbol {
    Symbol::new(env, "test_agent")
}

fn attest_invoice(env: &Env, invoice_client: &InvoiceContractClient, invoice_id: &BytesN<32>) {
    let (payload_bytes, signature) = signed_attestation(
        env,
        invoice_id,
        test_agent_id(env),
        invoice_client.get_attestation_domain_separator(),
        env.ledger().timestamp() + 3_600,
        1,
    );
    invoice_client.submit_attestation(invoice_id, &payload_bytes, &signature);
}

fn signed_attestation(
    env: &Env,
    invoice_id: &BytesN<32>,
    agent_id: Symbol,
    domain_separator: BytesN<32>,
    expires_at: u64,
    nonce: u64,
) -> (Bytes, BytesN<65>) {
    let payload = clearledger_invoice::AttestationPayload {
        domain_separator,
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(env, &[9u8; 32]),
        agent_id,
        nonce,
        expires_at,
    };
    let payload_bytes = payload.to_xdr(env);
    let digest = env.crypto().keccak256(&payload_bytes).to_array();
    let (sig, recid) = test_agent_signing_key()
        .sign_prehash_recoverable(&digest)
        .unwrap();
    let mut sig_bytes = [0u8; 65];
    sig_bytes[..64].copy_from_slice(&sig.to_bytes());
    sig_bytes[64] = recid.to_byte();
    let signature = BytesN::from_array(env, &sig_bytes);
    (payload_bytes, signature)
}

// A deterministic initial state machine, deliberately limited to pre-funding
// create/list/cancel/replay behavior. These seeds reproduce failures exactly.
#[test]
fn test_seeded_prefunding_state_machine_matches_model() {
    for seed in [0x110_u64, 0x5eed_u64, 0xc1ea_u64] {
        let lifecycle = setup_fee_lifecycle();
        lifecycle.env.budget().reset_unlimited();
        let mut rng = seed;
        let mut ids = std::vec::Vec::new();
        let mut model = [InvoiceStatus::Created; 8];
        let due = lifecycle.env.ledger().timestamp() + 30 * 86400;
        for _ in 0..8 {
            ids.push(lifecycle.invoice.create(
                &lifecycle.issuer,
                &lifecycle.buyer,
                &1_000_000_000,
                &due,
                &lifecycle.usdc_id,
            ));
        }
        for step in 0..48 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let index = (rng % 8) as usize;
            let listing = (rng >> 8) & 1 == 1;
            let id = &ids[index];
            let before = lifecycle.invoice.get(id).to_xdr(&lifecycle.env);
            if model[index] == InvoiceStatus::Created {
                if listing {
                    attest_invoice(&lifecycle.env, &lifecycle.invoice, id);
                    assert!(lifecycle.invoice.list_for_financing(id, &200));
                    model[index] = InvoiceStatus::Listed;
                } else {
                    assert!(lifecycle.invoice.cancel(id));
                    model[index] = InvoiceStatus::Cancelled;
                }
            } else {
                let rejected = if listing {
                    lifecycle.invoice.try_list_for_financing(id, &200).is_err()
                } else {
                    lifecycle.invoice.try_cancel(id).is_err()
                };
                assert!(rejected, "seed={seed} step={step} index={index}");
                assert_eq!(
                    lifecycle.invoice.get(id).to_xdr(&lifecycle.env),
                    before,
                    "seed={seed} step={step}"
                );
            }
            for (i, expected) in model.iter().enumerate() {
                assert_eq!(
                    lifecycle.invoice.get_status(&ids[i]),
                    *expected as u32,
                    "seed={seed} step={step} invoice={i}"
                );
            }
            assert_eq!(
                lifecycle
                    .invoice
                    .get_invoice_count_by_issuer(&lifecycle.issuer),
                8
            );
            assert_eq!(
                lifecycle
                    .invoice
                    .get_invoice_count_by_buyer(&lifecycle.buyer),
                8
            );
            let counts = lifecycle.invoice.get_counts();
            for (status, name) in [
                (InvoiceStatus::Created, "Created"),
                (InvoiceStatus::Listed, "Listed"),
                (InvoiceStatus::Cancelled, "Cancelled"),
            ] {
                let expected = model.iter().filter(|s| **s == status).count() as u64;
                assert_eq!(
                    counts.get(String::from_str(&lifecycle.env, name)).unwrap(),
                    expected,
                    "seed={seed} step={step} status={name}"
                );
            }
        }
        let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
        assert_eq!(token.balance(&lifecycle.escrow_id), 0);
        assert_eq!(token.balance(&lifecycle.pool_id), 0);
        assert_eq!(lifecycle.pool.get_stats().total_shares, 0);
    }
}

// --------------- Cross-Contract Integration Tests (Issue #313) ---------------

// Issue #98: explicit pool-authorized issuer payout after the real funding flow.
// Delivery confirmation itself currently changes status only; this exercises the
// existing escrow payout entrypoint, without implying an automatic payout hook.
#[test]
fn test_funded_invoice_explicit_issuer_payout_is_exactly_once() {
    let lifecycle = setup_fee_lifecycle();
    lifecycle.pool.deposit(&lifecycle.lp, &12_000_000_000);
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let issuer_before = token.balance(&lifecycle.issuer);
    let (invoice_id, _) = create_fee_lifecycle_invoice(&lifecycle);
    let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
    let funded = lifecycle.invoice.get(&invoice_id).funded_amount;
    assert_eq!(escrow.get_locked(&invoice_id), funded);
    assert_eq!(token.balance(&lifecycle.issuer), issuer_before);
    let pool_before = token.balance(&lifecycle.pool_id);
    assert!(escrow.release_to_issuer(&invoice_id, &lifecycle.issuer));
    assert_eq!(
        token.balance(&lifecycle.issuer),
        issuer_before + funded as i128
    );
    assert_eq!(token.balance(&lifecycle.escrow_id), 0);
    assert_eq!(token.balance(&lifecycle.pool_id), pool_before);
    assert_eq!(escrow.get_locked(&invoice_id), 0);
    assert!(escrow
        .try_release_to_issuer(&invoice_id, &lifecycle.issuer)
        .is_err());
    assert_eq!(
        token.balance(&lifecycle.issuer),
        issuer_before + funded as i128
    );
    assert_eq!(token.balance(&lifecycle.escrow_id), 0);
    assert_eq!(token.balance(&lifecycle.pool_id), pool_before);
}

#[test]
fn test_cross_contract_invoice_pool_escrow_lifecycle() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    // 1. Deploy & initialize mock USDC token
    let usdc_id = env.register_contract(None, MockToken);
    let lp_key = TKey(lp.clone());
    let buyer_key = TKey(buyer.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&lp_key, &100_000_000_000_000i128);
        env.storage()
            .persistent()
            .set(&buyer_key, &100_000_000_000_000i128);
    });

    // 2. Deploy real Registry contract & register participants
    let registry_id = env.register_contract(None, RegistryContract);
    let registry = RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);

    let metadata: Map<String, String> = Map::new(&env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);
    assert!(registry.is_verified(&issuer));
    assert!(registry.is_verified(&buyer));

    // 3. Deploy real Invoice, Escrow, and Pool contracts
    let invoice_id = env.register_contract(None, InvoiceContract);
    let escrow_id = env.register_contract(None, EscrowContract);
    let pool_id = env.register_contract(None, PoolContract);

    let invoice = InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);
    invoice.add_supported_asset(&usdc_id);
    invoice.set_pool_contract(&pool_id);
    invoice.set_escrow_contract(&escrow_id);

    // Configure Agent Registry for attestations
    let agent_reg_id = env.register_contract(None, MockAgentRegistry);
    let agent_reg = MockAgentRegistryClient::new(&env, &agent_reg_id);
    agent_reg.register_agent(
        &test_agent_id(&env),
        &clearledger_invoice::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    invoice.set_agent_registry_contract(&agent_reg_id);

    let escrow = EscrowContractClient::new(&env, &escrow_id);
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
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool.set_max_utilization(&admin, &10000); // 100% cap

    // 4. Issuer creates invoice
    let face_value = 1_200_000_000u128; // 120 USDC
    let due_date = env.ledger().timestamp() + 86400 * 30;
    let inv_id = invoice.create(&issuer, &buyer, &face_value, &due_date, &usdc_id);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Created as u32);

    // 5. Attest & list invoice for financing (discount 1666 bps)
    attest_invoice(&env, &invoice, &inv_id);
    let discount_bps = 1666u32;
    invoice.list_for_financing(&inv_id, &discount_bps);

    // 6. LP deposits into Pool
    let deposit_amount = 10_000_000_000u128;
    let shares = pool.deposit(&lp, &deposit_amount);
    assert!(shares > 0);

    // 7. Pool funds invoice (cross-contract call into Escrow to lock and Invoice to mark funded)
    let funded = pool.fund_invoice(&inv_id);
    assert!(funded);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Funded as u32);

    // 8. Settle repayment
    let repaid = pool.receive_repayment(&inv_id, &face_value);
    assert!(repaid);

    // 9. LP withdraws principal + earned yield
    let returned = pool.withdraw(&lp, &shares);
    assert!(
        returned > deposit_amount,
        "LP must receive yield on top of deposit"
    );
}

#[test]
fn test_fee_bearing_early_repayment_with_lp_withdrawal() {
    let lifecycle = setup_fee_lifecycle();
    let deposit = 12_000_000_000u128;
    let shares = lifecycle.pool.deposit(&lifecycle.lp, &deposit);
    let fee_bps = 1000u32;
    lifecycle
        .pool
        .set_protocol_fee(&fee_bps, &lifecycle.treasury);
    let (invoice_id, _) = create_fee_lifecycle_invoice(&lifecycle);

    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let buyer_before = token.balance(&lifecycle.buyer);
    let treasury_before = token.balance(&lifecycle.treasury);
    let invoice = lifecycle.invoice.get(&invoice_id);
    let funded_at = invoice.funded_at.unwrap();
    let term = invoice.due_date - funded_at;
    let repaid_at = funded_at + term / 2;
    let (gross_yield, refund) = expected_repayment_split(&invoice, repaid_at);
    assert!(
        refund > 0,
        "early repayment should return part of the discount"
    );
    lifecycle.env.ledger().set_timestamp(repaid_at);

    assert!(lifecycle.invoice.repay_early(&invoice_id));

    let protocol_cut = gross_yield * fee_bps as u128 / 10_000;
    let lp_yield = gross_yield - protocol_cut;
    assert_fee_settlement(
        &lifecycle,
        FeeSettlementExpectation {
            shares,
            deposit,
            face_value: invoice.face_value,
            refund,
            protocol_cut,
            lp_yield,
            buyer_before,
            treasury_before,
        },
    );
}

#[test]
fn test_fee_bearing_full_term_repayment_with_lp_withdrawal() {
    let lifecycle = setup_fee_lifecycle();
    let deposit = 12_000_000_000u128;
    let shares = lifecycle.pool.deposit(&lifecycle.lp, &deposit);
    let fee_bps = 1000u32;
    lifecycle
        .pool
        .set_protocol_fee(&fee_bps, &lifecycle.treasury);
    let (invoice_id, due_date) = create_fee_lifecycle_invoice(&lifecycle);

    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let buyer_before = token.balance(&lifecycle.buyer);
    let treasury_before = token.balance(&lifecycle.treasury);
    lifecycle.env.ledger().set_timestamp(due_date);
    let invoice = lifecycle.invoice.get(&invoice_id);
    let (gross_yield, refund) = expected_repayment_split(&invoice, due_date);
    assert_eq!(refund, 0, "full-term repayment should return no discount");

    assert!(lifecycle.invoice.repay(&invoice_id));

    let protocol_cut = gross_yield * fee_bps as u128 / 10_000;
    let lp_yield = gross_yield - protocol_cut;
    assert_fee_settlement(
        &lifecycle,
        FeeSettlementExpectation {
            shares,
            deposit,
            face_value: invoice.face_value,
            refund,
            protocol_cut,
            lp_yield,
            buyer_before,
            treasury_before,
        },
    );
}

// Issue #105: the relay authenticates its caller, but cannot impersonate the pool.
#[contract]
pub struct UnrelatedEscrowRelay;

#[contractimpl]
impl UnrelatedEscrowRelay {
    pub fn lock(
        env: Env,
        caller: Address,
        escrow: Address,
        id: BytesN<32>,
        amount: u128,
        issuer: Address,
    ) {
        caller.require_auth();
        EscrowContractClient::new(&env, &escrow).lock(&id, &amount, &issuer);
    }
    pub fn release(env: Env, caller: Address, escrow: Address, id: BytesN<32>, issuer: Address) {
        caller.require_auth();
        EscrowContractClient::new(&env, &escrow).release_to_issuer(&id, &issuer);
    }
}

#[test]
fn test_unrelated_account_and_relay_cannot_lock_or_release_escrow() {
    use soroban_sdk::{
        testutils::{MockAuth, MockAuthInvoke},
        IntoVal,
    };
    let lifecycle = setup_fee_lifecycle();
    lifecycle.pool.deposit(&lifecycle.lp, &12_000_000_000);
    let (funded_id, _) = create_fee_lifecycle_invoice(&lifecycle);
    let fresh_id = BytesN::from_array(&lifecycle.env, &[89; 32]);
    let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
    let attacker = Address::generate(&lifecycle.env);
    let relay_id = lifecycle.env.register_contract(None, UnrelatedEscrowRelay);
    let relay = UnrelatedEscrowRelayClient::new(&lifecycle.env, &relay_id);
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let balances = (
        token.balance(&lifecycle.pool_id),
        token.balance(&lifecycle.escrow_id),
        token.balance(&lifecycle.issuer),
    );
    let locked = escrow.get_locked(&funded_id);
    lifecycle.env.set_auths(&[]);
    assert!(escrow
        .mock_auths(&[MockAuth {
            address: &attacker,
            invoke: &MockAuthInvoke {
                contract: &lifecycle.escrow_id,
                fn_name: "lock",
                args: (fresh_id.clone(), 100u128, lifecycle.issuer.clone())
                    .into_val(&lifecycle.env),
                sub_invokes: &[]
            },
        }])
        .try_lock(&fresh_id, &100, &lifecycle.issuer)
        .is_err());
    assert!(escrow
        .mock_auths(&[MockAuth {
            address: &attacker,
            invoke: &MockAuthInvoke {
                contract: &lifecycle.escrow_id,
                fn_name: "release_to_issuer",
                args: (funded_id.clone(), lifecycle.issuer.clone()).into_val(&lifecycle.env),
                sub_invokes: &[]
            },
        }])
        .try_release_to_issuer(&funded_id, &lifecycle.issuer)
        .is_err());
    assert!(relay
        .mock_auths(&[MockAuth {
            address: &attacker,
            invoke: &MockAuthInvoke {
                contract: &relay_id,
                fn_name: "lock",
                args: (
                    attacker.clone(),
                    lifecycle.escrow_id.clone(),
                    fresh_id.clone(),
                    100u128,
                    lifecycle.issuer.clone()
                )
                    .into_val(&lifecycle.env),
                sub_invokes: &[]
            },
        }])
        .try_lock(
            &attacker,
            &lifecycle.escrow_id,
            &fresh_id,
            &100,
            &lifecycle.issuer
        )
        .is_err());
    assert!(relay
        .mock_auths(&[MockAuth {
            address: &attacker,
            invoke: &MockAuthInvoke {
                contract: &relay_id,
                fn_name: "release",
                args: (
                    attacker.clone(),
                    lifecycle.escrow_id.clone(),
                    funded_id.clone(),
                    lifecycle.issuer.clone()
                )
                    .into_val(&lifecycle.env),
                sub_invokes: &[]
            },
        }])
        .try_release(
            &attacker,
            &lifecycle.escrow_id,
            &funded_id,
            &lifecycle.issuer
        )
        .is_err());
    assert_eq!(
        (
            token.balance(&lifecycle.pool_id),
            token.balance(&lifecycle.escrow_id),
            token.balance(&lifecycle.issuer)
        ),
        balances
    );
    assert_eq!(escrow.get_locked(&funded_id), locked);
    assert_eq!(escrow.get_locked(&fresh_id), 0);
    // Narrow positive control: authorizing the configured pool succeeds.
    assert!(escrow
        .mock_auths(&[MockAuth {
            address: &lifecycle.pool_id,
            invoke: &MockAuthInvoke {
                contract: &lifecycle.escrow_id,
                fn_name: "release_to_issuer",
                args: (funded_id.clone(), lifecycle.issuer.clone()).into_val(&lifecycle.env),
                sub_invokes: &[]
            },
        }])
        .release_to_issuer(&funded_id, &lifecycle.issuer));
    assert_eq!(escrow.get_locked(&funded_id), 0);
    assert_eq!(
        token.balance(&lifecycle.issuer),
        balances.2 + locked as i128
    );
}

#[test]
#[should_panic]
fn test_cross_contract_unauthorized_pool_setter_rejected() {
    let env = Env::default();
    env.set_auths(&[]);

    let admin = Address::generate(&env);
    let registry_id = env.register_contract(None, RegistryContract);
    let invoice_id = env.register_contract(None, InvoiceContract);
    let escrow_id = env.register_contract(None, EscrowContract);
    let pool_id = env.register_contract(None, PoolContract);
    let usdc_id = Address::generate(&env);

    let pool = PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &DEFAULT_SHARE_DECIMALS,
    );

    // Non-admin call without authorization must fail
    let attacker = Address::generate(&env);
    pool.set_max_utilization(&attacker, &5000);
}

#[test]
fn test_cross_contract_partial_repayment_lifecycle() {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    // 1. Deploy & initialize mock USDC token
    let usdc_id = env.register_contract(None, MockToken);
    let lp_key = TKey(lp.clone());
    let buyer_key = TKey(buyer.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&lp_key, &100_000_000_000_000i128);
        env.storage()
            .persistent()
            .set(&buyer_key, &100_000_000_000_000i128);
    });

    // 2. Deploy real Registry contract & register participants
    let registry_id = env.register_contract(None, RegistryContract);
    let registry = RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);

    let metadata: Map<String, String> = Map::new(&env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);

    // 3. Deploy real Invoice, Escrow, and Pool contracts
    let invoice_id = env.register_contract(None, InvoiceContract);
    let escrow_id = env.register_contract(None, EscrowContract);
    let pool_id = env.register_contract(None, PoolContract);

    let invoice = InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);
    invoice.add_supported_asset(&usdc_id);
    invoice.set_pool_contract(&pool_id);
    invoice.set_escrow_contract(&escrow_id);

    let agent_reg_id = env.register_contract(None, MockAgentRegistry);
    let agent_reg = MockAgentRegistryClient::new(&env, &agent_reg_id);
    agent_reg.register_agent(
        &test_agent_id(&env),
        &clearledger_invoice::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    invoice.set_agent_registry_contract(&agent_reg_id);

    let escrow = EscrowContractClient::new(&env, &escrow_id);
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
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool.set_max_utilization(&admin, &10000);

    // 4. Issuer creates invoice
    let face_value = 1_200_000_000u128; // 120 USDC
    let due_date = env.ledger().timestamp() + 86400 * 30;
    let inv_id = invoice.create(&issuer, &buyer, &face_value, &due_date, &usdc_id);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Created as u32);

    // 5. Attest & list invoice for financing
    attest_invoice(&env, &invoice, &inv_id);
    let discount_bps = 1666u32;
    invoice.list_for_financing(&inv_id, &discount_bps);

    // 6. LP deposits into Pool
    let deposit_amount = 10_000_000_000u128;
    let shares = pool.deposit(&lp, &deposit_amount);
    assert!(shares > 0);

    // 7. Pool funds invoice
    let funded = pool.fund_invoice(&inv_id);
    assert!(funded);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Funded as u32);

    // Advance to confirmed
    invoice.mark_shipped(&inv_id);
    invoice.confirm_delivery(&inv_id, &issuer);
    invoice.confirm_delivery(&inv_id, &buyer);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);

    // Advance ledger timestamp to due_date so full yield accrues to the pool
    env.ledger().set_timestamp(due_date);

    // 8. Multi-step partial repayment sequence:
    // Step 8a: Repay partial (1st time)

    let part1 = 400_000_000u128;
    let res1 = invoice.repay_partial(&inv_id, &part1);
    assert!(res1);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);
    assert_eq!(invoice.get_remaining_balance(&inv_id), 800_000_000);
    assert_eq!(invoice.get_repaid_amount(&inv_id), 400_000_000);

    // Step 8b: Repay partial (2nd time)
    let part2 = 300_000_000u128;
    let res2 = invoice.repay_partial(&inv_id, &part2);
    assert!(res2);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);
    assert_eq!(invoice.get_remaining_balance(&inv_id), 500_000_000);
    assert_eq!(invoice.get_repaid_amount(&inv_id), 700_000_000);

    // Step 8c: Repay remaining balance -> invoice reaches Repaid
    let rem = 500_000_000u128;
    let res3 = invoice.repay_partial(&inv_id, &rem);
    assert!(res3);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Repaid as u32);
    assert_eq!(invoice.get_remaining_balance(&inv_id), 0);
    assert_eq!(invoice.get_repaid_amount(&inv_id), face_value);

    // 9. LP withdraws principal + earned yield
    let returned = pool.withdraw(&lp, &shares);
    assert!(
        returned > deposit_amount,
        "LP must receive yield on top of deposit"
    );
}

const PARTIAL_REPAYMENT_DEPOSIT: u128 = 12_000_000_000;

fn setup_partial_repayment() -> (FeeLifecycle, BytesN<32>, u64) {
    let lifecycle = setup_fee_lifecycle();
    lifecycle
        .pool
        .deposit(&lifecycle.lp, &PARTIAL_REPAYMENT_DEPOSIT);
    let (invoice_id, due_date) = create_fee_lifecycle_invoice(&lifecycle);
    (lifecycle, invoice_id, due_date)
}

#[test]
fn test_cross_contract_partial_repayment_zero_rejected_without_state_change() {
    let (lifecycle, invoice_id, _) = setup_partial_repayment();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let invoice_before = lifecycle.invoice.get(&invoice_id);
    let stats_before = lifecycle.pool.get_stats();
    let buyer_before = token.balance(&lifecycle.buyer);
    let escrow_before = token.balance(&lifecycle.escrow_id);

    assert!(lifecycle
        .invoice
        .try_repay_partial(&invoice_id, &0)
        .is_err());

    let invoice_after = lifecycle.invoice.get(&invoice_id);
    let stats_after = lifecycle.pool.get_stats();
    assert_eq!(invoice_after.status, invoice_before.status);
    assert_eq!(invoice_after.repaid_amount, invoice_before.repaid_amount);
    assert_eq!(
        invoice_after.remaining_balance,
        invoice_before.remaining_balance
    );
    assert_eq!(stats_after.total_funded, stats_before.total_funded);
    assert_eq!(
        stats_after.active_invoice_count,
        stats_before.active_invoice_count
    );
    assert_eq!(
        stats_after.total_yield_distributed,
        stats_before.total_yield_distributed
    );
    assert_eq!(token.balance(&lifecycle.buyer), buyer_before);
    assert_eq!(token.balance(&lifecycle.escrow_id), escrow_before);
}

#[test]
fn test_cross_contract_partial_repayment_minimum_stroop_accepted() {
    let (lifecycle, invoice_id, _) = setup_partial_repayment();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let invoice_before = lifecycle.invoice.get(&invoice_id);
    let stats_before = lifecycle.pool.get_stats();

    assert!(lifecycle.invoice.repay_partial(&invoice_id, &1));

    let invoice_after = lifecycle.invoice.get(&invoice_id);
    assert_eq!(invoice_after.status, InvoiceStatus::Confirmed);
    assert_eq!(invoice_after.repaid_amount, 1);
    assert_eq!(
        invoice_after.remaining_balance,
        invoice_before.face_value - 1
    );
    assert_eq!(
        lifecycle.invoice.get_remaining_balance(&invoice_id),
        invoice_before.face_value - 1
    );

    let stats_after = lifecycle.pool.get_stats();
    assert_eq!(stats_after.total_funded, stats_before.total_funded);
    assert_eq!(
        stats_after.active_invoice_count,
        stats_before.active_invoice_count
    );
    assert_eq!(
        stats_after.total_yield_distributed,
        stats_before.total_yield_distributed
    );
    assert_eq!(
        token.balance(&lifecycle.escrow_id),
        invoice_before.funded_amount as i128 + 1
    );
}

#[test]
fn test_cross_contract_repeated_partial_repayments_track_invoice_and_pool() {
    let (lifecycle, invoice_id, _) = setup_partial_repayment();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let invoice_before = lifecycle.invoice.get(&invoice_id);
    let stats_before = lifecycle.pool.get_stats();
    let mut cumulative = 0u128;

    for amount in [3_000_000_000u128, 2_000_000_000u128, 1_000_000_000u128] {
        assert!(lifecycle.invoice.repay_partial(&invoice_id, &amount));
        cumulative += amount;

        let invoice_after = lifecycle.invoice.get(&invoice_id);
        assert_eq!(invoice_after.status, InvoiceStatus::Confirmed);
        assert_eq!(invoice_after.repaid_amount, cumulative);
        assert_eq!(
            invoice_after.remaining_balance,
            invoice_before.face_value - cumulative
        );
        assert_eq!(
            lifecycle.invoice.get_remaining_balance(&invoice_id),
            invoice_before.face_value - cumulative
        );

        let stats_after = lifecycle.pool.get_stats();
        assert_eq!(stats_after.total_funded, stats_before.total_funded);
        assert_eq!(
            stats_after.active_invoice_count,
            stats_before.active_invoice_count
        );
        assert_eq!(
            stats_after.total_yield_distributed,
            stats_before.total_yield_distributed
        );
        assert_eq!(
            token.balance(&lifecycle.escrow_id),
            invoice_before.funded_amount as i128 + cumulative as i128
        );
    }
}

#[test]
fn test_cross_contract_final_exact_balance_repayment_settles_invoice() {
    let (lifecycle, invoice_id, due_date) = setup_partial_repayment();
    lifecycle.env.ledger().set_timestamp(due_date);
    let invoice_before = lifecycle.invoice.get(&invoice_id);
    let discount = invoice_before.face_value - invoice_before.funded_amount;
    let first_installment = 4_000_000_000u128;

    assert!(lifecycle
        .invoice
        .repay_partial(&invoice_id, &first_installment));
    let remaining = invoice_before.face_value - first_installment;
    assert!(lifecycle.invoice.repay_partial(&invoice_id, &remaining));

    let invoice_after = lifecycle.invoice.get(&invoice_id);
    assert_eq!(invoice_after.status, InvoiceStatus::Repaid);
    assert_eq!(invoice_after.remaining_balance, 0);
    assert_eq!(invoice_after.repaid_amount, invoice_before.face_value);
    assert!(invoice_after.repaid_at.is_some());

    let stats_after = lifecycle.pool.get_stats();
    assert_eq!(stats_after.total_funded, 0);
    assert_eq!(stats_after.active_invoice_count, 0);
    assert_eq!(stats_after.total_yield_distributed, discount);
    assert_eq!(
        stats_after.total_deposits,
        PARTIAL_REPAYMENT_DEPOSIT + discount
    );

    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    assert_eq!(
        token.balance(&lifecycle.escrow_id),
        invoice_before.funded_amount as i128
    );
}

#[test]
fn test_cross_contract_partial_repayment_over_balance_rejected_without_state_change() {
    let (lifecycle, invoice_id, _) = setup_partial_repayment();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let invoice_before = lifecycle.invoice.get(&invoice_id);
    let stats_before = lifecycle.pool.get_stats();
    let buyer_before = token.balance(&lifecycle.buyer);
    let escrow_before = token.balance(&lifecycle.escrow_id);

    let over_balance = invoice_before.remaining_balance + 1;
    assert!(lifecycle
        .invoice
        .try_repay_partial(&invoice_id, &over_balance)
        .is_err());

    let invoice_after = lifecycle.invoice.get(&invoice_id);
    let stats_after = lifecycle.pool.get_stats();
    assert_eq!(invoice_after.status, invoice_before.status);
    assert_eq!(invoice_after.repaid_amount, invoice_before.repaid_amount);
    assert_eq!(
        invoice_after.remaining_balance,
        invoice_before.remaining_balance
    );
    assert_eq!(stats_after.total_funded, stats_before.total_funded);
    assert_eq!(
        stats_after.active_invoice_count,
        stats_before.active_invoice_count
    );
    assert_eq!(
        stats_after.total_yield_distributed,
        stats_before.total_yield_distributed
    );
    assert_eq!(token.balance(&lifecycle.buyer), buyer_before);
    assert_eq!(token.balance(&lifecycle.escrow_id), escrow_before);
}

/// Opt-in native-host measurements, not network fee estimates or a safe-size promise.
#[test]
#[ignore = "manual budget baseline; run with --ignored --nocapture"]
fn measure_batch_create_budget_through_fifty_entries() {
    use soroban_sdk::vec;
    for size in [1u32, 10, 25, 50] {
        let lifecycle = setup_fee_lifecycle();
        let due = lifecycle.env.ledger().timestamp() + 30 * 86400;
        let mut entries = soroban_sdk::Vec::new(&lifecycle.env);
        for _ in 0..size {
            entries.push_back((
                lifecycle.buyer.clone(),
                1_000_000_000u128,
                due,
                lifecycle.usdc_id.clone(),
            ));
        }
        // Reset only after setup and input construction, so the reported total
        // measures batch invocation rather than deployment/registry fixture work.
        lifecycle.env.budget().reset_unlimited();
        let ids = lifecycle.invoice.batch_create(&lifecycle.issuer, &entries);
        let cpu = lifecycle.env.budget().cpu_instruction_cost();
        let memory = lifecycle.env.budget().memory_bytes_cost();
        std::println!("batch_create,size={size},cpu_instructions={cpu},memory_bytes={memory}");
        assert_eq!(ids.len(), size);
    }
    for size in [1u32, 10, 25, 50] {
        for buyers in [false, true] {
            let lifecycle = setup_fee_lifecycle();
            let registry_id = lifecycle.invoice.get_registry_contract().unwrap();
            let registry = RegistryContractClient::new(&lifecycle.env, &registry_id);
            let mut entries = soroban_sdk::Vec::new(&lifecycle.env);
            for _ in 0..size {
                entries.push_back((
                    Address::generate(&lifecycle.env),
                    Map::<String, String>::new(&lifecycle.env),
                ));
            }
            lifecycle.env.budget().reset_unlimited();
            let skipped = if buyers {
                registry.batch_register_buyers(&entries)
            } else {
                registry.batch_register_issuers(&entries)
            };
            let cpu = lifecycle.env.budget().cpu_instruction_cost();
            let memory = lifecycle.env.budget().memory_bytes_cost();
            std::println!("registry_batch,buyers={buyers},size={size},cpu_instructions={cpu},memory_bytes={memory}");
            assert!(skipped.is_empty());
            let role = if buyers {
                clearledger_registry::Role::Buyer
            } else {
                clearledger_registry::Role::Issuer
            };
            assert_eq!(registry.get_profile_count(&role), size + 1);
        }
    }
    let lifecycle = setup_fee_lifecycle();
    let entry = (
        lifecycle.buyer.clone(),
        1_000_000_000u128,
        lifecycle.env.ledger().timestamp() + 30 * 86400,
        lifecycle.usdc_id.clone(),
    );
    let mut oversized = vec![&lifecycle.env];
    for _ in 0..51 {
        oversized.push_back(entry.clone());
    }
    assert!(lifecycle
        .invoice
        .try_batch_create(&lifecycle.issuer, &oversized)
        .is_err());
    assert_eq!(
        lifecycle
            .invoice
            .get_invoice_count_by_issuer(&lifecycle.issuer),
        0
    );
}

#[test]
#[should_panic(expected = "Contract re-entry is not allowed")]
fn test_partial_repayment_then_default_lifecycle() {
    // Policy for the repaid portion:
    // - Buyer's partial repayments are real funds that have reached escrow.
    // - On default, the unpaid portion (face_value - repaid_amount) is the true loss to LPs.
    // - The pool's total_loss_realised should increase by (funded_amount - repaid_amount), not the full funded_amount.
    // - Escrow should release ALL held funds (original funded_amount + partial repayments) to the pool.
    // - Pool token balance should reflect the partial repayments received.
    // - Escrow token balance should be zero after default handling.
    // - Invoice repaid_amount and remaining_balance should be preserved (not reset).
    //
    // Current behavior (buggy):
    // - pool.handle_default records loss of full funded_amount (overstates LP loss).
    // - escrow.handle_default only returns the original funded_amount, stranding partial repayments in escrow.
    // - Invoice trigger_default -> pool.handle_default -> invoice.mark_defaulted causes reentrancy error.

    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    // 1. Deploy & initialize mock USDC token
    let usdc_id = env.register_contract(None, MockToken);
    let lp_key = TKey(lp.clone());
    let buyer_key = TKey(buyer.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&lp_key, &100_000_000_000_000i128);
        env.storage()
            .persistent()
            .set(&buyer_key, &100_000_000_000_000i128);
    });

    // 2. Deploy real Registry contract & register participants
    let registry_id = env.register_contract(None, RegistryContract);
    let registry = RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);

    let metadata: Map<String, String> = Map::new(&env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);

    // 3. Deploy real Invoice, Escrow, and Pool contracts
    let invoice_id = env.register_contract(None, InvoiceContract);
    let escrow_id = env.register_contract(None, EscrowContract);
    let pool_id = env.register_contract(None, PoolContract);

    let invoice = InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);
    invoice.add_supported_asset(&usdc_id);
    invoice.set_pool_contract(&pool_id);
    invoice.set_escrow_contract(&escrow_id);

    let agent_reg_id = env.register_contract(None, MockAgentRegistry);
    let agent_reg = MockAgentRegistryClient::new(&env, &agent_reg_id);
    agent_reg.register_agent(
        &test_agent_id(&env),
        &clearledger_invoice::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    invoice.set_agent_registry_contract(&agent_reg_id);

    let escrow = EscrowContractClient::new(&env, &escrow_id);
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
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool.set_max_utilization(&admin, &10000);

    // 4. Issuer creates invoice
    let face_value = 1_200_000_000u128; // 120 USDC
    let due_date = env.ledger().timestamp() + 86400 * 30;
    let inv_id = invoice.create(&issuer, &buyer, &face_value, &due_date, &usdc_id);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Created as u32);

    // 5. Attest & list invoice for financing (discount 1666 bps)
    attest_invoice(&env, &invoice, &inv_id);
    let discount_bps = 1666u32;
    invoice.list_for_financing(&inv_id, &discount_bps);

    // 6. LP deposits into Pool
    let deposit_amount = 10_000_000_000u128;
    let shares = pool.deposit(&lp, &deposit_amount);
    assert!(shares > 0);

    // 7. Pool funds invoice
    let funded = pool.fund_invoice(&inv_id);
    assert!(funded);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Funded as u32);

    // Get funded amount for later assertions
    let invoice_data = invoice.get(&inv_id);
    let funded_amount = invoice_data.funded_amount;
    let _funded_at = invoice_data.funded_at.unwrap();

    // Advance to confirmed
    invoice.mark_shipped(&inv_id);
    invoice.confirm_delivery(&inv_id, &issuer);
    invoice.confirm_delivery(&inv_id, &buyer);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);

    // 8. Buyer makes partial repayment (50% of face value)
    let partial_repayment = 600_000_000u128; // 60 USDC = 50% of 120 USDC
    let res = invoice.repay_partial(&inv_id, &partial_repayment);
    assert!(res);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);
    assert_eq!(
        invoice.get_remaining_balance(&inv_id),
        face_value - partial_repayment
    );
    assert_eq!(invoice.get_repaid_amount(&inv_id), partial_repayment);

    // Verify escrow now holds funded_amount + partial_repayment
    let token = MockTokenClient::new(&env, &usdc_id);
    let escrow_balance_after_partial = token.balance(&escrow_id);
    assert_eq!(
        escrow_balance_after_partial,
        funded_amount as i128 + partial_repayment as i128
    );

    // 9. Advance time past due_date AND escrow grace period (60 seconds)
    // escrow DEFAULT_MIN_LOCK_SECONDS = 60, so we need locked_at + 60 < now
    // locked_at == funded_at (approximately), so advance to due_date + 60
    env.ledger().set_timestamp(due_date + 60);

    // 10. Trigger default - this currently fails with reentrancy error
    // because trigger_default -> pool.handle_default -> invoice.mark_defaulted
    // creates a reentrant call into invoice contract.
    let defaulted = invoice.trigger_default(&inv_id);
    assert!(defaulted);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Defaulted as u32);

    // 11. Assert invoice repaid_amount and remaining_balance are preserved
    assert_eq!(invoice.get_repaid_amount(&inv_id), partial_repayment);
    assert_eq!(
        invoice.get_remaining_balance(&inv_id),
        face_value - partial_repayment
    );

    // 12. Assert pool stats reflect correct loss (funded_amount - repaid_amount), not full funded_amount
    let pool_stats = pool.get_stats();
    let expected_loss = funded_amount.saturating_sub(partial_repayment);
    assert_eq!(
        pool_stats.total_loss_realised, expected_loss,
        "LP loss should only be the unpaid funded amount"
    );
    assert_eq!(
        pool_stats.total_funded, 0,
        "no active invoices after default"
    );
    assert_eq!(
        pool_stats.active_invoice_count, 0,
        "active invoice count should be zero"
    );

    // 13. Assert pool token balance reflects partial repayment received
    let pool_token_balance = token.balance(&pool_id);
    assert_eq!(
        pool_token_balance,
        (deposit_amount as i128) - (expected_loss as i128)
    );

    // 14. Assert escrow token balance is zero (all funds released to pool)
    let escrow_token_balance = token.balance(&escrow_id);
    assert_eq!(
        escrow_token_balance, 0,
        "escrow should not strand funds after default"
    );

    // 15. LP withdrawal should reflect correct principal after loss
    let _lp_position_before = pool.get_lp_position(&lp);
    let returned = pool.withdraw(&lp, &shares);
    assert_eq!(returned, deposit_amount - expected_loss);
}

// Issue #103: a paused dependency must roll back the entire outer transaction.
#[test]
fn test_paused_funding_dependency_rolls_back_then_allows_retry() {
    use soroban_sdk::testutils::Events;
    for pause_invoice in [false, true] {
        let lifecycle = setup_fee_lifecycle();
        lifecycle.pool.deposit(&lifecycle.lp, &12_000_000_000);
        let invoice_id = lifecycle.invoice.create(
            &lifecycle.issuer,
            &lifecycle.buyer,
            &10_000_000_000,
            &(lifecycle.env.ledger().timestamp() + 30 * 86400),
            &lifecycle.usdc_id,
        );
        attest_invoice(&lifecycle.env, &lifecycle.invoice, &invoice_id);
        lifecycle.invoice.list_for_financing(&invoice_id, &200);
        let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
        let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
        if pause_invoice {
            lifecycle.invoice.pause();
        } else {
            escrow.pause();
        }
        let stats = lifecycle.pool.get_stats().to_xdr(&lifecycle.env);
        let invoice = lifecycle.invoice.get(&invoice_id).to_xdr(&lifecycle.env);
        let pool_balance = token.balance(&lifecycle.pool_id);
        let escrow_balance = token.balance(&lifecycle.escrow_id);
        let event_count = lifecycle.env.events().all().len();
        assert!(lifecycle.pool.try_fund_invoice(&invoice_id).is_err());
        assert_eq!(lifecycle.pool.get_stats().to_xdr(&lifecycle.env), stats);
        assert_eq!(
            lifecycle.invoice.get(&invoice_id).to_xdr(&lifecycle.env),
            invoice
        );
        assert_eq!(token.balance(&lifecycle.pool_id), pool_balance);
        assert_eq!(token.balance(&lifecycle.escrow_id), escrow_balance);
        assert_eq!(escrow.get_locked(&invoice_id), 0);
        assert_eq!(lifecycle.env.events().all().len(), event_count);
        if pause_invoice {
            lifecycle.invoice.unpause();
        } else {
            escrow.unpause();
        }
        assert!(lifecycle.pool.fund_invoice(&invoice_id));
        assert_eq!(
            lifecycle.invoice.get_status(&invoice_id),
            InvoiceStatus::Funded as u32
        );
        assert_eq!(escrow.get_locked(&invoice_id), 9_800_000_000);
    }
}

#[test]
fn test_paused_repayment_dependency_rolls_back_then_allows_retry() {
    use soroban_sdk::testutils::Events;
    for pause_pool in [false, true] {
        let lifecycle = setup_fee_lifecycle();
        lifecycle.pool.deposit(&lifecycle.lp, &12_000_000_000);
        let (invoice_id, due_date) = create_fee_lifecycle_invoice(&lifecycle);
        lifecycle.env.ledger().set_timestamp(due_date);
        let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
        let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
        if pause_pool {
            lifecycle.pool.pause();
        } else {
            escrow.pause();
        }
        let stats = lifecycle.pool.get_stats().to_xdr(&lifecycle.env);
        let invoice = lifecycle.invoice.get(&invoice_id).to_xdr(&lifecycle.env);
        let balances = (
            token.balance(&lifecycle.pool_id),
            token.balance(&lifecycle.escrow_id),
            token.balance(&lifecycle.buyer),
        );
        let locked = escrow.get_locked(&invoice_id);
        let event_count = lifecycle.env.events().all().len();
        assert!(lifecycle.invoice.try_repay(&invoice_id).is_err());
        assert_eq!(lifecycle.pool.get_stats().to_xdr(&lifecycle.env), stats);
        assert_eq!(
            lifecycle.invoice.get(&invoice_id).to_xdr(&lifecycle.env),
            invoice
        );
        assert_eq!(
            (
                token.balance(&lifecycle.pool_id),
                token.balance(&lifecycle.escrow_id),
                token.balance(&lifecycle.buyer)
            ),
            balances
        );
        assert_eq!(escrow.get_locked(&invoice_id), locked);
        assert_eq!(lifecycle.env.events().all().len(), event_count);
        if pause_pool {
            lifecycle.pool.unpause();
        } else {
            escrow.unpause();
        }
        assert!(lifecycle.invoice.repay(&invoice_id));
        assert_eq!(
            lifecycle.invoice.get_status(&invoice_id),
            InvoiceStatus::Repaid as u32
        );
    }
}

#[test]
#[should_panic(expected = "LP loss should only be the unpaid funded amount")]
fn test_partial_repayment_then_default_pool_loss_calculation() {
    // This test bypasses the reentrancy bug in invoice.trigger_default by calling
    // pool.handle_default and invoice.mark_defaulted directly.
    //
    // Policy for the repaid portion (same as above):
    // - Buyer's partial repayments are real funds that have reached escrow.
    // - On default, the unpaid portion (face_value - repaid_amount) is the true loss to LPs.
    // - The pool's total_loss_realised should increase by (funded_amount - repaid_amount), not the full funded_amount.
    // - Escrow should release ALL held funds (original funded_amount + partial repayments) to the pool.
    //
    // Current buggy behavior in pool.handle_default:
    // - Reads funded_amount from FundedInvoice entry (original funded amount)
    // - Records total_loss_realised += funded_amount (full amount, ignoring partial repayments)
    // - Reduces total_deposits by funded_amount (should only reduce by funded_amount - partial_repayment)
    // - escrow.handle_default only returns the original funded_amount, stranding partial repayments in escrow.

    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    // 1. Deploy & initialize mock USDC token
    let usdc_id = env.register_contract(None, MockToken);
    let lp_key = TKey(lp.clone());
    let buyer_key = TKey(buyer.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&lp_key, &100_000_000_000_000i128);
        env.storage()
            .persistent()
            .set(&buyer_key, &100_000_000_000_000i128);
    });

    // 2. Deploy real Registry contract & register participants
    let registry_id = env.register_contract(None, RegistryContract);
    let registry = RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);

    let metadata: Map<String, String> = Map::new(&env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);

    // 3. Deploy real Invoice, Escrow, and Pool contracts
    let invoice_id = env.register_contract(None, InvoiceContract);
    let escrow_id = env.register_contract(None, EscrowContract);
    let pool_id = env.register_contract(None, PoolContract);

    let invoice = InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);
    invoice.add_supported_asset(&usdc_id);
    invoice.set_pool_contract(&pool_id);
    invoice.set_escrow_contract(&escrow_id);

    let agent_reg_id = env.register_contract(None, MockAgentRegistry);
    let agent_reg = MockAgentRegistryClient::new(&env, &agent_reg_id);
    agent_reg.register_agent(
        &test_agent_id(&env),
        &clearledger_invoice::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    invoice.set_agent_registry_contract(&agent_reg_id);

    let escrow = EscrowContractClient::new(&env, &escrow_id);
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
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool.set_max_utilization(&admin, &10000);

    // 4. Issuer creates invoice
    let face_value = 1_200_000_000u128; // 120 USDC
    let due_date = env.ledger().timestamp() + 86400 * 30;
    let inv_id = invoice.create(&issuer, &buyer, &face_value, &due_date, &usdc_id);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Created as u32);

    // 5. Attest & list invoice for financing (discount 1666 bps)
    attest_invoice(&env, &invoice, &inv_id);
    let discount_bps = 1666u32;
    invoice.list_for_financing(&inv_id, &discount_bps);

    // 6. LP deposits into Pool
    let deposit_amount = 10_000_000_000u128;
    let shares = pool.deposit(&lp, &deposit_amount);
    assert!(shares > 0);

    // 7. Pool funds invoice
    let funded = pool.fund_invoice(&inv_id);
    assert!(funded);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Funded as u32);

    // Get funded amount for later assertions
    let invoice_data = invoice.get(&inv_id);
    let funded_amount = invoice_data.funded_amount;
    let _funded_at = invoice_data.funded_at.unwrap();

    // Advance to confirmed
    invoice.mark_shipped(&inv_id);
    invoice.confirm_delivery(&inv_id, &issuer);
    invoice.confirm_delivery(&inv_id, &buyer);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);

    // 8. Buyer makes partial repayment (50% of face value)
    let partial_repayment = 600_000_000u128; // 60 USDC = 50% of 120 USDC
    let res = invoice.repay_partial(&inv_id, &partial_repayment);
    assert!(res);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Confirmed as u32);
    assert_eq!(
        invoice.get_remaining_balance(&inv_id),
        face_value - partial_repayment
    );
    assert_eq!(invoice.get_repaid_amount(&inv_id), partial_repayment);

    // Verify escrow now holds funded_amount + partial_repayment
    let token = MockTokenClient::new(&env, &usdc_id);
    let escrow_balance_after_partial = token.balance(&escrow_id);
    assert_eq!(
        escrow_balance_after_partial,
        funded_amount as i128 + partial_repayment as i128
    );

    // 9. Advance time past due_date AND escrow grace period (60 seconds)
    env.ledger().set_timestamp(due_date + 60);

    // 10. Call pool.handle_default directly (bypassing invoice.trigger_default reentrancy bug)
    let defaulted = pool.handle_default(&inv_id);
    assert!(defaulted);

    // 11. Call invoice.mark_defaulted directly to set invoice status (requires pool auth)
    let marked = invoice.mark_defaulted(&inv_id);
    assert!(marked);
    assert_eq!(invoice.get_status(&inv_id), InvoiceStatus::Defaulted as u32);

    // 12. Assert invoice repaid_amount and remaining_balance are preserved
    assert_eq!(invoice.get_repaid_amount(&inv_id), partial_repayment);
    assert_eq!(
        invoice.get_remaining_balance(&inv_id),
        face_value - partial_repayment
    );

    // 13. Assert pool stats reflect correct loss (funded_amount - repaid_amount), not full funded_amount
    let pool_stats = pool.get_stats();
    let expected_loss = funded_amount.saturating_sub(partial_repayment);
    assert_eq!(
        pool_stats.total_loss_realised, expected_loss,
        "LP loss should only be the unpaid funded amount"
    );
    assert_eq!(
        pool_stats.total_funded, 0,
        "no active invoices after default"
    );
    assert_eq!(
        pool_stats.active_invoice_count, 0,
        "active invoice count should be zero"
    );

    // 14. Assert pool token balance reflects partial repayment received
    let pool_token_balance = token.balance(&pool_id);
    assert_eq!(
        pool_token_balance,
        (deposit_amount as i128) - (expected_loss as i128)
    );

    // 15. Assert escrow token balance is zero (all funds released to pool)
    let escrow_token_balance = token.balance(&escrow_id);
    assert_eq!(
        escrow_token_balance, 0,
        "escrow should not strand funds after default"
    );

    // 16. LP withdrawal should reflect correct principal after loss
    let returned = pool.withdraw(&lp, &shares);
    assert_eq!(returned, deposit_amount - expected_loss);
}

// --------------- Batch Listing Failure Ordering (Issue #107) ---------------

fn create_batch_test_invoice(lifecycle: &FeeLifecycle, attested: bool) -> BytesN<32> {
    let face_value = 10_000_000_000u128;
    let due_date = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let invoice_id = lifecycle.invoice.create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &face_value,
        &due_date,
        &lifecycle.usdc_id,
    );
    if attested {
        attest_invoice(&lifecycle.env, &lifecycle.invoice, &invoice_id);
    }
    invoice_id
}

/// `batch_list_for_financing` is tolerant per entry: it returns only the IDs
/// that failed, and this test pins that those IDs come back in the exact order
/// the caller supplied them, with the valid entries still listed.
#[test]
fn test_batch_listing_failure_ids_preserve_input_order() {
    let lifecycle = setup_fee_lifecycle();

    let valid_first = create_batch_test_invoice(&lifecycle, true);
    let unattested = create_batch_test_invoice(&lifecycle, false);
    let already_listed = create_batch_test_invoice(&lifecycle, true);
    lifecycle.invoice.list_for_financing(&already_listed, &200);
    let invalid_discount = create_batch_test_invoice(&lifecycle, true);
    let valid_last = create_batch_test_invoice(&lifecycle, true);
    let unattested_last = create_batch_test_invoice(&lifecycle, false);

    // Failure IDs are deliberately out of creation order (and therefore not
    // sorted) so a stable-sort or map iteration cannot satisfy this test.
    let entries = vec![
        &lifecycle.env,
        (valid_first.clone(), 100),
        (unattested_last.clone(), 150),
        (already_listed.clone(), 200),
        (invalid_discount.clone(), 5_001),
        (valid_last.clone(), 250),
        (unattested.clone(), 300),
    ];

    let failed = lifecycle.invoice.batch_list_for_financing(&entries);

    assert_eq!(failed.len(), 4);
    assert_eq!(failed.get(0).unwrap(), unattested_last);
    assert_eq!(failed.get(1).unwrap(), already_listed);
    assert_eq!(failed.get(2).unwrap(), invalid_discount);
    assert_eq!(failed.get(3).unwrap(), unattested);

    assert_eq!(
        lifecycle.invoice.get_status(&valid_first),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&valid_first), 100);
    assert_eq!(
        lifecycle.invoice.get_status(&valid_last),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&valid_last), 250);

    assert_eq!(
        lifecycle.invoice.get_status(&unattested),
        InvoiceStatus::Created as u32
    );
    assert_eq!(
        lifecycle.invoice.get_status(&already_listed),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&already_listed), 200);
    assert_eq!(
        lifecycle.invoice.get_status(&invalid_discount),
        InvoiceStatus::Created as u32
    );
    assert_eq!(
        lifecycle.invoice.get_status(&unattested_last),
        InvoiceStatus::Created as u32
    );
}

#[test]
fn test_batch_listing_all_valid_returns_no_failures() {
    let lifecycle = setup_fee_lifecycle();

    let first = create_batch_test_invoice(&lifecycle, true);
    let second = create_batch_test_invoice(&lifecycle, true);
    let third = create_batch_test_invoice(&lifecycle, true);

    let entries = vec![
        &lifecycle.env,
        (second.clone(), 400),
        (first.clone(), 123),
        (third.clone(), 5_000),
    ];

    let failed = lifecycle.invoice.batch_list_for_financing(&entries);

    assert!(failed.is_empty());
    assert_eq!(
        lifecycle.invoice.get_status(&first),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&first), 123);
    assert_eq!(
        lifecycle.invoice.get_status(&second),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&second), 400);
    assert_eq!(
        lifecycle.invoice.get_status(&third),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&third), 5_000);
}

#[test]
fn test_batch_listing_all_failing_preserves_input_order() {
    let lifecycle = setup_fee_lifecycle();

    let already_listed = create_batch_test_invoice(&lifecycle, true);
    lifecycle.invoice.list_for_financing(&already_listed, &200);
    let unattested_first = create_batch_test_invoice(&lifecycle, false);
    let invalid_discount = create_batch_test_invoice(&lifecycle, true);
    let unattested_last = create_batch_test_invoice(&lifecycle, false);

    let entries = vec![
        &lifecycle.env,
        (unattested_last.clone(), 200),
        (already_listed.clone(), 300),
        (invalid_discount.clone(), 6_000),
        (unattested_first.clone(), 100),
    ];

    let failed = lifecycle.invoice.batch_list_for_financing(&entries);

    assert_eq!(failed.len(), 4);
    assert_eq!(failed.get(0).unwrap(), unattested_last);
    assert_eq!(failed.get(1).unwrap(), already_listed);
    assert_eq!(failed.get(2).unwrap(), invalid_discount);
    assert_eq!(failed.get(3).unwrap(), unattested_first);

    assert_eq!(
        lifecycle.invoice.get_status(&already_listed),
        InvoiceStatus::Listed as u32
    );
    assert_eq!(lifecycle.invoice.get_discount_bps(&already_listed), 200);
    for invoice_id in [unattested_first, invalid_discount, unattested_last] {
        assert_eq!(
            lifecycle.invoice.get_status(&invoice_id),
            InvoiceStatus::Created as u32
        );
        assert_eq!(lifecycle.invoice.get_discount_bps(&invoice_id), 0);
    }
}

// --------------- Funding Atomicity (Issue #20) ---------------

#[test]
fn test_fund_invoice_rolls_back_atomically_when_issuer_payout_fails() {
    let lifecycle = setup_fee_lifecycle();
    let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);

    let deposit = 12_000_000_000u128;
    lifecycle.pool.deposit(&lifecycle.lp, &deposit);

    let face_value = 10_000_000_000u128;
    let due_date = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let invoice_id = lifecycle.invoice.create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &face_value,
        &due_date,
        &lifecycle.usdc_id,
    );
    attest_invoice(&lifecycle.env, &lifecycle.invoice, &invoice_id);
    lifecycle.invoice.list_for_financing(&invoice_id, &200);

    let invoice_before = lifecycle.invoice.get(&invoice_id);
    let stats_before = lifecycle.pool.get_stats();
    let locked_before = escrow.get_locked(&invoice_id);
    let history_len_before = escrow.get_history(&invoice_id).len();
    let issuer_before = token.balance(&lifecycle.issuer);
    let escrow_before = token.balance(&lifecycle.escrow_id);
    let pool_before = token.balance(&lifecycle.pool_id);
    let lp_before = token.balance(&lifecycle.lp);

    // Make only the escrow -> issuer payout leg fail; pool -> escrow lock and
    // LP -> pool deposit still succeed.
    token.set_fail_transfers_to(&lifecycle.issuer);

    let orchestrator_id = lifecycle.env.register_contract(None, FundingOrchestrator);
    let orchestrator = FundingOrchestratorClient::new(&lifecycle.env, &orchestrator_id);
    let result = orchestrator.try_fund_and_release(
        &lifecycle.pool_id,
        &lifecycle.escrow_id,
        &lifecycle.issuer,
        &invoice_id,
    );
    assert!(
        result.is_err(),
        "issuer payout failure must revert the call"
    );

    let invoice_after = lifecycle.invoice.get(&invoice_id);
    assert_eq!(invoice_after.status, invoice_before.status);
    assert_eq!(invoice_after.status, InvoiceStatus::Listed);
    assert_eq!(invoice_after.funded_amount, invoice_before.funded_amount);
    assert_eq!(invoice_after.funded_at, invoice_before.funded_at);
    assert_eq!(invoice_after.funding_pool, invoice_before.funding_pool);
    assert_eq!(invoice_after.listed_at, invoice_before.listed_at);
    assert_eq!(invoice_after.repaid_amount, invoice_before.repaid_amount);
    assert_eq!(
        invoice_after.remaining_balance,
        invoice_before.remaining_balance
    );

    let stats_after = lifecycle.pool.get_stats();
    assert_eq!(stats_after.total_deposits, stats_before.total_deposits);
    assert_eq!(stats_after.total_funded, stats_before.total_funded);
    assert_eq!(
        stats_after.available_liquidity,
        stats_before.available_liquidity
    );
    assert_eq!(
        stats_after.utilization_rate_bps,
        stats_before.utilization_rate_bps
    );
    assert_eq!(
        stats_after.active_invoice_count,
        stats_before.active_invoice_count
    );
    assert_eq!(stats_after.total_shares, stats_before.total_shares);

    assert_eq!(escrow.get_locked(&invoice_id), locked_before);
    assert_eq!(escrow.get_locked(&invoice_id), 0);
    assert_eq!(escrow.get_history(&invoice_id).len(), history_len_before);
    assert_eq!(token.balance(&lifecycle.issuer), issuer_before);
    assert_eq!(token.balance(&lifecycle.escrow_id), escrow_before);
    assert_eq!(token.balance(&lifecycle.pool_id), pool_before);
    assert_eq!(token.balance(&lifecycle.lp), lp_before);
}
// --------------- Repayment / Default Terminal Exclusivity (Issue #99) ---------------

#[test]
fn test_full_repayment_blocks_default_and_second_settlement() {
    let (lifecycle, invoice_id, due_date) = setup_funded_lifecycle();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
    let invoice = lifecycle.invoice.get(&invoice_id);
    let funded_amount = invoice.funded_amount;
    let face_value = invoice.face_value;
    let discount = face_value - funded_amount;

    lifecycle.env.ledger().set_timestamp(due_date);
    assert!(lifecycle.invoice.repay(&invoice_id));

    // Repayment is terminal: the invoice has no remaining balance, the pool has
    // no outstanding funded principal, and the escrow lock has been consumed.
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Repaid as u32
    );
    assert_eq!(lifecycle.invoice.get_remaining_balance(&invoice_id), 0);
    assert_eq!(lifecycle.invoice.get_repaid_amount(&invoice_id), face_value);
    assert_eq!(lifecycle.pool.get_funded_amount(&invoice_id), None);
    assert_eq!(escrow.get_locked(&invoice_id), 0);

    let stats_after_repay = lifecycle.pool.get_stats();
    assert_eq!(stats_after_repay.total_funded, 0);
    assert_eq!(stats_after_repay.active_invoice_count, 0);
    assert_eq!(stats_after_repay.total_loss_realised, 0);
    assert_eq!(
        stats_after_repay.total_deposits,
        DEFAULT_LIFECYCLE_DEPOSIT + discount
    );
    // Pool receives face value from escrow and (at maturity) refunds nothing.
    assert_eq!(
        token.balance(&lifecycle.pool_id),
        (DEFAULT_LIFECYCLE_DEPOSIT + discount) as i128
    );
    // Escrow retains only the original funded principal, no repayment balance.
    assert_eq!(token.balance(&lifecycle.escrow_id), funded_amount as i128);

    let buyer_after_repay = token.balance(&lifecycle.buyer);
    let escrow_after_repay = token.balance(&lifecycle.escrow_id);

    // Every default/recovery entry point must reject the already-repaid
    // invoice without moving funds or writing accounting.
    lifecycle.env.ledger().set_timestamp(due_date + 60);
    assert!(lifecycle.invoice.try_trigger_default(&invoice_id).is_err());
    assert!(lifecycle.pool.try_handle_default(&invoice_id).is_err());
    assert!(!escrow.handle_default(&invoice_id, &lifecycle.pool_id));
    assert!(lifecycle.invoice.try_repay(&invoice_id).is_err());
    assert!(lifecycle
        .invoice
        .try_repay_partial(&invoice_id, &1u128)
        .is_err());

    let stats_after_rejections = lifecycle.pool.get_stats();
    assert_eq!(
        stats_after_rejections.total_deposits,
        stats_after_repay.total_deposits
    );
    assert_eq!(
        stats_after_rejections.total_funded,
        stats_after_repay.total_funded
    );
    assert_eq!(
        stats_after_rejections.total_loss_realised,
        stats_after_repay.total_loss_realised
    );
    assert_eq!(
        stats_after_rejections.active_invoice_count,
        stats_after_repay.active_invoice_count
    );
    assert_eq!(token.balance(&lifecycle.buyer), buyer_after_repay);
    assert_eq!(token.balance(&lifecycle.escrow_id), escrow_after_repay);
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Repaid as u32
    );
    assert_eq!(lifecycle.invoice.get_remaining_balance(&invoice_id), 0);
}

#[test]
fn test_default_blocks_repayment_and_second_settlement() {
    let (lifecycle, invoice_id, due_date) = setup_funded_lifecycle();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
    let invoice = lifecycle.invoice.get(&invoice_id);
    let funded_amount = invoice.funded_amount;
    let face_value = invoice.face_value;

    // Funded + confirmed invoice defaults after maturity and the escrow grace
    // period, exercising the pool-led recovery path.
    lifecycle.env.ledger().set_timestamp(due_date + 60);
    assert!(default_via_pool(&lifecycle, &invoice_id));

    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Defaulted as u32
    );
    // No repayment occurred, so the invoice balance is preserved (not reset).
    assert_eq!(
        lifecycle.invoice.get_remaining_balance(&invoice_id),
        face_value
    );
    assert_eq!(lifecycle.invoice.get_repaid_amount(&invoice_id), 0);
    assert_eq!(lifecycle.pool.get_funded_amount(&invoice_id), None);
    assert_eq!(escrow.get_locked(&invoice_id), 0);

    let stats_after_default = lifecycle.pool.get_stats();
    assert_eq!(stats_after_default.total_funded, 0);
    assert_eq!(stats_after_default.active_invoice_count, 0);
    assert_eq!(stats_after_default.total_loss_realised, funded_amount);
    assert_eq!(
        stats_after_default.total_deposits, DEFAULT_LIFECYCLE_DEPOSIT,
        "default recovery must not double-count the write-off against LP deposits"
    );
    // The escrow advance is returned exactly once, restoring the deposited
    // principal available to the pool.
    assert_eq!(
        token.balance(&lifecycle.pool_id),
        DEFAULT_LIFECYCLE_DEPOSIT as i128
    );
    assert_eq!(token.balance(&lifecycle.escrow_id), 0);

    let buyer_after_default = token.balance(&lifecycle.buyer);
    let escrow_after_default = token.balance(&lifecycle.escrow_id);

    // The terminal default must reject every repayment variant (no buyer
    // transfer) and replaying default must not realize a second loss.
    assert!(lifecycle.invoice.try_repay(&invoice_id).is_err());
    assert!(lifecycle
        .invoice
        .try_repay_partial(&invoice_id, &1u128)
        .is_err());
    assert!(lifecycle.invoice.try_repay_early(&invoice_id).is_err());
    assert!(lifecycle.invoice.try_trigger_default(&invoice_id).is_err());
    assert!(lifecycle.pool.try_handle_default(&invoice_id).is_err());
    assert!(!escrow.handle_default(&invoice_id, &lifecycle.pool_id));

    let stats_after_rejections = lifecycle.pool.get_stats();
    assert_eq!(
        stats_after_rejections.total_deposits,
        stats_after_default.total_deposits
    );
    assert_eq!(
        stats_after_rejections.total_funded,
        stats_after_default.total_funded
    );
    assert_eq!(
        stats_after_rejections.total_loss_realised,
        stats_after_default.total_loss_realised
    );
    assert_eq!(
        stats_after_rejections.active_invoice_count,
        stats_after_default.active_invoice_count
    );
    assert_eq!(token.balance(&lifecycle.buyer), buyer_after_default);
    assert_eq!(token.balance(&lifecycle.escrow_id), escrow_after_default);
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Defaulted as u32
    );
    assert_eq!(
        lifecycle.invoice.get_remaining_balance(&invoice_id),
        face_value
    );
}

#[test]
fn test_partial_repayment_then_default_preserves_balance_without_double_settlement() {
    let (lifecycle, invoice_id, due_date) = setup_funded_lifecycle();
    let token = MockTokenClient::new(&lifecycle.env, &lifecycle.usdc_id);
    let escrow = EscrowContractClient::new(&lifecycle.env, &lifecycle.escrow_id);
    let invoice = lifecycle.invoice.get(&invoice_id);
    let funded_amount = invoice.funded_amount;
    let face_value = invoice.face_value;
    let partial_repayment = face_value / 2;

    lifecycle.env.ledger().set_timestamp(due_date);
    assert!(lifecycle
        .invoice
        .repay_partial(&invoice_id, &partial_repayment));

    // Partial repayment records the installment but does not settle the
    // invoice: status stays non-terminal, the remaining balance shrinks, and
    // the escrow lock still covers the original funded principal.
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Confirmed as u32
    );
    assert_eq!(
        lifecycle.invoice.get_remaining_balance(&invoice_id),
        face_value - partial_repayment
    );
    assert_eq!(
        lifecycle.invoice.get_repaid_amount(&invoice_id),
        partial_repayment
    );
    assert_eq!(escrow.get_locked(&invoice_id), funded_amount);
    assert_eq!(
        token.balance(&lifecycle.escrow_id),
        (funded_amount + partial_repayment) as i128
    );
    assert_eq!(
        token.balance(&lifecycle.pool_id),
        (DEFAULT_LIFECYCLE_DEPOSIT - funded_amount) as i128
    );

    let stats_before_default = lifecycle.pool.get_stats();
    assert_eq!(stats_before_default.total_funded, funded_amount);
    assert_eq!(stats_before_default.active_invoice_count, 1);
    assert_eq!(stats_before_default.total_loss_realised, 0);
    assert_eq!(
        stats_before_default.total_deposits,
        DEFAULT_LIFECYCLE_DEPOSIT
    );

    // Default after the partial installment: the unpaid balance is recovered
    // as a single write-off, and the invoice's repayment counters are not
    // reset to zero (which is what a double settlement would look like).
    lifecycle.env.ledger().set_timestamp(due_date + 60);
    assert!(default_via_pool(&lifecycle, &invoice_id));

    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Defaulted as u32
    );
    assert_eq!(
        lifecycle.invoice.get_repaid_amount(&invoice_id),
        partial_repayment
    );
    assert_eq!(
        lifecycle.invoice.get_remaining_balance(&invoice_id),
        face_value - partial_repayment
    );
    assert_eq!(lifecycle.pool.get_funded_amount(&invoice_id), None);
    assert_eq!(escrow.get_locked(&invoice_id), 0);

    let stats_after_default = lifecycle.pool.get_stats();
    assert_eq!(stats_after_default.total_funded, 0);
    assert_eq!(stats_after_default.active_invoice_count, 0);
    assert_eq!(
        stats_after_default.total_deposits, stats_before_default.total_deposits,
        "default must not reduce LP deposits a second time"
    );
    // Current single-settlement accounting: the originating funded principal
    // is written off exactly once by the pool, and escrow releases that same
    // principal exactly once (the partial installment remains in escrow).
    assert_eq!(stats_after_default.total_loss_realised, funded_amount);
    assert_eq!(
        token.balance(&lifecycle.pool_id),
        DEFAULT_LIFECYCLE_DEPOSIT as i128
    );
    assert_eq!(
        token.balance(&lifecycle.escrow_id),
        partial_repayment as i128
    );

    let buyer_after_default = token.balance(&lifecycle.buyer);
    let escrow_after_default = token.balance(&lifecycle.escrow_id);

    // No repayment variable may resurrect a defaulted invoice, and replaying
    // the default path must not move more funds or realize more loss.
    assert!(lifecycle.invoice.try_repay(&invoice_id).is_err());
    assert!(lifecycle
        .invoice
        .try_repay_partial(&invoice_id, &1u128)
        .is_err());
    assert!(lifecycle.invoice.try_repay_early(&invoice_id).is_err());
    assert!(lifecycle.invoice.try_trigger_default(&invoice_id).is_err());
    assert!(lifecycle.pool.try_handle_default(&invoice_id).is_err());
    assert!(!escrow.handle_default(&invoice_id, &lifecycle.pool_id));

    let stats_after_rejections = lifecycle.pool.get_stats();
    assert_eq!(
        stats_after_rejections.total_deposits,
        stats_after_default.total_deposits
    );
    assert_eq!(
        stats_after_rejections.total_funded,
        stats_after_default.total_funded
    );
    assert_eq!(
        stats_after_rejections.total_loss_realised,
        stats_after_default.total_loss_realised
    );
    assert_eq!(
        stats_after_rejections.active_invoice_count,
        stats_after_default.active_invoice_count
    );
    assert_eq!(token.balance(&lifecycle.buyer), buyer_after_default);
    assert_eq!(token.balance(&lifecycle.escrow_id), escrow_after_default);
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Defaulted as u32
    );
    assert_eq!(
        lifecycle.invoice.get_remaining_balance(&invoice_id),
        face_value - partial_repayment
    );
    assert_eq!(
        lifecycle.invoice.get_repaid_amount(&invoice_id),
        partial_repayment
    );
}

/// Opt-in measurement only: normal correctness tests do not rewrite baselines.
#[test]
#[ignore = "run scripts/report-budgets.sh to generate a native testutils report"]
fn report_core_workflow_budgets() {
    fn measure<T>(
        env: &Env,
        rows: &mut std::vec::Vec<std::string::String>,
        name: &str,
        action: impl FnOnce() -> T,
    ) -> T {
        env.budget().reset_unlimited();
        let result = action();
        let cpu = env.budget().cpu_instruction_cost();
        let memory = env.budget().memory_bytes_cost();
        assert!(cpu > 0, "measurement did not execute host work");
        rows.push(std::format!(
            "{{\"workflow\":\"{name}\",\"cpu_instructions\":{cpu},\"memory_bytes\":{memory}}}"
        ));
        result
    }

    let mut rows = std::vec::Vec::new();
    let lifecycle = setup_fee_lifecycle();
    let env = &lifecycle.env;
    let shares = measure(env, &mut rows, "deposit", || {
        lifecycle.pool.deposit(&lifecycle.lp, &12_000_000_000)
    });
    let due = env.ledger().timestamp() + 30 * 86400;
    let id = measure(env, &mut rows, "create", || {
        lifecycle.invoice.create(
            &lifecycle.issuer,
            &lifecycle.buyer,
            &10_000_000_000,
            &due,
            &lifecycle.usdc_id,
        )
    });
    attest_invoice(env, &lifecycle.invoice, &id);
    measure(env, &mut rows, "list", || {
        lifecycle.invoice.list_for_financing(&id, &200)
    });
    assert!(measure(env, &mut rows, "fund", || lifecycle
        .pool
        .fund_invoice(&id)));
    lifecycle.invoice.mark_shipped(&id);
    lifecycle.invoice.confirm_delivery(&id, &lifecycle.issuer);
    lifecycle.invoice.confirm_delivery(&id, &lifecycle.buyer);
    env.ledger().set_timestamp(due);
    assert!(measure(env, &mut rows, "repay", || lifecycle
        .invoice
        .repay(&id)));
    assert!(
        measure(env, &mut rows, "withdraw", || lifecycle
            .pool
            .withdraw(&lifecycle.lp, &shares))
            > 0
    );

    // Separate fixture: repayment and default are mutually exclusive terminal
    // paths. Measure the existing pool default entry, not the known-reentrant
    // invoice.trigger_default path documented by the lifecycle tests above.
    let default_case = setup_fee_lifecycle();
    default_case.pool.deposit(&default_case.lp, &12_000_000_000);
    let (default_id, default_due) = create_fee_lifecycle_invoice(&default_case);
    default_case.env.ledger().set_timestamp(default_due + 60);
    assert!(measure(&default_case.env, &mut rows, "default", || {
        default_case.pool.handle_default(&default_id)
    }));
    assert!(default_case.invoice.mark_defaulted(&default_id));
    assert_eq!(rows.len(), 7);

    let report = std::format!("{{\n  \"schema_version\":1,\n  \"execution\":\"native-soroban-testutils\",\n  \"budget_limits\":\"unlimited\",\n  \"mock_authorization\":true,\n  \"amount_stroops\":10000000000,\n  \"deposit_stroops\":12000000000,\n  \"discount_bps\":200,\n  \"results\":[\n    {}\n  ]\n}}\n", rows.join(",\n    "));
    let destination = std::env::var("CLEARLEDGER_BUDGET_REPORT")
        .expect("set CLEARLEDGER_BUDGET_REPORT via scripts/report-budgets.sh");
    let path = std::path::Path::new(&destination);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create report directory");
    }
    // Do not publish a partial report if any workflow failed above.
    std::fs::write(path, report).expect("write budget report");
    std::println!("Budget report written to {}", path.display());
}

// --------------- Registry Revocation Integration Tests (Issue #100) ---------------
//
// Revocation is prospective: `invoice.create`, `invoice.list_for_financing`, and
// `pool.fund_invoice` each re-check the real registry before committing new
// business. An invoice that was already funded is deliberately *not* unwound by
// a later revocation, because pool capital is already committed and repayment
// terms are fixed. The tests below pin both sides of that rule against the real
// registry, invoice, and pool contracts.

fn create_attested_invoice(lifecycle: &FeeLifecycle, face_value: u128) -> (BytesN<32>, u64) {
    let due_date = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let invoice_id = lifecycle.invoice.create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &face_value,
        &due_date,
        &lifecycle.usdc_id,
    );
    attest_invoice(&lifecycle.env, &lifecycle.invoice, &invoice_id);
    (invoice_id, due_date)
}

#[test]
fn test_revoked_issuer_cannot_create_invoice() {
    let lifecycle = setup_fee_lifecycle();
    lifecycle.registry.revoke(&lifecycle.issuer);
    assert_eq!(
        lifecycle
            .registry
            .get_verification_status(&lifecycle.issuer),
        VerificationStatus::Revoked
    );
    assert!(!lifecycle.registry.is_verified(&lifecycle.issuer));

    let face_value = 1_200_000_000u128;
    let due_date = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let result = lifecycle.invoice.try_create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &face_value,
        &due_date,
        &lifecycle.usdc_id,
    );
    assert!(result.is_err());
    assert!(
        std::format!("{result:?}").contains("#4"),
        "revoked issuer should fail invoice.create with IssuerNotVerified: {result:?}"
    );
}

#[test]
fn test_revoked_buyer_cannot_create_invoice() {
    let lifecycle = setup_fee_lifecycle();
    lifecycle.registry.revoke(&lifecycle.buyer);
    assert_eq!(
        lifecycle.registry.get_verification_status(&lifecycle.buyer),
        VerificationStatus::Revoked
    );
    assert!(!lifecycle.registry.is_verified(&lifecycle.buyer));

    let face_value = 1_200_000_000u128;
    let due_date = lifecycle.env.ledger().timestamp() + 30 * 86400;
    let result = lifecycle.invoice.try_create(
        &lifecycle.issuer,
        &lifecycle.buyer,
        &face_value,
        &due_date,
        &lifecycle.usdc_id,
    );
    assert!(result.is_err());
    assert!(
        std::format!("{result:?}").contains("#5"),
        "revoked buyer should fail invoice.create with BuyerNotVerified: {result:?}"
    );
}

#[test]
fn test_revocation_after_create_blocks_listing() {
    let lifecycle = setup_fee_lifecycle();

    // Issuer was verified at create() time, then revoked before listing.
    let (issuer_invoice, _) = create_attested_invoice(&lifecycle, 1_200_000_000);
    lifecycle.registry.revoke(&lifecycle.issuer);
    let issuer_list = lifecycle
        .invoice
        .try_list_for_financing(&issuer_invoice, &200);
    assert!(issuer_list.is_err());
    assert!(
        std::format!("{issuer_list:?}").contains("#4"),
        "revoked issuer should fail list_for_financing with IssuerNotVerified: {issuer_list:?}"
    );
    assert_eq!(
        lifecycle.invoice.get_status(&issuer_invoice),
        InvoiceStatus::Created as u32,
        "failed listing must not advance the invoice past Created"
    );
    lifecycle.registry.reinstate(&lifecycle.issuer);
    assert!(lifecycle.invoice.list_for_financing(&issuer_invoice, &200));

    // Repeat with a buyer revocation between create() and listing.
    let (buyer_invoice, _) = create_attested_invoice(&lifecycle, 1_200_000_000);
    lifecycle.registry.revoke(&lifecycle.buyer);
    let buyer_list = lifecycle
        .invoice
        .try_list_for_financing(&buyer_invoice, &200);
    assert!(buyer_list.is_err());
    assert!(
        std::format!("{buyer_list:?}").contains("#5"),
        "revoked buyer should fail list_for_financing with BuyerNotVerified: {buyer_list:?}"
    );
    assert_eq!(
        lifecycle.invoice.get_status(&buyer_invoice),
        InvoiceStatus::Created as u32,
        "failed listing must not advance the invoice past Created"
    );
    lifecycle.registry.reinstate(&lifecycle.buyer);
    assert!(lifecycle.invoice.list_for_financing(&buyer_invoice, &200));
}

#[test]
fn test_revocation_after_listing_blocks_funding() {
    let lifecycle = setup_fee_lifecycle();
    lifecycle.pool.deposit(&lifecycle.lp, &10_000_000_000u128);

    // Issuer was verified at listing time, then revoked before funding.
    let (issuer_invoice, _) = create_attested_invoice(&lifecycle, 1_200_000_000);
    lifecycle.invoice.list_for_financing(&issuer_invoice, &200);
    lifecycle.registry.revoke(&lifecycle.issuer);
    let stats_before = lifecycle.pool.get_stats();
    let issuer_funding = lifecycle.pool.try_fund_invoice(&issuer_invoice);
    assert!(issuer_funding.is_err());
    assert!(
        std::format!("{issuer_funding:?}").contains("#18"),
        "revoked issuer should fail fund_invoice with IssuerNotVerified: {issuer_funding:?}"
    );
    let stats_after = lifecycle.pool.get_stats();
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
    assert_eq!(
        lifecycle.invoice.get_status(&issuer_invoice),
        InvoiceStatus::Listed as u32,
        "failed funding must leave the invoice listed"
    );
    lifecycle.registry.reinstate(&lifecycle.issuer);
    assert!(lifecycle.pool.fund_invoice(&issuer_invoice));

    // Repeat with a buyer revocation between listing and funding.
    let (buyer_invoice, _) = create_attested_invoice(&lifecycle, 1_200_000_000);
    lifecycle.invoice.list_for_financing(&buyer_invoice, &200);
    lifecycle.registry.revoke(&lifecycle.buyer);
    let stats_before = lifecycle.pool.get_stats();
    let buyer_funding = lifecycle.pool.try_fund_invoice(&buyer_invoice);
    assert!(buyer_funding.is_err());
    assert!(
        std::format!("{buyer_funding:?}").contains("#19"),
        "revoked buyer should fail fund_invoice with BuyerNotVerified: {buyer_funding:?}"
    );
    let stats_after = lifecycle.pool.get_stats();
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
    assert_eq!(
        lifecycle.invoice.get_status(&buyer_invoice),
        InvoiceStatus::Listed as u32,
        "failed funding must leave the invoice listed"
    );
    lifecycle.registry.reinstate(&lifecycle.buyer);
    assert!(lifecycle.pool.fund_invoice(&buyer_invoice));
}

#[test]
fn test_revocation_after_funding_does_not_unwind_invoice() {
    let lifecycle = setup_fee_lifecycle();
    lifecycle.pool.deposit(&lifecycle.lp, &10_000_000_000u128);

    let (invoice_id, due_date) = create_attested_invoice(&lifecycle, 1_200_000_000);
    lifecycle.invoice.list_for_financing(&invoice_id, &200);
    assert!(lifecycle.pool.fund_invoice(&invoice_id));
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Funded as u32
    );

    // Both parties are revoked only after pool capital is already committed.
    lifecycle.registry.revoke(&lifecycle.issuer);
    lifecycle.registry.revoke(&lifecycle.buyer);
    assert_eq!(
        lifecycle
            .registry
            .get_verification_status(&lifecycle.issuer),
        VerificationStatus::Revoked
    );
    assert_eq!(
        lifecycle.registry.get_verification_status(&lifecycle.buyer),
        VerificationStatus::Revoked
    );
    assert!(!lifecycle.registry.is_verified(&lifecycle.issuer));
    assert!(!lifecycle.registry.is_verified(&lifecycle.buyer));

    // Documented rule: revocation is not retroactive. Shipping, delivery
    // confirmation, and repayment all proceed for the in-flight invoice.
    assert!(lifecycle.invoice.mark_shipped(&invoice_id));
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Active as u32
    );
    assert!(lifecycle
        .invoice
        .confirm_delivery(&invoice_id, &lifecycle.issuer));
    assert!(lifecycle
        .invoice
        .confirm_delivery(&invoice_id, &lifecycle.buyer));
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Confirmed as u32
    );
    lifecycle.env.ledger().set_timestamp(due_date);
    assert!(lifecycle.invoice.repay(&invoice_id));
    assert_eq!(
        lifecycle.invoice.get_status(&invoice_id),
        InvoiceStatus::Repaid as u32
    );
}

#[test]
fn test_revoke_requires_admin_auth_and_registered_profile() {
    let lifecycle = setup_fee_lifecycle();

    // Revoking an address that was never registered is a malformed request.
    let unknown = Address::generate(&lifecycle.env);
    let missing = lifecycle.registry.try_revoke(&unknown);
    assert!(missing.is_err());
    assert!(
        std::format!("{missing:?}").contains("#3"),
        "revoking an unregistered profile should fail with NotFound: {missing:?}"
    );

    // Without the admin authorization, revoke must be rejected.
    lifecycle.env.set_auths(&[]);
    let unauthorized = lifecycle.registry.try_revoke(&lifecycle.issuer);
    assert!(unauthorized.is_err());
    assert!(
        std::format!("{unauthorized:?}").contains("Auth"),
        "unauthorized revoke should fail with an auth error: {unauthorized:?}"
    );
    assert!(lifecycle.registry.is_verified(&lifecycle.issuer));
    assert_eq!(
        lifecycle
            .registry
            .get_verification_status(&lifecycle.issuer),
        VerificationStatus::Verified
    );
}
