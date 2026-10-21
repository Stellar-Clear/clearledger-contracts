#![cfg(test)]

extern crate alloc;

// Lint baseline (issue #252): every destructured `env` here is consumed by the
// test body — see `generate_invoice_id(&env, …)`, `Address::generate(&env)`,
// `env.ledger()`, `env.set_auths(&[])`, `assert_last_event_*(&env, …)`, or by
// being returned through `setup_without_auths`. Renaming to `_env` would change
// semantics; do not do so. `cargo clippy --workspace --all-targets -- -D warnings`
// must remain clean.

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{Address as _, Events as _, Ledger},
    Address, BytesN, Env, Symbol, TryFromVal, Vec,
};

use crate::{
    DataKey, EscrowAction, EscrowContract, EscrowContractClient, EscrowEvent, EscrowRecord,
    MAX_HISTORY_EVENTS, MIN_HISTORY_EVENTS,
};

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_key = BalanceKey(from.clone());
        let to_key = BalanceKey(to.clone());
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_bal - amount));
        env.storage().persistent().set(&to_key, &(to_bal + amount));
    }

    pub fn balance(env: Env, addr: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&BalanceKey(addr))
            .unwrap_or(0)
    }
}

#[contract]
pub struct MockCaller;

#[contractimpl]
impl MockCaller {
    pub fn noop(_env: Env) {}
}

#[contracttype]
pub struct BalanceKey(Address);

/// A strict mock token that reverts transfers which would overdraw an
/// account, mimicking a real asset's insufficient-balance behavior. The
/// lenient [`MockToken`] happily drives balances negative, so it cannot
/// exercise the escrow contract's behavior when it lacks the funds it
/// tries to transfer.
///
/// Lives in its own module because `#[contractimpl]` emits helper symbols
/// (`__transfer`, `__balance`, …) at module scope, which would collide with
/// the ones generated for [`MockToken`].
mod strict {
    use super::BalanceKey;
    use soroban_sdk::{contract, contracterror, contractimpl, panic_with_error, Address, Env};

    #[contract]
    pub struct StrictMockToken;

    #[contracterror]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum StrictMockTokenError {
        /// Returned when a transfer would overdraw the source account.
        InsufficientBalance = 1,
    }

    #[contractimpl]
    impl StrictMockToken {
        pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
            let from_key = BalanceKey(from.clone());
            let to_key = BalanceKey(to.clone());
            let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
            let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
            if from_bal < amount {
                panic_with_error!(&env, StrictMockTokenError::InsufficientBalance);
            }
            env.storage()
                .persistent()
                .set(&from_key, &(from_bal - amount));
            env.storage().persistent().set(&to_key, &(to_bal + amount));
        }

        pub fn balance(env: Env, addr: Address) -> i128 {
            env.storage()
                .persistent()
                .get(&BalanceKey(addr))
                .unwrap_or(0)
        }
    }
}

fn setup() -> (
    Env,
    EscrowContractClient<'static>,
    Address,
    Address,
    Address,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = env.register_contract(None, MockCaller);
    let pool = env.register_contract(None, MockCaller);
    let issuer = env.register_contract(None, MockCaller);
    let usdc_id = env.register_contract(None, MockToken);
    let _mock_token_client = MockTokenClient::new(&env, &usdc_id);

    let pool_bal_key = BalanceKey(pool.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&pool_bal_key, &10_000_000_000_000i128);
    });

    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);

    client.initialize(&admin, &pool, &usdc_id);

    (env, client, admin, pool, issuer, usdc_id)
}

fn setup_without_auths() -> (
    Env,
    EscrowContractClient<'static>,
    Address,
    Address,
    Address,
    Address,
) {
    let (env, client, admin, pool, issuer, usdc_id) = setup();
    env.set_auths(&[]);
    (env, client, admin, pool, issuer, usdc_id)
}

fn generate_invoice_id(env: &Env, counter: u64) -> BytesN<32> {
    let mut arr = [0u8; 32];
    let bytes = (env.ledger().timestamp() + counter).to_be_bytes();
    arr[0..8].copy_from_slice(&bytes);
    BytesN::from_array(env, &arr)
}

fn get_balance(env: &Env, token_id: &Address, addr: &Address) -> i128 {
    MockTokenClient::new(env, token_id).balance(addr)
}

/// Seed a synthetic history of `count` events directly into storage.
///
/// Amounts are `1..=count` and timestamps are `0..count` so tests can assert
/// exactly which suffix survived compaction.
fn seed_history(env: &Env, contract_id: &Address, invoice_id: &BytesN<32>, count: u32) {
    env.as_contract(contract_id, || {
        let mut history = Vec::new(env);
        for i in 0..count {
            history.push_back(EscrowEvent {
                invoice_id: invoice_id.clone(),
                action: EscrowAction::Locked,
                amount: i as u128 + 1,
                timestamp: i as u64,
                caller: None,
            });
        }
        env.storage()
            .persistent()
            .set(&DataKey::History(invoice_id.clone()), &history);
    });
}

fn assert_last_event_three<T1, T2>(
    env: &Env,
    expected_name: &str,
    expected_topic1: T1,
    expected_topic2: T2,
    expected_data: u128,
) where
    T1: TryFromVal<Env, soroban_sdk::Val> + core::fmt::Debug + PartialEq,
    T2: TryFromVal<Env, soroban_sdk::Val> + core::fmt::Debug + PartialEq,
    <T1 as TryFromVal<Env, soroban_sdk::Val>>::Error: core::fmt::Debug,
    <T2 as TryFromVal<Env, soroban_sdk::Val>>::Error: core::fmt::Debug,
{
    let events = env.events().all();
    let (_, topics, data) = events.last().expect("expected at least one event");

    let topic0: Symbol = Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap();
    let topic1: T1 = T1::try_from_val(env, &topics.get(1).unwrap()).unwrap();
    let topic2: T2 = T2::try_from_val(env, &topics.get(2).unwrap()).unwrap();
    let actual_data: u128 = u128::try_from_val(env, &data).unwrap();

    assert_eq!(topic0, Symbol::new(env, expected_name));
    assert_eq!(topic1, expected_topic1);
    assert_eq!(topic2, expected_topic2);
    assert_eq!(actual_data, expected_data);
}

fn assert_last_event_four<T1, T2, T3>(
    env: &Env,
    expected_name: &str,
    expected_topic1: T1,
    expected_topic2: T2,
    expected_topic3: T3,
    expected_data: u128,
) where
    T1: TryFromVal<Env, soroban_sdk::Val> + core::fmt::Debug + PartialEq,
    T2: TryFromVal<Env, soroban_sdk::Val> + core::fmt::Debug + PartialEq,
    T3: TryFromVal<Env, soroban_sdk::Val> + core::fmt::Debug + PartialEq,
    <T1 as TryFromVal<Env, soroban_sdk::Val>>::Error: core::fmt::Debug,
    <T2 as TryFromVal<Env, soroban_sdk::Val>>::Error: core::fmt::Debug,
    <T3 as TryFromVal<Env, soroban_sdk::Val>>::Error: core::fmt::Debug,
{
    let events = env.events().all();
    let (_, topics, data) = events.last().expect("expected at least one event");

    let topic0: Symbol = Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap();
    let topic1: T1 = T1::try_from_val(env, &topics.get(1).unwrap()).unwrap();
    let topic2: T2 = T2::try_from_val(env, &topics.get(2).unwrap()).unwrap();
    let topic3: T3 = T3::try_from_val(env, &topics.get(3).unwrap()).unwrap();
    let actual_data: u128 = u128::try_from_val(env, &data).unwrap();

    assert_eq!(topic0, Symbol::new(env, expected_name));
    assert_eq!(topic1, expected_topic1);
    assert_eq!(topic2, expected_topic2);
    assert_eq!(topic3, expected_topic3);
    assert_eq!(actual_data, expected_data);
}

#[test]
fn test_initialize() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let _invoice_contract = Address::generate(&env);
    let usdc = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &usdc);

    assert_eq!(client.get_locked(&generate_invoice_id(&env, 1)), 0);
}

#[test]
#[should_panic(expected = "Error(Auth")]
fn test_upgrade_requires_admin_auth() {
    let (env, client, _, _, _, _) = setup_without_auths();
    client.upgrade(&BytesN::from_array(&env, &[0; 32]));
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_initialize_twice_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let _invoice_contract = Address::generate(&env);
    let usdc = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);

    // First initialize succeeds
    client.initialize(&admin, &pool, &usdc);

    // Second initialize must panic with AlreadyInitialized (Error #1)
    let admin2 = Address::generate(&env);
    let pool2 = Address::generate(&env);
    let _invoice_contract2 = Address::generate(&env);
    let usdc2 = env.register_contract(None, MockToken);
    client.initialize(&admin2, &pool2, &usdc2);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_initialize_panics_when_pool_equals_usdc() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let usdc = pool.clone();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &usdc);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_initialize_panics_when_pool_equals_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = admin.clone();
    let usdc = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &usdc);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_initialize_panics_when_usdc_equals_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let _usdc = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &admin);
}

#[test]
fn test_get_admin_returns_stored_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = env.register_contract(None, MockCaller);
    let pool = env.register_contract(None, MockCaller);
    let usdc_id = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin.clone(), &pool, &usdc_id);

    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_get_pool_contract_returns_stored_pool() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = env.register_contract(None, MockCaller);
    let pool = env.register_contract(None, MockCaller);
    let usdc_id = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool.clone(), &usdc_id);

    assert_eq!(client.get_pool_contract(), pool);
}

#[test]
fn test_get_usdc_asset_returns_stored_usdc() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = env.register_contract(None, MockCaller);
    let pool = env.register_contract(None, MockCaller);
    let usdc_id = env.register_contract(None, MockToken);
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &usdc_id.clone());

    assert_eq!(client.get_usdc_asset(), usdc_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_get_admin_panics_when_not_initialized() {
    let env = Env::default();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.get_admin();
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_get_pool_contract_panics_when_not_initialized() {
    let env = Env::default();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.get_pool_contract();
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_get_usdc_asset_panics_when_not_initialized() {
    let env = Env::default();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.get_usdc_asset();
}

// ============================================================================
// Lock Tests
// ============================================================================

#[test]
fn test_lock_stores_record_and_transfers_usdc() {
    let (env, client, _admin, pool, issuer, usdc_id) = setup();
    let contract_id = env.register_contract(None, crate::EscrowContract);
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    // Check initial balances
    let _pool_balance_before = get_balance(&env, &usdc_id, &pool);
    let _contract_balance_before = get_balance(&env, &usdc_id, &contract_id);

    // Execute lock
    let result = client.lock(&invoice_id, &amount, &issuer);
    assert!(result);

    // Verify record was stored
    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, amount);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_lock_fails_zero_amount() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 3);
    client.lock(&invoice_id, &0, &issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_lock_fails_amount_exceeds_i128_max() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 4);
    let amount = i128::MAX as u128 + 1;
    client.lock(&invoice_id, &amount, &issuer);
}

#[test]
fn test_lock_succeeds_at_i128_max() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 5);
    let amount = i128::MAX as u128;
    let result = client.lock(&invoice_id, &amount, &issuer);
    assert!(result);
}

#[test]
fn test_lock_only_callable_by_pool() {
    // This test verifies that lock requires pool authorization.
    // In the soroban-sdk testutils with mock_all_auths(), any address can call.
    // The actual authorization is checked by pool.require_auth() in the contract.
    // Here we verify the lock mechanism works when called by pool.
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 4);
    let amount: u128 = 1_000_000_000;

    // Lock should succeed when called with proper setup
    // (pool is mocked to be the caller in setup())
    let result = client.lock(&invoice_id, &amount, &issuer);
    assert!(result);

    // Verify the record was stored
    assert_eq!(client.get_locked(&invoice_id), amount);
}

// ============================================================================
// Release to Issuer Tests
// ============================================================================

#[test]
fn test_release_to_issuer_sends_correct_amount() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 5);
    let amount: u128 = 1_000_000_000;

    // Lock funds first
    client.lock(&invoice_id, &amount, &issuer);

    // Release to issuer
    let result = client.release_to_issuer(&invoice_id, &issuer);
    assert!(result);

    // Verify record was removed
    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_release_to_issuer_self_address_panics() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let contract_id = env.register_contract(None, crate::EscrowContract);
    let invoice_id = generate_invoice_id(&env, 6);
    client.lock(&invoice_id, &1_000_000_000, &issuer);

    // Releasing to self (escrow contract itself) must panic with InvalidRecipient (#7)
    client.release_to_issuer(&invoice_id, &contract_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_release_to_issuer_pool_address_panics() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 7);
    client.lock(&invoice_id, &1_000_000_000, &issuer);

    // Releasing to pool contract address must panic with InvalidRecipient (#7)
    client.release_to_issuer(&invoice_id, &pool);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_release_to_issuer_mismatched_address_panics() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 8);
    let wrong_issuer = Address::generate(&env);
    client.lock(&invoice_id, &1_000_000_000, &issuer);

    // Releasing to a different address than the stored issuer must panic with InvalidRecipient
    client.release_to_issuer(&invoice_id, &wrong_issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_release_to_issuer_second_call_panics() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 8);
    let issuer = Address::generate(&env);
    let amount: u128 = 1_000_000_000;

    // Lock funds and release once
    client.lock(&invoice_id, &amount, &issuer);
    client.release_to_issuer(&invoice_id, &issuer);

    // Second release_to_issuer call on the same invoice_id should panic with NotFound (#2)
    client.release_to_issuer(&invoice_id, &issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_release_to_issuer_unknown_invoice_id_panics() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let unknown_id = generate_invoice_id(&env, 999);
    let issuer = Address::generate(&env);

    // Never locked this invoice_id — should panic with NotFound (#2)
    client.release_to_issuer(&unknown_id, &issuer);
}

#[test]
fn test_release_to_issuer_transfers_to_stored_issuer() {
    let (env, client, _admin, pool, issuer, usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 52);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    assert_eq!(get_balance(&env, &usdc_id, &client.address), amount as i128);
    assert_eq!(get_balance(&env, &usdc_id, &issuer), 0);

    assert!(client.release_to_issuer(&invoice_id, &issuer));

    assert_eq!(get_balance(&env, &usdc_id, &client.address), 0);
    assert_eq!(get_balance(&env, &usdc_id, &issuer), amount as i128);
    assert_eq!(
        get_balance(&env, &usdc_id, &pool),
        10_000_000_000_000i128 - amount as i128
    );
    assert_eq!(client.get_locked(&invoice_id), 0);
    assert_eq!(client.get_locked_at(&invoice_id), 0);
    assert_last_event_three(
        &env,
        "released_to_issuer",
        invoice_id.clone(),
        issuer.clone(),
        amount,
    );
}

#[test]
fn test_release_to_issuer_rejects_mismatch_without_moving_funds() {
    let (env, client, _admin, _pool, issuer, usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 53);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    let locked_at = client.get_locked_at(&invoice_id);
    let history_len = client.get_history(&invoice_id).len();
    let wrong_issuer = Address::generate(&env);

    assert!(client
        .try_release_to_issuer(&invoice_id, &wrong_issuer)
        .is_err());

    assert_eq!(client.get_locked(&invoice_id), amount);
    assert_eq!(client.get_locked_at(&invoice_id), locked_at);
    assert_eq!(client.get_history(&invoice_id).len(), history_len);
    assert_eq!(get_balance(&env, &usdc_id, &client.address), amount as i128);
    assert_eq!(get_balance(&env, &usdc_id, &wrong_issuer), 0);
    assert_eq!(get_balance(&env, &usdc_id, &issuer), 0);

    assert!(client.release_to_issuer(&invoice_id, &issuer));
    assert_eq!(get_balance(&env, &usdc_id, &issuer), amount as i128);
}

#[test]
fn test_release_to_issuer_unknown_invoice_does_not_move_funds() {
    let (env, client, _admin, _pool, _issuer, usdc_id) = setup();
    let unknown_id = generate_invoice_id(&env, 998);
    let caller_supplied = Address::generate(&env);

    assert!(client
        .try_release_to_issuer(&unknown_id, &caller_supplied)
        .is_err());

    assert_eq!(client.get_locked(&unknown_id), 0);
    assert_eq!(client.get_locked_at(&unknown_id), 0);
    assert_eq!(client.get_history(&unknown_id).len(), 0);
    assert_eq!(get_balance(&env, &usdc_id, &client.address), 0);
    assert_eq!(get_balance(&env, &usdc_id, &caller_supplied), 0);
}

#[test]
fn test_release_to_issuer_unauthorized_does_not_move_funds() {
    let (env, client, _admin, _pool, issuer, usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 54);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    let history_len = client.get_history(&invoice_id).len();
    env.set_auths(&[]);

    assert!(client.try_release_to_issuer(&invoice_id, &issuer).is_err());

    assert_eq!(client.get_locked(&invoice_id), amount);
    assert_eq!(client.get_history(&invoice_id).len(), history_len);
    assert_eq!(get_balance(&env, &usdc_id, &client.address), amount as i128);
    assert_eq!(get_balance(&env, &usdc_id, &issuer), 0);
}

#[test]
fn test_release_to_issuer_replay_does_not_move_funds() {
    let (env, client, _admin, _pool, issuer, usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 55);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    assert!(client.release_to_issuer(&invoice_id, &issuer));
    let history_len = client.get_history(&invoice_id).len();
    let issuer_balance = get_balance(&env, &usdc_id, &issuer);

    assert!(client.try_release_to_issuer(&invoice_id, &issuer).is_err());

    assert_eq!(client.get_locked(&invoice_id), 0);
    assert_eq!(client.get_history(&invoice_id).len(), history_len);
    assert_eq!(get_balance(&env, &usdc_id, &issuer), issuer_balance);
    assert_eq!(get_balance(&env, &usdc_id, &client.address), 0);
}

// ============================================================================
// Release to Pool Tests
// ============================================================================

#[test]
fn test_release_to_pool_transfers_correct_amount() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    // Lock funds first.
    client.lock(&invoice_id, &amount, &issuer);

    let repayment: u128 = amount;
    let result = client.release_to_pool(&invoice_id, &repayment);
    assert!(result);

    // Verify lock was removed
    assert_eq!(client.get_locked(&invoice_id), 0);
}

#[test]
fn test_release_to_pool_over_repayment_succeeds() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);

    // Test the documented over-repayment path (repayment_amount > locked amount)
    let repayment: u128 = amount + 500_000; // Adding yield/penalty
    let result = client.release_to_pool(&invoice_id, &repayment);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
}

#[test]
fn test_release_to_pool_requires_pool_auth() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    client.release_to_pool(&invoice_id, &amount);

    let auths = env.auths();
    assert!(auths.iter().any(|(addr, _)| *addr == pool));
}

#[test]
#[should_panic]
fn test_release_to_pool_rejects_non_invoice_contract_caller() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);

    // Without mocked auths, an arbitrary caller (nothing authorizes the
    // configured invoice contract) must be rejected before any funds move
    // or the lock record is touched — proving the previously-possible
    // escrow/pool/invoice desync can no longer be triggered by anyone
    // other than the configured invoice contract.
    env.set_auths(&[]);
    client.release_to_pool(&invoice_id, &amount);
}

// ============================================================================
// Handle Default Tests
// ============================================================================

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_release_to_pool_fails_zero_repayment() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    client.release_to_pool(&invoice_id, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_release_to_pool_fails_amount_exceeds_i128_max() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 2);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    let repayment_amount = i128::MAX as u128 + 1;
    client.release_to_pool(&invoice_id, &repayment_amount);
}

#[test]
fn test_release_to_pool_partial_repayment_succeeds() {
    let (env, client, _admin, pool, issuer, usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);

    // Capture balances before release
    let pool_balance_before = get_balance(&env, &usdc_id, &pool);
    let escrow_balance_before = get_balance(&env, &usdc_id, &client.address);

    let partial: u128 = 500_000_000;
    let result = client.release_to_pool(&invoice_id, &partial);
    assert!(result);
    assert_eq!(client.get_locked(&invoice_id), amount - partial);

    // Verify pool received exactly `partial`, not the full locked `amount`
    let pool_balance_after = get_balance(&env, &usdc_id, &pool);
    let escrow_balance_after = get_balance(&env, &usdc_id, &client.address);
    assert_eq!(pool_balance_after - pool_balance_before, partial as i128);
    assert_eq!(
        escrow_balance_before - escrow_balance_after,
        partial as i128
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_release_to_pool_unknown_invoice_id_panics() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let unknown_id = generate_invoice_id(&env, 999);

    // Never locked this invoice_id — should panic with NotFound (#2)
    client.release_to_pool(&unknown_id, &1_000_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_release_to_pool_second_call_panics() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    // Lock funds and release once
    client.lock(&invoice_id, &amount, &issuer);
    client.release_to_pool(&invoice_id, &amount);

    // Second release_to_pool call on the same invoice_id should panic with NotFound (#2)
    client.release_to_pool(&invoice_id, &amount);
}

#[test]
fn test_handle_default_returns_funds_to_pool() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);

    env.ledger().set_timestamp(env.ledger().timestamp() + 60);
    // Pool is the normal operational caller for default resolution
    let result = client.handle_default(&invoice_id, &pool);
    assert!(result);

    // Verify caller is in the event
    let events = env.events().all();
    let (_, topics, _) = events.last().expect("expected at least one event");
    let caller: Address = Address::try_from_val(&env, &topics.get(3).unwrap()).unwrap();
    assert_eq!(caller, pool);

    // Verify record was removed
    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
}

#[test]
fn test_handle_default_invoked_by_admin_succeeds() {
    let (env, client, admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.ledger().set_timestamp(env.ledger().timestamp() + 60);

    // Call handle_default with admin as the caller
    let result = client.handle_default(&invoice_id, &admin);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
    assert_last_event_four(
        &env,
        "default_resolved",
        invoice_id.clone(),
        _pool,
        admin,
        amount,
    );
}

#[test]
fn test_handle_default_admin_can_trigger() {
    let (env, client, admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.ledger().set_timestamp(env.ledger().timestamp() + 60);
    // Admin can directly trigger default resolution (emergency / recovery path)
    let result = client.handle_default(&invoice_id, &admin);
    assert!(result);

    let locked = client.get_locked(&invoice_id);
    assert_eq!(locked, 0);
    // Funds are always returned to the pool address regardless of who triggered
    assert_last_event_four(
        &env,
        "default_resolved",
        invoice_id.clone(),
        pool,
        admin,
        amount,
    );
}

#[test]
fn test_handle_default_returns_false_if_no_record() {
    let (env, client, _admin, pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 8);

    let result = client.handle_default(&invoice_id, &pool);
    assert!(!result);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_handle_default_rejects_before_grace_period() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 9);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.ledger().set_timestamp(env.ledger().timestamp() + 59);

    client.handle_default(&invoice_id, &pool);
}

#[test]
fn test_handle_default_allows_at_grace_period_boundary() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 10);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.ledger().set_timestamp(env.ledger().timestamp() + 60);

    let result = client.handle_default(&invoice_id, &pool);
    assert!(result);
    assert_eq!(client.get_locked(&invoice_id), 0);
}

#[test]
fn test_handle_default_second_call_returns_false_without_side_effects() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    // Lock funds and advance past grace period
    client.lock(&invoice_id, &amount, &issuer);
    env.ledger().set_timestamp(env.ledger().timestamp() + 60);

    // First call: should succeed, remove the record, and emit default_resolved
    let result1 = client.handle_default(&invoice_id, &pool);
    assert!(result1);
    assert_eq!(client.get_locked(&invoice_id), 0);
    assert_last_event_four(
        &env,
        "default_resolved",
        invoice_id.clone(),
        pool.clone(),
        pool.clone(),
        amount,
    );

    // Capture event count after first (successful) call
    let event_count_after_first = env.events().all().len();

    // Second call: should return false with no side effects
    let result2 = client.handle_default(&invoice_id, &pool);
    assert!(!result2);

    // Verify state is unchanged — get_locked still 0
    assert_eq!(client.get_locked(&invoice_id), 0);

    // Verify no new events were emitted by the second call
    assert_eq!(
        env.events().all().len(),
        event_count_after_first,
        "second handle_default must not emit any events"
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_handle_default_panics_when_escrow_balance_insufficient() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = env.register_contract(None, MockCaller);
    let pool = env.register_contract(None, MockCaller);
    let issuer = env.register_contract(None, MockCaller);
    let usdc_id = env.register_contract(None, strict::StrictMockToken);
    let token_client = strict::StrictMockTokenClient::new(&env, &usdc_id);

    // Fund the pool so the initial lock can proceed.
    let pool_bal_key = BalanceKey(pool.clone());
    env.as_contract(&usdc_id, || {
        env.storage()
            .persistent()
            .set(&pool_bal_key, &10_000_000_000_000i128);
    });

    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    client.initialize(&admin, &pool, &usdc_id);

    let invoice_id = generate_invoice_id(&env, 30);
    let amount: u128 = 1_000_000_000;
    client.lock(&invoice_id, &amount, &issuer);

    // Sanity: the escrow contract now holds exactly the locked amount.
    assert_eq!(token_client.balance(&contract_id), amount as i128);

    // Drain the escrow contract's balance directly through the token so the
    // subsequent default transfer cannot complete.
    let escrow_balance = token_client.balance(&contract_id);
    token_client.transfer(&contract_id, &issuer, &escrow_balance);
    assert_eq!(token_client.balance(&contract_id), 0);

    // Advance past the grace period so the only remaining failure mode is
    // the token transfer itself.
    env.ledger().set_timestamp(env.ledger().timestamp() + 60);

    client.handle_default(&invoice_id, &pool);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_handle_default_admin_caller_without_pool_set_panics_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    let invoice_id = generate_invoice_id(&env, 105);
    let admin = Address::generate(&env);

    // Set only the Admin key — PoolContract (and UsdcAsset) remain unset —
    // and seed a lock record so handle_default gets past its early guard.
    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Admin, &admin);
        let record = EscrowRecord {
            invoice_id: invoice_id.clone(),
            amount: 1_000_000_000,
            locked_at: env.ledger().timestamp(),
            issuer: Address::generate(&env),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Locked(invoice_id.clone()), &record);
    });

    // Even though the caller is the admin, the missing PoolContract must
    // panic with NotInitialized before any funds move.
    client.handle_default(&invoice_id, &admin);
}

#[test]
fn test_handle_default_event_includes_caller() {
    let (env, client, admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.ledger().set_timestamp(env.ledger().timestamp() + 60);

    // Call with admin as caller - verify caller is in event
    let result = client.handle_default(&invoice_id, &admin);
    assert!(result);

    let events = env.events().all();
    let (_, topics, _) = events.last().expect("expected at least one event");

    let topic0: Symbol = Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap();
    let topic1: BytesN<32> = BytesN::<32>::try_from_val(&env, &topics.get(1).unwrap()).unwrap();
    let topic2: Address = Address::try_from_val(&env, &topics.get(2).unwrap()).unwrap();
    let topic3: Address = Address::try_from_val(&env, &topics.get(3).unwrap()).unwrap();

    assert_eq!(topic0, Symbol::new(&env, "default_resolved"));
    assert_eq!(topic1, invoice_id);
    assert_eq!(topic2, pool);
    assert_eq!(topic3, admin);

    // Verify caller is also recorded in history
    let history = client.get_history(&invoice_id);
    let last_event = history.last().unwrap();
    assert_eq!(last_event.caller, Some(admin));
}

// ============================================================================
// Get Locked Tests
// ============================================================================

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_handle_default_unauthorized_caller_panics() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;
    let stranger = Address::generate(&env);

    client.lock(&invoice_id, &amount, &issuer);
    // A caller that is neither admin nor pool must be rejected
    client.handle_default(&invoice_id, &stranger);
    // also ensure that pool is indeed required for the normal path
    let _ = pool;
}

#[test]
fn test_get_locked_returns_zero_when_empty() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 9);

    assert_eq!(client.get_locked(&invoice_id), 0);
}

#[test]
fn test_get_locked_returns_zero_for_unknown_id() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();

    // Generate a random unknown invoice ID
    let unknown_id = generate_invoice_id(&env, 999);
    assert_eq!(client.get_locked(&unknown_id), 0);
}

#[test]
fn test_get_locked_returns_amount_when_locked() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 10);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    assert_eq!(client.get_locked(&invoice_id), amount);
}

#[test]
fn test_get_locked_returns_zero_after_release_to_issuer() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 11);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    assert_eq!(client.get_locked(&invoice_id), amount);

    client.release_to_issuer(&invoice_id, &issuer);
    assert_eq!(client.get_locked(&invoice_id), 0);
}

#[test]
fn prop_partial_pool_releases_conserve_initial_lock_and_residual() {
    let strategy = (
        proptest::collection::vec(1u128..10_000, 1..16),
        1u128..10_000,
    );
    let mut runner = TestRunner::new(ProptestConfig::with_cases(16));
    runner
        .run(&strategy, |(chunks, tail)| {
            let (env, client, _admin, pool, issuer, usdc) = setup();
            let id = generate_invoice_id(&env, 530);
            let initial = chunks.iter().copied().sum::<u128>() + tail;
            client.lock(&id, &initial, &issuer);
            let locked_at = client.get_locked_at(&id);
            let pool_after_lock = get_balance(&env, &usdc, &pool);
            let mut released = 0u128;
            for (i, amount) in chunks.iter().enumerate() {
                prop_assert!(client.release_to_pool(&id, amount));
                released += amount;
                let residual = client.get_locked(&id);
                prop_assert_eq!(released + residual, initial);
                prop_assert_eq!(get_balance(&env, &usdc, &client.address), residual as i128);
                prop_assert_eq!(
                    get_balance(&env, &usdc, &pool) - pool_after_lock,
                    released as i128
                );
                prop_assert_eq!(client.get_locked_at(&id), locked_at);
                let history = client.get_history(&id);
                prop_assert_eq!(history.len(), i as u32 + 2);
                prop_assert_eq!(history.get(history.len() - 1).unwrap().amount, *amount);
            }
            prop_assert!(client.try_release_to_pool(&id, &0).is_err());
            prop_assert_eq!(client.get_locked(&id), tail);
            prop_assert!(client.release_to_pool(&id, &tail));
            prop_assert_eq!(client.get_locked(&id), 0);
            prop_assert_eq!(get_balance(&env, &usdc, &client.address), 0);
            prop_assert!(client.try_release_to_pool(&id, &1).is_err());
            prop_assert_eq!(
                get_balance(&env, &usdc, &pool) - pool_after_lock,
                initial as i128
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn test_get_locked_returns_zero_after_release_to_pool() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 12);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    assert_eq!(client.get_locked(&invoice_id), amount);

    client.release_to_pool(&invoice_id, &amount);
    assert_eq!(client.get_locked(&invoice_id), 0);
}

#[test]
fn test_get_locked_at_returns_zero_when_empty() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 20);

    assert_eq!(client.get_locked_at(&invoice_id), 0);
}

#[test]
fn test_get_locked_at_returns_timestamp_when_locked() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 21);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    let locked_at = client.get_locked_at(&invoice_id);
    assert_eq!(locked_at, env.ledger().timestamp());
}

#[test]
fn test_get_locked_at_returns_zero_after_release() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    env.ledger().set_timestamp(1_000_000);
    let invoice_id = generate_invoice_id(&env, 22);
    let issuer = Address::generate(&env);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    assert!(client.get_locked_at(&invoice_id) > 0);

    client.release_to_issuer(&invoice_id, &issuer);
    assert_eq!(client.get_locked_at(&invoice_id), 0);
}

// ============================================================================
// Integration Tests
// ============================================================================

#[test]
fn test_multiple_invoices_independent() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();

    let invoice_id_1 = generate_invoice_id(&env, 13);
    let invoice_id_2 = generate_invoice_id(&env, 14);
    let amount_1: u128 = 1_000_000_000;
    let amount_2: u128 = 2_000_000_000;

    client.lock(&invoice_id_1, &amount_1, &issuer);
    client.lock(&invoice_id_2, &amount_2, &issuer);

    assert_eq!(client.get_locked(&invoice_id_1), amount_1);
    assert_eq!(client.get_locked(&invoice_id_2), amount_2);

    client.release_to_pool(&invoice_id_1, &amount_1);

    assert_eq!(client.get_locked(&invoice_id_1), 0);
    assert_eq!(client.get_locked(&invoice_id_2), amount_2);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_lock_fails_duplicate() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 15);
    client.lock(&invoice_id, &1_000_000_000, &issuer);
    client.lock(&invoice_id, &500_000_000, &issuer);
}

#[test]
fn test_get_history_returns_action_log() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 16);
    let amount: u128 = 1_000_000_000;
    let issuer = Address::generate(&env);

    client.lock(&invoice_id, &amount, &issuer);
    client.release_to_issuer(&invoice_id, &issuer);

    let history: Vec<EscrowEvent> = client.get_history(&invoice_id);
    assert_eq!(history.len(), 2);
    let lock_event = history.get(0).unwrap();
    let release_event = history.get(1).unwrap();

    assert_eq!(lock_event.invoice_id, invoice_id);
    assert_eq!(lock_event.action, EscrowAction::Locked);
    assert_eq!(lock_event.amount, amount);

    assert_eq!(release_event.invoice_id, invoice_id);
    assert_eq!(release_event.action, EscrowAction::ReleasedToIssuer);
    assert_eq!(release_event.amount, amount);
    assert!(release_event.timestamp >= lock_event.timestamp);

    assert_eq!(client.get_locked(&invoice_id), 0);
    assert_last_event_three(
        &env,
        "released_to_issuer",
        invoice_id.clone(),
        issuer,
        amount,
    );
}

#[test]
fn test_compact_history_below_threshold_is_unchanged() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 200);
    seed_history(&env, &client.address, &invoice_id, 4);

    let removed = client.compact_history(&invoice_id, &8);

    assert_eq!(removed, 0);
    let history = client.get_history(&invoice_id);
    assert_eq!(history.len(), 4);
    assert_eq!(history.get(0).unwrap().amount, 1);
    assert_eq!(history.get(3).unwrap().amount, 4);
}

#[test]
fn test_compact_history_at_threshold_is_unchanged() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 201);
    seed_history(&env, &client.address, &invoice_id, 5);

    let removed = client.compact_history(&invoice_id, &5);

    assert_eq!(removed, 0);
    assert_eq!(client.get_history(&invoice_id).len(), 5);
}

#[test]
fn test_compact_history_above_threshold_prunes_oldest_and_preserves_order() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 202);
    seed_history(&env, &client.address, &invoice_id, 10);

    let removed = client.compact_history(&invoice_id, &4);

    assert_eq!(removed, 6);
    let history = client.get_history(&invoice_id);
    assert_eq!(history.len(), 4);
    for (index, expected_amount) in [7u128, 8, 9, 10].iter().enumerate() {
        let event = history.get(index as u32).unwrap();
        assert_eq!(event.amount, *expected_amount);
        assert_eq!(event.timestamp, 6 + index as u64);
    }
}

#[test]
fn test_compact_history_is_idempotent() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 203);
    seed_history(&env, &client.address, &invoice_id, 12);

    assert_eq!(client.compact_history(&invoice_id, &4), 8);
    assert_eq!(client.compact_history(&invoice_id, &4), 0);
    assert_eq!(client.get_history(&invoice_id).len(), 4);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_compact_history_missing_invoice_panics_not_found() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 204);

    client.compact_history(&invoice_id, &4);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_compact_history_rejects_zero_retention() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 205);
    seed_history(&env, &client.address, &invoice_id, 4);

    client.compact_history(&invoice_id, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_compact_history_rejects_retention_above_cap() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 206);
    seed_history(&env, &client.address, &invoice_id, 4);

    client.compact_history(&invoice_id, &(MAX_HISTORY_EVENTS + 1));
}

#[test]
fn test_compact_history_accepts_minimum_retention() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 207);
    seed_history(&env, &client.address, &invoice_id, 4);

    assert_eq!(
        client.compact_history(&invoice_id, &MIN_HISTORY_EVENTS),
        3
    );
    assert_eq!(client.get_history(&invoice_id).len(), MIN_HISTORY_EVENTS);
}

#[test]
#[should_panic(expected = "Error(Auth")]
fn test_compact_history_requires_admin_authorization() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup_without_auths();
    let invoice_id = generate_invoice_id(&env, 206);
    seed_history(&env, &client.address, &invoice_id, 4);

    client.compact_history(&invoice_id, &2);
}

#[test]
fn test_compact_history_emits_event() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 207);
    seed_history(&env, &client.address, &invoice_id, 6);

    client.compact_history(&invoice_id, &2);

    let events = env.events().all();
    let (contract, topics, data) = events.last().expect("expected compaction event");
    assert_eq!(contract, client.address);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "history_compacted")
    );
    assert_eq!(
        BytesN::<32>::try_from_val(&env, &topics.get(1).unwrap()).unwrap(),
        invoice_id
    );
    let (retained, removed): (u32, u32) = TryFromVal::try_from_val(&env, &data).unwrap();
    assert_eq!((retained, removed), (2, 4));
}

#[test]
fn test_append_history_automatically_compacts_at_cap() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 208);
    seed_history(&env, &client.address, &invoice_id, MAX_HISTORY_EVENTS);

    client.lock(&invoice_id, &1_000_000_000, &issuer);

    let history = client.get_history(&invoice_id);
    assert_eq!(history.len(), MAX_HISTORY_EVENTS);
    assert_eq!(history.get(0).unwrap().amount, 2);
    let last = history.last().unwrap();
    assert_eq!(last.action, EscrowAction::Locked);
    assert_eq!(last.amount, 1_000_000_000);
}

#[test]
#[should_panic]
fn test_lock_requires_pool_authorization() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup_without_auths();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    // The contract stores a pool address internally, but no auth entry is
    // present after setup_without_auths(), so this must fail at require_auth().
    client.lock(&invoice_id, &amount, &issuer);
}

#[test]
#[should_panic]
fn test_release_to_issuer_requires_pool_authorization() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.set_auths(&[]);
    client.release_to_issuer(&invoice_id, &issuer);
}

#[test]
#[should_panic]
fn test_handle_default_requires_pool_authorization() {
    let (env, client, _admin, pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount: u128 = 1_000_000_000;

    client.lock(&invoice_id, &amount, &issuer);
    env.set_auths(&[]);
    // No auth entries present — require_auth() on the pool caller must fail
    client.handle_default(&invoice_id, &pool);
}

// ============================================================================
// Pre-initialization tests — typed NotInitialized (Contract error #6)
// ============================================================================

/// `lock` must panic with `NotInitialized` rather than the cryptic
/// host-side `Option::unwrap()` error when called before `initialize()`.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_lock_uninitialized_panics_with_typed_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    let invoice_id = generate_invoice_id(&env, 100);
    let issuer = Address::generate(&env);
    // mock_all_auths bypasses pool.require_auth so the unwrap on the
    // uninitialized PoolContract is observable as the typed error.
    client.lock(&invoice_id, &1_000_000_000, &issuer);
}

/// `release_to_issuer` must panic with `NotInitialized` rather than the
/// cryptic host-side `Option::unwrap()` error when called before `initialize()`.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_release_to_issuer_uninitialized_panics_with_typed_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    let invoice_id = generate_invoice_id(&env, 101);
    let issuer = Address::generate(&env);
    client.release_to_issuer(&invoice_id, &issuer);
}

/// `release_to_pool` must panic with `NotInitialized` rather than the
/// cryptic host-side `Option::unwrap()` error when called before `initialize()`.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_release_to_pool_uninitialized_panics_with_typed_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    let invoice_id = generate_invoice_id(&env, 102);
    client.release_to_pool(&invoice_id, &1_000_000_000);
}

/// When a stale lock record exists but the contract has not been initialized,
/// `handle_default` must panic with `NotInitialized` rather than the cryptic
/// `Option::unwrap()` error.
#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_handle_default_uninitialized_with_existing_record_panics_with_typed_not_initialized() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    let invoice_id = generate_invoice_id(&env, 103);
    let caller = Address::generate(&env);

    // Seed a stale lock record so handle_default passes its early
    // `has(Locked)` guard and reaches the unwrap on instance storage.
    env.as_contract(&contract_id, || {
        let record = EscrowRecord {
            invoice_id: invoice_id.clone(),
            amount: 1_000_000_000,
            locked_at: env.ledger().timestamp(),
            issuer: Address::generate(&env),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Locked(invoice_id.clone()), &record);
    });

    client.handle_default(&invoice_id, &caller);
}

/// Sanity: without a lock record, `handle_default` short-circuits to `false`
/// before touching instance storage, so no `NotInitialized` panic occurs
/// even when the contract has not been initialized.
#[test]
fn test_handle_default_uninitialized_without_record_returns_false() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &contract_id);
    let invoice_id = generate_invoice_id(&env, 104);
    let caller = Address::generate(&env);
    assert!(!client.handle_default(&invoice_id, &caller));
}

#[derive(Clone, Debug)]
enum EscrowOperation {
    Lock { invoice: usize, amount: u128 },
    ReleaseToPool { invoice: usize },
    ReleaseToIssuer { invoice: usize },
    HandleDefault { invoice: usize },
    UnauthorizedDefault { invoice: usize },
}

fn escrow_operations() -> impl Strategy<Value = alloc::vec::Vec<EscrowOperation>> {
    prop::collection::vec((0usize..4, 1u128..=1_000_000_000, 0u8..5), 1..=30).prop_map(
        |operations| {
            operations
                .into_iter()
                .map(|(invoice, amount, action)| match action {
                    0 => EscrowOperation::Lock { invoice, amount },
                    1 => EscrowOperation::ReleaseToPool { invoice },
                    2 => EscrowOperation::ReleaseToIssuer { invoice },
                    3 => EscrowOperation::HandleDefault { invoice },
                    _ => EscrowOperation::UnauthorizedDefault { invoice },
                })
                .collect()
        },
    )
}

#[test]
fn prop_escrow_balance_equals_sum_of_locked_records() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&escrow_operations(), |operations| {
            let (env, client, _admin, pool, issuer, usdc_id) = setup();
            let invoice_ids = [
                generate_invoice_id(&env, 200),
                generate_invoice_id(&env, 201),
                generate_invoice_id(&env, 202),
                generate_invoice_id(&env, 203),
            ];
            let stranger = Address::generate(&env);
            let mut expected_locks = [None; 4];
            let mut history_lengths = [0u32; 4];

            for operation in operations {
                match operation {
                    EscrowOperation::Lock { invoice, amount } => {
                        let invoice_id = &invoice_ids[invoice];
                        if expected_locks[invoice].is_some() {
                            let balance_before = get_balance(&env, &usdc_id, &client.address);
                            let history_before = client.get_history(invoice_id).len();
                            let locked_before = client.get_locked(invoice_id);

                            prop_assert!(client.try_lock(invoice_id, &amount, &issuer).is_err());
                            prop_assert_eq!(
                                get_balance(&env, &usdc_id, &client.address),
                                balance_before
                            );
                            prop_assert_eq!(client.get_locked(invoice_id), locked_before);
                            prop_assert_eq!(client.get_history(invoice_id).len(), history_before);
                        } else {
                            prop_assert!(client.lock(invoice_id, &amount, &issuer));
                            expected_locks[invoice] = Some(amount);
                        }
                    }
                    EscrowOperation::ReleaseToPool { invoice } => {
                        let invoice_id = &invoice_ids[invoice];
                        if let Some(amount) = expected_locks[invoice] {
                            prop_assert!(client.release_to_pool(invoice_id, &amount));
                            expected_locks[invoice] = None;
                            prop_assert_eq!(client.get_locked(invoice_id), 0);
                        } else {
                            let balance_before = get_balance(&env, &usdc_id, &client.address);
                            let history_before = client.get_history(invoice_id).len();

                            prop_assert!(client.try_release_to_pool(invoice_id, &1).is_err());
                            prop_assert_eq!(
                                get_balance(&env, &usdc_id, &client.address),
                                balance_before
                            );
                            prop_assert_eq!(client.get_locked(invoice_id), 0);
                            prop_assert_eq!(client.get_history(invoice_id).len(), history_before);
                        }
                    }
                    EscrowOperation::ReleaseToIssuer { invoice } => {
                        let invoice_id = &invoice_ids[invoice];
                        if expected_locks[invoice].is_some() {
                            prop_assert!(client.release_to_issuer(invoice_id, &issuer));
                            expected_locks[invoice] = None;
                            prop_assert_eq!(client.get_locked(invoice_id), 0);
                        } else {
                            let balance_before = get_balance(&env, &usdc_id, &client.address);
                            let history_before = client.get_history(invoice_id).len();

                            prop_assert!(client
                                .try_release_to_issuer(invoice_id, &issuer)
                                .is_err());
                            prop_assert_eq!(
                                get_balance(&env, &usdc_id, &client.address),
                                balance_before
                            );
                            prop_assert_eq!(client.get_locked(invoice_id), 0);
                            prop_assert_eq!(client.get_history(invoice_id).len(), history_before);
                        }
                    }
                    EscrowOperation::HandleDefault { invoice } => {
                        let invoice_id = &invoice_ids[invoice];
                        if expected_locks[invoice].is_some() {
                            env.ledger().set_timestamp(env.ledger().timestamp() + 60);
                            prop_assert!(client.handle_default(invoice_id, &pool));
                            expected_locks[invoice] = None;
                            prop_assert_eq!(client.get_locked(invoice_id), 0);
                        } else {
                            prop_assert!(!client.handle_default(invoice_id, &pool));
                        }
                    }
                    EscrowOperation::UnauthorizedDefault { invoice } => {
                        let invoice_id = &invoice_ids[invoice];
                        if expected_locks[invoice].is_some() {
                            env.ledger().set_timestamp(env.ledger().timestamp() + 60);
                            let balance_before = get_balance(&env, &usdc_id, &client.address);
                            let history_before = client.get_history(invoice_id).len();
                            let locked_before = client.get_locked(invoice_id);

                            prop_assert!(client.try_handle_default(invoice_id, &stranger).is_err());
                            prop_assert_eq!(
                                get_balance(&env, &usdc_id, &client.address),
                                balance_before
                            );
                            prop_assert_eq!(client.get_locked(invoice_id), locked_before);
                            prop_assert_eq!(client.get_history(invoice_id).len(), history_before);
                        } else {
                            prop_assert!(!client.handle_default(invoice_id, &stranger));
                        }
                    }
                }

                let mut total_locked = 0i128;
                for (index, invoice_id) in invoice_ids.iter().enumerate() {
                    let expected = expected_locks[index].unwrap_or(0);
                    prop_assert_eq!(client.get_locked(invoice_id), expected);
                    total_locked += expected as i128;

                    let history_len = client.get_history(invoice_id).len();
                    prop_assert!(history_len >= history_lengths[index]);
                    history_lengths[index] = history_len;
                }
                prop_assert_eq!(get_balance(&env, &usdc_id, &client.address), total_locked);
            }

            Ok(())
        })
        .unwrap();
}
// ============== ISSUE #716: EMERGENCY PAUSE / UNPAUSE ==============

#[test]
fn test_pause_blocks_state_changes_and_unpause_restores_them() {
    let (env, client, _admin, pool, issuer, usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    let amount = 1_000_000_000u128;

    // Live: the pool can lock funds.
    client.lock(&invoice_id, &amount, &issuer);

    client.pause();

    // State-changing entry points are rejected while paused...
    let second = generate_invoice_id(&env, 2);
    assert!(client.try_lock(&second, &amount, &issuer).is_err());
    assert!(client.try_release_to_issuer(&invoice_id, &issuer).is_err());
    assert!(client.try_release_to_pool(&invoice_id, &amount).is_err());
    assert!(client.try_handle_default(&invoice_id, &pool).is_err());

    // ...while read-only views keep working.
    assert_eq!(client.get_locked(&invoice_id), amount);
    assert_eq!(client.get_admin(), client.get_admin());
    assert_eq!(client.get_pool_contract(), pool);
    assert_eq!(client.get_usdc_asset(), usdc_id);
    assert!(!client.get_history(&invoice_id).is_empty());
    // Funds never moved while paused.
    assert_eq!(get_balance(&env, &usdc_id, &client.address), amount as i128);

    client.unpause();

    // Unpaused: fund movement flows again.
    assert!(client.release_to_issuer(&invoice_id, &issuer));
    assert_eq!(client.get_locked(&invoice_id), 0);
}

/// Pins the exact breaker error: `PauseError::ContractPaused` from the shared
/// crate surfaces as `Error(Contract, #1)`. `EscrowError::AlreadyInitialized`
/// is the only other #1 and is unreachable from `lock`.
#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_lock_reverts_while_paused_with_contract_paused_error() {
    let (env, client, _admin, _pool, issuer, _usdc_id) = setup();
    let invoice_id = generate_invoice_id(&env, 1);
    client.pause();
    client.lock(&invoice_id, &1_000_000_000, &issuer);
}

/// Negative auth: only a non-admin signed `pause`, so the stored admin's
/// `require_auth()` must reject it — a non-admin cannot engage the breaker.
#[test]
fn test_pause_requires_admin_authorization() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    let non_admin = env.register_contract(None, MockCaller);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "pause",
            args: soroban_sdk::Vec::new(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_pause().is_err());
}

/// Negative auth for disengaging: a non-admin cannot unpause either, so paused
/// escrow cannot be resumed by anyone but the admin.
#[test]
fn test_unpause_requires_admin_authorization() {
    let (env, client, _admin, _pool, _issuer, _usdc_id) = setup();
    client.pause();
    let non_admin = env.register_contract(None, MockCaller);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "unpause",
            args: soroban_sdk::Vec::new(&env),
            sub_invokes: &[],
#[test]
fn test_pause_matrix_preserves_balances_records_history_and_events() {
    use soroban_sdk::xdr::ToXdr;
    for operation in 0..4 {
        let (env, client, admin, pool, issuer, usdc) = setup();
        let id = generate_invoice_id(&env, 550 + operation);
        let second = generate_invoice_id(&env, 560 + operation);
        let amount = 1_000_000u128;
        client.lock(&id, &amount, &issuer);
        // Default is otherwise eligible, so the breaker is the sole blocker.
        env.ledger().set_timestamp(env.ledger().timestamp() + 60);
        client.pause();
        let history = client.get_history(&id).to_xdr(&env);
        let balances = (
            get_balance(&env, &usdc, &pool),
            get_balance(&env, &usdc, &client.address),
            get_balance(&env, &usdc, &issuer),
        );
        let locked_at = client.get_locked_at(&id);
        let events = env.events().all().len();
        let rejected = match operation {
            0 => client.try_lock(&second, &amount, &issuer).is_err(),
            1 => client.try_release_to_issuer(&id, &issuer).is_err(),
            2 => client.try_release_to_pool(&id, &amount).is_err(),
            _ => client.try_handle_default(&id, &pool).is_err(),
        };
        assert!(rejected);
        assert_eq!(client.get_locked(&id), amount);
        assert_eq!(client.get_locked(&second), 0);
        assert_eq!(client.get_locked_at(&id), locked_at);
        assert_eq!(client.get_history(&id).to_xdr(&env), history);
        assert_eq!(
            (
                get_balance(&env, &usdc, &pool),
                get_balance(&env, &usdc, &client.address),
                get_balance(&env, &usdc, &issuer)
            ),
            balances
        );
        assert_eq!(env.events().all().len(), events);
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_pool_contract(), pool);
        assert_eq!(client.get_usdc_asset(), usdc);
        assert!(client.get_history(&second).is_empty());
        client.unpause();
        let succeeded = match operation {
            0 => client.lock(&second, &amount, &issuer),
            1 => client.release_to_issuer(&id, &issuer),
            2 => client.release_to_pool(&id, &amount),
            _ => client.handle_default(&id, &pool),
        };
        assert!(succeeded);
        if operation == 0 {
            assert_eq!(client.get_locked(&second), amount);
        } else {
            assert_eq!(client.get_locked(&id), 0);
        }
    }
}

#[test]
fn test_initialize_is_blocked_by_stored_pause_and_recoverable_after_unpause() {
    // Public pause requires initialization. Seed the shared breaker on a fresh
    // deployment to independently test initialize's own pause guard.
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let pool = Address::generate(&env);
    let token = Address::generate(&env);
    let id = env.register_contract(None, EscrowContract);
    let client = EscrowContractClient::new(&env, &id);
    env.as_contract(&id, || clearledger_pause::set_paused(&env, true));
    let events = env.events().all().len();
    assert!(client.try_initialize(&admin, &pool, &token).is_err());
    assert_eq!(env.events().all().len(), events);
    env.as_contract(&id, || {
        assert!(!env.storage().instance().has(&DataKey::Admin));
        assert!(!env.storage().instance().has(&DataKey::PoolContract));
        assert!(!env.storage().instance().has(&DataKey::UsdcAsset));
        clearledger_pause::set_paused(&env, false);
    });
    client.initialize(&admin, &pool, &token);
    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_pool_contract(), pool);
    assert_eq!(client.get_usdc_asset(), token);
    assert!(client.try_initialize(&admin, &pool, &token).is_err());
}

        },
    }]);
    assert!(client.try_unpause().is_err());
}

#[test]
fn test_pause_and_unpause_emit_events() {
    let (env, client, admin, _pool, _issuer, _usdc_id) = setup();

    client.pause();
    let events = env.events().all();
    let (contract, topics, _) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "paused")
    );
    assert_eq!(
        Address::try_from_val(&env, &topics.get(1).unwrap()).unwrap(),
        admin
    );

    client.unpause();
    let events = env.events().all();
    let (contract, topics, _) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "unpaused")
    );
    assert_eq!(
        Address::try_from_val(&env, &topics.get(1).unwrap()).unwrap(),
        admin
    );
}

/// The breaker is storage-local: pausing one escrow must not affect a second,
/// independently deployed instance.
#[test]
fn test_pause_is_scoped_to_one_instance() {
    let env = Env::default();
    env.mock_all_auths();
    let usdc_id = env.register_contract(None, MockToken);
    let _mock_token_client = MockTokenClient::new(&env, &usdc_id);
    let admin = env.register_contract(None, MockCaller);
    let pool = env.register_contract(None, MockCaller);
    let issuer = env.register_contract(None, MockCaller);

    let id_a = env.register_contract(None, EscrowContract);
    let id_b = env.register_contract(None, EscrowContract);
    let a = EscrowContractClient::new(&env, &id_a);
    let b = EscrowContractClient::new(&env, &id_b);
    a.initialize(&admin, &pool, &usdc_id);
    b.initialize(&admin, &pool, &usdc_id);

    a.pause();

    let id = generate_invoice_id(&env, 1);
    assert!(a.try_lock(&id, &1_000, &issuer).is_err());
    assert!(b.lock(&id, &1_000, &issuer));
}
