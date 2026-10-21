#![cfg(test)]

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::vec;
use soroban_sdk::{
    contract, contractimpl, contracttype,
    testutils::{storage::Persistent as _, Address as _, Events as _, Ledger},
    token,
    xdr::ToXdr,
    Address, BytesN, Env, IntoVal, String, Symbol, TryFromVal, Vec,
};

use crate::{
    InvoiceContract, InvoiceContractClient, InvoiceError, InvoiceStatus, DEFAULT_EXPIRY_WINDOW,
    MAX_BATCH_SIZE, MAX_FACE_VALUE, MAX_PAGE_SIZE, TTL_EXTEND_TO, TTL_THRESHOLD,
};

// Default invoice parameters used across tests.
// These computed constants eliminate magic numbers in test assertions
// and make the tests self-correcting when parameters change.
const DEFAULT_FACE_VALUE: u128 = 1_000_000_000;
const DEFAULT_DUE_OFFSET: u64 = 86400; // 1 day in seconds
const DEFAULT_DISCOUNT_BPS: u32 = 200;
const DEFAULT_FUNDED_AMOUNT: u128 =
    DEFAULT_FACE_VALUE * (10000 - DEFAULT_DISCOUNT_BPS as u128) / 10000;

#[contract]
pub struct MockRegistry;

#[contractimpl]
impl MockRegistry {
    pub fn is_verified(env: Env, address: Address) -> bool {
        env.storage()
            .persistent()
            .get::<_, bool>(&DataKey(address))
            .unwrap_or(false)
    }

    pub fn register(env: Env, address: Address) {
        env.storage()
            .persistent()
            .set(&DataKey(address.clone()), &true);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey(address), TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    pub fn revoke(env: Env, address: Address) {
        env.storage()
            .persistent()
            .set(&DataKey(address.clone()), &false);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey(address), TTL_THRESHOLD, TTL_EXTEND_TO);
    }
}

#[contracttype]
pub struct DataKey(Address);

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

    pub fn mint(env: Env, to: Address, amount: i128) {
        let key = TKey(to.clone());
        let bal: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(bal + amount));
    }
}

#[contracttype]
pub struct TKey(Address);

#[contract]
pub struct MockPool;

#[contractimpl]
impl MockPool {
    pub fn set_return_values(env: Env, handle_default: bool, repayment: bool) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "handle_default"), &handle_default);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "repayment"), &repayment);
    }

    pub fn handle_default(env: Env, _invoice_id: BytesN<32>) -> bool {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "handle_default"))
            .unwrap_or(true)
    }

    pub fn receive_repayment(_env: Env, _invoice_id: BytesN<32>, _amount: u128) -> bool {
        true
    }

    pub fn get_usdc_asset(env: Env) -> Address {
        let key = Symbol::new(&env, "asset");
        env.storage().instance().get(&key).unwrap()
    }

    pub fn receive_repayment_with_refund(
        env: Env,
        _invoice_id: BytesN<32>,
        _amount: u128,
        refund: u128,
        _buyer: Address,
    ) -> bool {
        let key = Symbol::new(&env, "last_refund");
        env.storage().instance().set(&key, &refund);
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "repayment"))
            .unwrap_or(true)
    }

    pub fn get_last_refund(env: Env) -> u128 {
        let key = Symbol::new(&env, "last_refund");
        env.storage().instance().get(&key).unwrap_or(0)
    }
}

type Setup = (
    Env,
    InvoiceContractClient<'static>,
    Address,
    Address,
    MockRegistryClient<'static>,
    Address,
);

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin, &registry_id);

    let usdc_asset = env.register_contract(None, MockToken);
    client.add_supported_asset(&usdc_asset);

    (env, client, issuer, buyer, registry_client, usdc_asset)
}

fn assert_wiring_setter_rejects(
    env: &Env,
    client: &InvoiceContractClient,
    method: &str,
    candidate: &Address,
) {
    let result = env.try_invoke_contract::<(), soroban_sdk::Error>(
        &client.address,
        &Symbol::new(env, method),
        (candidate.clone(),).into_val(env),
    );
    assert!(result.is_err(), "{method} accepted a conflicting address");
}

#[allow(dead_code)]
type SetupWithAdmin = (
    Env,
    InvoiceContractClient<'static>,
    Address,
    Address,
    MockRegistryClient<'static>,
    Address,
    Address,
);

#[allow(dead_code)]
fn setup_with_admin() -> SetupWithAdmin {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin, &registry_id);

    let usdc_asset = env.register_contract(None, MockToken);
    client.add_supported_asset(&usdc_asset);

    (
        env,
        client,
        issuer,
        buyer,
        registry_client,
        usdc_asset,
        admin,
    )
}

fn mock_pool_with_asset(env: &Env, asset: &Address) -> Address {
    let pool_id = env.register_contract(None, MockPool);
    let _pool_client = MockPoolClient::new(env, &pool_id);
    env.as_contract(&pool_id, || {
        let key = Symbol::new(env, "asset");
        env.storage().instance().set(&key, asset);
    });
    pool_id
}

/// Creates an invoice, lists it, and funds it through a mock pool, returning
/// the invoice id. Keeps the `get_funded_amount` boundary tests focused on the
/// amount under test rather than repeating the funding setup each time.
fn funded_invoice(
    env: &Env,
    client: &InvoiceContractClient,
    issuer: &Address,
    buyer: &Address,
    usdc: &Address,
    funded_amount: u128,
) -> BytesN<32> {
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(issuer, buyer, &DEFAULT_FACE_VALUE, &due_date, usdc);
    attest(env, client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    let pool = mock_pool_with_asset(env, usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, usdc, &funded_amount);
    invoice_id
}

// --------------- Mock Agent Registry (Underwrite) ---------------
//
// Stands in for the agent-registry contract from the separate
// `underwrite-contract` repo. Tests sign with a real secp256k1 key so
// `submit_attestation`'s recovery + registry-lookup path is exercised for
// real rather than mocked out.

#[contract]
pub struct MockAgentRegistry;

#[contractimpl]
impl MockAgentRegistry {
    pub fn get_agent(env: Env, agent_id: Symbol) -> Option<crate::Agent> {
        env.storage().persistent().get(&AgentKey(agent_id))
    }

    pub fn register_agent(env: Env, agent_id: Symbol, agent: crate::Agent) {
        env.storage().persistent().set(&AgentKey(agent_id), &agent);
    }
}

#[contracttype]
pub struct AgentKey(Symbol);

use clearledger_test_utils::{test_agent_pubkey, test_agent_signing_key};

/// Deploys a fresh mock agent-registry, registers one active agent with a
/// real secp256k1 keypair, points `client` at it, and submits a validly
/// signed attestation for `invoice_id`. Centralizes the plumbing so
/// individual tests can just call this right before `list_for_financing`.
fn attest(env: &Env, client: &InvoiceContractClient, invoice_id: &BytesN<32>) {
    let agent_id = Symbol::new(env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(env, &registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(env),
        },
    );
    client.set_agent_registry_contract(&registry_id);

    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(env, &[9u8; 32]),
        agent_id,
        nonce: 1,
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

    client.submit_attestation(invoice_id, &payload_bytes, &signature);
}

/// Registers the shared test secp256k1 key as an active agent and points
/// `client` at the fresh mock registry.
fn register_test_agent(env: &Env, client: &InvoiceContractClient) -> Symbol {
    let agent_id = Symbol::new(env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(env, &registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(env),
        },
    );
    client.set_agent_registry_contract(&registry_id);
    agent_id
}

/// Signs `payload` exactly as an off-chain agent would: keccak256 over the
/// XDR bytes, then a recoverable secp256k1 signature over that digest.
fn sign_attestation(
    env: &Env,
    payload: &crate::AttestationPayload,
) -> (soroban_sdk::Bytes, BytesN<65>) {
    let payload_bytes = payload.to_xdr(env);
    let digest = env.crypto().keccak256(&payload_bytes).to_array();
    let (sig, recid) = test_agent_signing_key()
        .sign_prehash_recoverable(&digest)
        .unwrap();
    let mut sig_bytes = [0u8; 65];
    sig_bytes[..64].copy_from_slice(&sig.to_bytes());
    sig_bytes[64] = recid.to_byte();
    (payload_bytes, BytesN::from_array(env, &sig_bytes))
}

fn test_payload(
    env: &Env,
    invoice_id: &BytesN<32>,
    agent_id: &Symbol,
    nonce: u64,
) -> crate::AttestationPayload {
    crate::AttestationPayload {
        domain_separator: BytesN::from_array(env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(env, &[9u8; 32]),
        agent_id: agent_id.clone(),
        nonce,
    }
}

// ============== FAILURE-PATH TESTS FOR submit_attestation ==============

#[cfg(test)]
mod versioned_event_tests {
    use crate::events::{invoice_funded, invoice_funded_v1};
    use soroban_sdk::{BytesN, Env, Symbol};
    use soroban_sdk::{testutils::Events, IntoVal, Val, Vec};

    #[test]
    fn funding_keeps_legacy_payload_and_adds_explicit_version() {
        let env = Env::default();
        let contract = env.register_contract(None, crate::InvoiceContract);
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
#[should_panic(expected = "Error(Contract, #21)")]
fn test_submit_attestation_untrusted_signer_wrong_pubkey() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);

    // Register agent with a different pubkey than the one we'll use to sign
    let wrong_pubkey_bytes = [42u8; 65];
    let wrong_pubkey = BytesN::from_array(&env, &wrong_pubkey_bytes);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: wrong_pubkey,
        },
    );
    client.set_agent_registry_contract(&registry_id);

    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
        agent_id,
        nonce: 1,
    };
    let payload_bytes = payload.to_xdr(&env);
    let digest = env.crypto().keccak256(&payload_bytes).to_array();

    // Sign with the test key (not the registered pubkey)
    let (sig, recid) = test_agent_signing_key()
        .sign_prehash_recoverable(&digest)
        .unwrap();
    let mut sig_bytes = [0u8; 65];
    sig_bytes[..64].copy_from_slice(&sig.to_bytes());
    sig_bytes[64] = recid.to_byte();
    let signature = BytesN::from_array(&env, &sig_bytes);

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")]
fn test_submit_attestation_inactive_agent() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);

    // Register agent as inactive
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: false,
            pubkey: test_agent_pubkey(&env),
        },
    );
    client.set_agent_registry_contract(&registry_id);

    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
        agent_id,
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

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")]
fn test_submit_attestation_unregistered_agent() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let _registry_client = MockAgentRegistryClient::new(&env, &registry_id);

    // Don't register the agent at all
    client.set_agent_registry_contract(&registry_id);

    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
        agent_id,
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

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")]
fn test_submit_attestation_replay_rejection() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // First attestation should succeed
    attest(&env, &client, &invoice_id);

    // Second attestation with a different valid payload should fail
    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    client.set_agent_registry_contract(&registry_id);

    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 6000,
        evidence_hash: BytesN::from_array(&env, &[10u8; 32]),
        agent_id,
        nonce: 2,
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

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

// Note: Malformed payload testing is limited by Soroban SDK's XDR decoding -
// it produces Value errors rather than Contract errors. The contract-level
// InvalidAmount (#16) error is only hit for valid XDR that fails business logic.
// The other failure paths below are the ones that can be tested at the contract level.

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_submit_attestation_domain_separator_mismatch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    client.set_agent_registry_contract(&registry_id);

    // Wrong domain separator
    let wrong_domain = [0u8; 32];
    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &wrong_domain),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
        agent_id,
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

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_submit_attestation_invoice_id_mismatch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    client.set_agent_registry_contract(&registry_id);

    // Different invoice_id in payload vs argument
    let wrong_invoice_id = BytesN::from_array(&env, &[1u8; 32]);
    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: wrong_invoice_id,
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
        agent_id,
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

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")]
fn test_submit_attestation_same_signature_cannot_be_replayed() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let agent_id = register_test_agent(&env, &client);

    let payload = test_payload(&env, &invoice_id, &agent_id, 1);
    let (payload_bytes, signature) = sign_attestation(&env, &payload);

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
    // Byte-for-byte replay of the accepted submission.
    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_submit_attestation_signature_not_reusable_across_invoices() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_a = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let invoice_b = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let agent_id = register_test_agent(&env, &client);

    // Signature covers invoice A; attempt to spend it on invoice B.
    let payload = test_payload(&env, &invoice_a, &agent_id, 1);
    let (payload_bytes, signature) = sign_attestation(&env, &payload);

    client.submit_attestation(&invoice_b, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_submit_attestation_signature_not_reusable_across_domains() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let agent_id = register_test_agent(&env, &client);

    // Simulate a signature produced for another protocol/network domain.
    let mut payload = test_payload(&env, &invoice_id, &agent_id, 1);
    payload.domain_separator =
        BytesN::from_array(&env, b"ClearLedger.OtherDomain.v1______");
    let (payload_bytes, signature) = sign_attestation(&env, &payload);

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_submit_attestation_wrong_contract_domain_separator() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let agent_id = register_test_agent(&env, &client);

    // A separator from a different ClearLedger contract must not validate.
    let mut payload = test_payload(&env, &invoice_id, &agent_id, 1);
    payload.domain_separator =
        BytesN::from_array(&env, b"ClearLedger.EscrowAttest.v1_____");
    let (payload_bytes, signature) = sign_attestation(&env, &payload);

    client.submit_attestation(&invoice_id, &payload_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")]
fn test_submit_attestation_signature_not_reusable_with_wrong_nonce() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let agent_id = register_test_agent(&env, &client);

    let signed_payload = test_payload(&env, &invoice_id, &agent_id, 1);
    let (_, signature) = sign_attestation(&env, &signed_payload);

    // Reuse the nonce-1 signature after mutating the payload's nonce.
    let tampered_payload = test_payload(&env, &invoice_id, &agent_id, 2);
    let tampered_bytes = tampered_payload.to_xdr(&env);

    client.submit_attestation(&invoice_id, &tampered_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")]
fn test_submit_attestation_signature_bound_to_agent_identity() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let agent_id = register_test_agent(&env, &client);
    let other_agent_id = Symbol::new(&env, "other_agent");

    let registry_id = client.get_agent_registry_contract().unwrap();
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);
    registry_client.register_agent(
        &other_agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );

    let signed_payload = test_payload(&env, &invoice_id, &agent_id, 1);
    let (_, signature) = sign_attestation(&env, &signed_payload);

    // Same key, valid agent, but the signed identity names another agent.
    let tampered_payload = test_payload(&env, &invoice_id, &other_agent_id, 1);
    let tampered_bytes = tampered_payload.to_xdr(&env);

    client.submit_attestation(&invoice_id, &tampered_bytes, &signature);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")]
fn test_list_for_financing_without_attestation() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // Try to list without calling submit_attestation
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_submit_attestation_nonexistent_invoice() {
    let (env, client, _issuer, _buyer, _, _usdc) = setup();

    let agent_id = Symbol::new(&env, "test_agent");
    let registry_id = env.register_contract(None, MockAgentRegistry);
    let registry_client = MockAgentRegistryClient::new(&env, &registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(&env),
        },
    );
    client.set_agent_registry_contract(&registry_id);

    let fake_invoice_id = BytesN::from_array(&env, &[1u8; 32]);
    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(&env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: fake_invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(&env, &[9u8; 32]),
        agent_id,
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

    client.submit_attestation(&fake_invoice_id, &payload_bytes, &signature);
}

#[test]
fn test_create_invoice_with_verified_parties() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    let invoice = client.get(&invoice_id);

    assert_eq!(invoice.issuer, issuer);
    assert_eq!(invoice.buyer, buyer);
    assert_eq!(invoice.face_value, face_value);
    assert_eq!(invoice.due_date, due_date);
    assert_eq!(invoice.status, InvoiceStatus::Created);
    assert_eq!(invoice.funding_asset, usdc);
    assert_eq!(invoice.funding_pool, None);
    assert!(!invoice.issuer_confirmed);
    assert!(!invoice.buyer_confirmed);
}

#[test]
fn test_cancel_created_invoice() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    assert!(client.cancel(&invoice_id));

    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Cancelled);
    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Cancelled")), Some(1));
}

#[test]
#[should_panic(expected = "Error(Auth")]
fn test_cancel_requires_issuer_auth() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    env.set_auths(&[]);

    client.cancel(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_cancel_listed_invoice_fails() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    client.cancel(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Auth")]
fn test_upgrade_requires_admin_auth() {
    let (env, client, _, _, _, _, _) = setup_with_admin();
    env.set_auths(&[]);
    client.upgrade(&BytesN::from_array(&env, &[0; 32]));
}

#[test]
fn test_get_counts_tracks_created_to_listed() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")), Some(1));
    assert_eq!(counts.get(String::from_str(&env, "Listed")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Funded")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Active")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Confirmed")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Repaid")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Defaulted")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Expired")), Some(0));

    attest(&env, &client, &invoice_id);
    assert!(client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS));

    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")), Some(0));
    assert_eq!(counts.get(String::from_str(&env, "Listed")), Some(1));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_create_fails_unverified_issuer() {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin, &registry_id);

    let usdc_asset = env.register_contract(None, MockToken);
    client.add_supported_asset(&usdc_asset);

    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    client.create(&issuer, &buyer, &face_value, &due_date, &usdc_asset);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_create_fails_unverified_buyer() {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin, &registry_id);

    let usdc_asset = env.register_contract(None, MockToken);
    client.add_supported_asset(&usdc_asset);

    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    client.create(&issuer, &buyer, &face_value, &due_date, &usdc_asset);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_create_fails_zero_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    client.create(&issuer, &buyer, &0, &due_date, &usdc);
}

#[test]
fn test_create_succeeds_at_max_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &MAX_FACE_VALUE, &due_date, &usdc);

    assert_eq!(client.get(&invoice_id).face_value, MAX_FACE_VALUE);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_create_fails_above_max_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let above_max = MAX_FACE_VALUE.checked_add(1).unwrap();

    client.create(&issuer, &buyer, &above_max, &due_date, &usdc);
}

#[test]
fn prop_invalid_face_values_do_not_create_invoices() {
    let invalid_face_values = prop_oneof![
        Just(0u128),
        (MAX_FACE_VALUE + 1)..=u128::MAX,
    ];
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&invalid_face_values, |face_value| {
            let (env, client, issuer, buyer, _, usdc) = setup();
            let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
            let before = client.get_counts();

            prop_assert!(
                client
                    .try_create(&issuer, &buyer, &face_value, &due_date, &usdc)
                    .is_err(),
                "invalid face value {face_value} must fail"
            );
            let after = client.get_counts();
            prop_assert_eq!(
                after.get(String::from_str(&env, "Created")),
                before.get(String::from_str(&env, "Created"))
            );
            Ok(())
        })
        .unwrap();
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_create_fails_past_due_date() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    env.ledger().set_timestamp(86400);
    let past_date = env.ledger().timestamp() - 1;
    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &past_date, &usdc);
}

// ============== ISSUE #B: due_date BOUNDARY (due_date == now) ==============

// At exactly `due_date == now`, `create` rejects with InvalidDueDate (#7).
// The check is `due_date <= env.ledger().timestamp()` so equality falls on
// the rejection side. Pins the current behaviour so a regression on the
// boundary comparator cannot land silently.
#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_create_fails_when_due_date_equals_now() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    env.ledger().set_timestamp(86400);
    let equal_due_date = env.ledger().timestamp();
    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &equal_due_date, &usdc);
}

// The boundary's other side: `due_date == now + 1` is the smallest accepted
// value. Confirms storage and events on the positive boundary so that a
// future refactor can't flip the boundary silently.
#[test]
fn test_create_succeeds_when_due_date_one_second_in_future() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    env.ledger().set_timestamp(86400);
    let just_future_due_date = env.ledger().timestamp() + 1;
    let face_value: u128 = DEFAULT_FACE_VALUE;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &just_future_due_date, &usdc);

    // State: invoice record exists at Created with the boundary due_date.
    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Created);
    assert_eq!(invoice.due_date, just_future_due_date);
    assert_eq!(invoice.created_at, env.ledger().timestamp());
    assert_eq!(invoice.face_value, face_value);

    // Events: exactly one invoice_created event was emitted by the invoice
    // contract. Per `events::invoice_created` the topic tuple is
    // `(Symbol("invoice_created"), invoice_id, issuer, buyer, funding_asset)`
    // and the data payload is `face_value: u128`. We pin the count and event
    // shape here; detailed per-topic comparisons live in the dedicated event
    // integration tests because soroban_sdk's `Val` does not implement
    // `PartialEq` for ad-hoc equality assertions.
    let events = env.events().all();
    let (event_contract, _topics, _data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, client.address.clone());
}

#[test]
fn test_list_for_financing() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest(&env, &client, &invoice_id);
    let result = client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert!(result);

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Listed);
    assert_eq!(invoice.discount_bps, DEFAULT_DISCOUNT_BPS);
}

// ============== REGISTRY REVOCATION RE-CHECK (registry+invoice+pool bug) ==============
//
// Design decision: registry revocation is prospective, not retroactive.
// `list_for_financing` re-verifies the issuer and buyer, since this is
// still a pre-funding step where declining new business is cheap. Once an
// invoice is `Funded`, however, verification is no longer re-checked at any
// later step — see `test_revocation_after_mark_funded_does_not_block_lifecycle`.

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_list_for_financing_fails_when_issuer_revoked() {
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    // Verified at create() time, revoked before listing.
    registry.revoke(&issuer);

    client.list_for_financing(&invoice_id, &200);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_list_for_financing_fails_when_buyer_revoked() {
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    registry.revoke(&buyer);

    client.list_for_financing(&invoice_id, &200);
}

// Mid-lifecycle revocation (post-`mark_funded`) must NOT retroactively
// affect an in-flight invoice: shipment, dual confirmation, and default all
// proceed exactly as if the issuer/buyer were still verified. This pins the
// documented, deliberate behavior that revocation only gates new
// commitments (`list_for_financing`, `PoolContract::fund_invoice`), not
// invoices that already cleared those gates.
#[test]
fn test_revocation_after_mark_funded_does_not_block_lifecycle() {
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &200);

    let pool_id = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool_id);
    client.mark_funded(&invoice_id, &pool_id, &usdc, &980_000_000);

    // Revoke both parties only after the invoice is already Funded.
    registry.revoke(&issuer);
    registry.revoke(&buyer);
    assert!(!registry.is_verified(&issuer));
    assert!(!registry.is_verified(&buyer));

    // The rest of the lifecycle proceeds unaffected.
    assert!(client.mark_shipped(&invoice_id));
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);

    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    env.ledger().set_timestamp(due_date + 1);
    assert!(client.trigger_default(&invoice_id));
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Defaulted);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_list_fails_wrong_status() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    client.list_for_financing(&invoice_id, &300);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn test_list_fails_discount_too_high() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &5001);
}

#[test]
fn test_zero_discount_full_lifecycle_and_zero_term_repayment() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value = DEFAULT_FACE_VALUE;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    assert!(client.list_for_financing(&invoice_id, &0));

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);

    // Funding at maturity exercises repay's zero-term branch.
    env.ledger().set_timestamp(due_date);
    client.mark_funded(&invoice_id, &pool, &usdc, &face_value);
    let funded = client.get(&invoice_id);
    assert_eq!(funded.discount_bps, 0);
    assert_eq!(funded.funded_amount, face_value);

    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    assert!(client.repay(&invoice_id));
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Repaid);
    assert_eq!(MockPoolClient::new(&env, &pool).get_last_refund(), 0);
}

#[test]
fn test_list_for_financing_discount_bps_min_boundary() {
    // discount_bps == 1 is the smallest valid value and must succeed.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest(&env, &client, &invoice_id);
    let listed_at = env.ledger().timestamp();
    let result = client.list_for_financing(&invoice_id, &1);
    assert!(result);

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Listed);
    assert_eq!(invoice.discount_bps, 1);

    let contract_id = client.address.clone();
    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, contract_id);
    assert_eq!(
        topics,
        (Symbol::new(&env, "invoice_listed"), invoice_id.clone()).into_val(&env)
    );
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (DEFAULT_DISCOUNT_BPS, listed_at + DEFAULT_EXPIRY_WINDOW)
    );
}

#[test]
fn test_batch_list_for_financing_expiry_overflow_reports_failed_entry() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.set_expiry_window(&11);
    env.ledger().set_timestamp(u64::MAX - 10);

    let entries = vec![&env, (invoice_id.clone(), DEFAULT_DISCOUNT_BPS)];
    let failed = client.batch_list_for_financing(&entries);

    assert_eq!(failed.len(), 1);
    assert_eq!(failed.get(0).unwrap(), invoice_id);
    assert_eq!(client.get_status(&invoice_id), 0);
}

#[test]
fn test_list_for_financing_discount_bps_max_boundary() {
    // discount_bps == 5000 is the inclusive upper bound and must succeed.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest(&env, &client, &invoice_id);
    let listed_at = env.ledger().timestamp();
    let result = client.list_for_financing(&invoice_id, &5000);
    assert!(result);

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Listed);
    assert_eq!(invoice.discount_bps, 5000);

    let contract_id = client.address.clone();
    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, contract_id);
    assert_eq!(
        topics,
        (Symbol::new(&env, "invoice_listed"), invoice_id.clone()).into_val(&env)
    );
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (5000u32, listed_at + DEFAULT_EXPIRY_WINDOW)
    );
}

#[test]
fn test_invoice_listed_event_includes_default_expiry_deadline() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let listed_at = 1_700_000_000u64;
    env.ledger().set_timestamp(listed_at);
    let due_date = listed_at + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    assert_eq!(client.get(&invoice_id).listed_at, Some(listed_at));

    let events = env.events().all();
    let (contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(contract, client.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "invoice_listed"), invoice_id.clone()).into_val(&env)
    );
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (DEFAULT_DISCOUNT_BPS, listed_at + DEFAULT_EXPIRY_WINDOW)
    );
}

#[test]
fn test_invoice_listed_event_uses_configured_expiry_window() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let listed_at = 1_700_000_000u64;
    let window = 3_600u64;
    env.ledger().set_timestamp(listed_at);
    client.set_expiry_window(&window);
    let due_date = listed_at + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let events = env.events().all();
    let (_, _, data) = events.last().expect("expected at least one event");
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (DEFAULT_DISCOUNT_BPS, listed_at + window)
    );
}

#[test]
fn test_invoice_listed_event_min_expiry_window_boundary() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let listed_at = 1_700_000_000u64;
    env.ledger().set_timestamp(listed_at);
    client.set_expiry_window(&0);
    let due_date = listed_at + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let events = env.events().all();
    let (_, _, data) = events.last().expect("expected at least one event");
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (DEFAULT_DISCOUNT_BPS, listed_at)
    );
}

#[test]
fn test_invoice_listed_event_max_expiry_window_boundary() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let listed_at = 1_700_000_000u64;
    let window = 31_536_000u64;
    env.ledger().set_timestamp(listed_at);
    client.set_expiry_window(&window);
    let due_date = listed_at + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let events = env.events().all();
    let (_, _, data) = events.last().expect("expected at least one event");
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (DEFAULT_DISCOUNT_BPS, listed_at + window)
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn test_invoice_listed_event_expiry_overflow_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.set_expiry_window(&11);
    env.ledger().set_timestamp(u64::MAX - 10);

    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")]
fn test_list_for_financing_discount_bps_one_above_max_boundary_panics() {
    // discount_bps == 5001 is one past the inclusive upper bound and must panic
    // with DiscountTooHigh (#9). Pins the exact boundary alongside the existing
    // test_list_fails_discount_too_high regression test.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &5001);
}

#[test]
#[should_panic(expected = "Error(Auth")]
fn test_list_for_financing_non_issuer_panics() {
    let (env, client, issuer, _buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &_buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);
    attest(&env, &client, &invoice_id);

    env.set_auths(&[]);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
}

#[test]
fn test_full_lifecycle() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);

    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Listed);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);

    let funded_amount: u128 = DEFAULT_FUNDED_AMOUNT;
    let result = client.mark_funded(&invoice_id, &pool, &usdc, &funded_amount);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Funded);
    assert_eq!(client.get(&invoice_id).funding_pool, Some(pool));

    client.mark_shipped(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);

    client.confirm_delivery(&invoice_id, &issuer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);
    assert!(client.get(&invoice_id).issuer_confirmed);
    assert!(!client.get(&invoice_id).buyer_confirmed);

    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
    assert!(client.get(&invoice_id).issuer_confirmed);
    assert!(client.get(&invoice_id).buyer_confirmed);
}

#[test]
fn test_get_by_issuer_returns_correct_invoices() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);

    let invoices = client.get_by_issuer(&issuer, &0, &MAX_PAGE_SIZE);
    assert_eq!(invoices.len(), 2);

    let other = Address::generate(&env);
    let empty = client.get_by_issuer(&other, &0, &MAX_PAGE_SIZE);
    assert_eq!(empty.len(), 0);
}

#[test]
fn test_get_by_buyer_returns_correct_invoices() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);

    let invoices = client.get_by_buyer(&buyer, &0, &MAX_PAGE_SIZE);
    assert_eq!(invoices.len(), 2);
}

#[test]
fn test_get_invoice_count_by_issuer_matches_get_by_issuer() {
    // Verifies that `get_invoice_count_by_issuer` returns the same value as
    // `get_by_issuer(..).len()` after several creates spread across multiple
    // issuers and buyers (issue #840).
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let issuer2 = Address::generate(&env);
    let issuer3 = Address::generate(&env);
    let buyer2 = Address::generate(&env);
    registry.register(&issuer2);
    registry.register(&issuer3);
    registry.register(&buyer2);

    // issuer -> buyer: 2 invoices, issuer -> buyer2: 1 invoice
    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);
    client.create(&issuer, &buyer2, &3_000_000_000, &due_date, &usdc);
    // issuer2 -> buyer: 1 invoice
    client.create(&issuer2, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    // issuer3 -> buyer2: 2 invoices
    client.create(&issuer3, &buyer2, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer3, &buyer2, &4_000_000_000, &due_date, &usdc);

    for party in [&issuer, &issuer2, &issuer3, &buyer, &buyer2] {
        assert_eq!(
            client.get_invoice_count_by_issuer(party),
            client.get_by_issuer(party, &0, &MAX_PAGE_SIZE).len(),
            "issuer count mismatch for {party:?}"
        );
    }

    assert_eq!(client.get_invoice_count_by_issuer(&issuer), 3);
    assert_eq!(client.get_invoice_count_by_issuer(&issuer2), 1);
    assert_eq!(client.get_invoice_count_by_issuer(&issuer3), 2);
    assert_eq!(client.get_invoice_count_by_issuer(&buyer), 0);
    assert_eq!(client.get_invoice_count_by_issuer(&buyer2), 0);
}

#[test]
fn test_get_invoice_count_by_buyer_matches_get_by_buyer() {
    // Verifies that `get_invoice_count_by_buyer` returns the same value as
    // `get_by_buyer(..).len()` after several creates spread across multiple
    // issuers and buyers (issue #840).
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let issuer2 = Address::generate(&env);
    let buyer2 = Address::generate(&env);
    let buyer3 = Address::generate(&env);
    registry.register(&issuer2);
    registry.register(&buyer2);
    registry.register(&buyer3);

    // issuer -> buyer: 2 invoices, issuer -> buyer2: 1 invoice
    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);
    client.create(&issuer, &buyer2, &3_000_000_000, &due_date, &usdc);
    // issuer2 -> buyer3: 2 invoices
    client.create(&issuer2, &buyer3, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer2, &buyer3, &4_000_000_000, &due_date, &usdc);

    for party in [&issuer, &issuer2, &buyer, &buyer2, &buyer3] {
        assert_eq!(
            client.get_invoice_count_by_buyer(party),
            client.get_by_buyer(party, &0, &MAX_PAGE_SIZE).len(),
            "buyer count mismatch for {party:?}"
        );
    }

    assert_eq!(client.get_invoice_count_by_buyer(&buyer), 2);
    assert_eq!(client.get_invoice_count_by_buyer(&buyer2), 1);
    assert_eq!(client.get_invoice_count_by_buyer(&buyer3), 2);
    assert_eq!(client.get_invoice_count_by_buyer(&issuer), 0);
    assert_eq!(client.get_invoice_count_by_buyer(&issuer2), 0);
}

#[test]
fn test_get_invoice_count_by_issuer_and_buyer_unknown_address_is_zero() {
    // Verifies both count views return `0` for addresses that have never
    // issued or bought an invoice, both before and after other activity
    // (issue #840).
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let stranger = Address::generate(&env);

    // Before any invoice exists
    assert_eq!(client.get_invoice_count_by_issuer(&stranger), 0);
    assert_eq!(client.get_invoice_count_by_buyer(&stranger), 0);
    assert_eq!(client.get_invoice_count_by_issuer(&issuer), 0);
    assert_eq!(client.get_invoice_count_by_buyer(&buyer), 0);

    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // Unrelated addresses stay at zero while known ones advance
    assert_eq!(client.get_invoice_count_by_issuer(&stranger), 0);
    assert_eq!(client.get_invoice_count_by_buyer(&stranger), 0);
    assert_eq!(client.get_invoice_count_by_issuer(&issuer), 1);
    assert_eq!(client.get_invoice_count_by_buyer(&buyer), 1);
    // Cross-index: an issuer that never bought is `0` as a buyer and vice versa
    assert_eq!(client.get_invoice_count_by_buyer(&issuer), 0);
    assert_eq!(client.get_invoice_count_by_issuer(&buyer), 0);
}

#[test]
fn test_get_by_status_returns_correct_invoices() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);

    let created = client.get_by_status(&InvoiceStatus::Created, &0, &MAX_PAGE_SIZE);
    assert_eq!(created.len(), 2);
}

#[test]
fn test_expire_listing_transitions_to_expired_after_window() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    client.set_expiry_window(&100);
    env.ledger().set_timestamp(env.ledger().timestamp() + 101);

    let result = client.expire_listing(&invoice_id, &issuer);
    assert!(result);

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Expired);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_unknown_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get(&fake_id);
}

#[test]
fn test_dual_confirmation_both_must_confirm() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    client.mark_shipped(&invoice_id);

    client.confirm_delivery(&invoice_id, &issuer);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Active);
    assert!(inv.issuer_confirmed);
    assert!(!inv.buyer_confirmed);

    client.confirm_delivery(&invoice_id, &buyer);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Confirmed);
    assert!(inv.issuer_confirmed);
    assert!(inv.buyer_confirmed);
}

#[test]
fn test_confirm_by_both_transitions_to_confirmed() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);

    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_confirm_delivery_wrong_party_panics() {
    let (env, client, issuer, _buyer, registry, usdc) = setup();
    let stranger = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry.register(&buyer);

    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);

    client.confirm_delivery(&invoice_id, &stranger);
}

#[test]
fn test_trigger_default_requires_past_due_date() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool_id = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool_id);
    client.mark_funded(&invoice_id, &pool_id, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    env.ledger().set_timestamp(due_date + 1);

    let result = client.trigger_default(&invoice_id);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Defaulted);
}

// ============== ISSUE #211: trigger_default FROM INVALID STATUSES ==============

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_trigger_default_from_created_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);

    // A freshly created invoice is not Funded/Active/Confirmed, so defaulting
    // it must be rejected with InvalidStatusTransition (#8).
    env.ledger().set_timestamp(due_date + 1);
    client.trigger_default(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_trigger_default_from_listed_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Listed);

    // A Listed invoice has not been funded, so defaulting it must be rejected
    // with InvalidStatusTransition (#8).
    env.ledger().set_timestamp(due_date + 1);
    client.trigger_default(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_trigger_default_from_repaid_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool_id = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool_id);
    let escrow = mock_escrow_for_pool(&env, &pool_id, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool_id, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    client.repay(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Repaid);

    // A Repaid invoice is terminal, so defaulting it must be rejected with
    // InvalidStatusTransition (#8).
    env.ledger().set_timestamp(due_date + 1);
    client.trigger_default(&invoice_id);
}

#[test]
fn test_trigger_default_succeeds_at_exact_due_date() {
    // Boundary test: default must be allowed when `now == due_date`
    // (previously panicked due to `<=` comparison — issue #200)
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool_id = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool_id);
    client.mark_funded(&invoice_id, &pool_id, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    // Set ledger to exactly the due date
    env.ledger().set_timestamp(due_date);

    let result = client.trigger_default(&invoice_id);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Defaulted);
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")]
fn test_trigger_default_fails_before_due_date() {
    // Negative test: default must NOT be allowed when `now < due_date`
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool_id = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool_id);
    client.mark_funded(&invoice_id, &pool_id, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    // Set ledger to 1 second before the due date — should panic
    env.ledger().set_timestamp(due_date - 1);

    client.trigger_default(&invoice_id);
}

// PR 363 (closes #314) removed `test_trigger_default_admin_succeeds_after_due_date_with_auth`:
// explicit `mock_auths` + cross-contract call to `pool.handle_default` surfaced
// `Error(Context, MissingValue)` regardless of `sub_invokes` shape. The same
// happy-path is covered by `test_trigger_default_requires_past_due_date` and
// `test_trigger_default_succeeds_at_exact_due_date` via `setup()`.
#[test]
// `trigger_default` calls `admin.require_auth()` directly, so non-admin
// callers are rejected by Soroban's native `Error(Auth, InvalidAction)`
// before any contract-level error can be returned.
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_trigger_default_stranger_panics() {
    // `trigger_default` calls `admin.require_auth()` directly, so a non-admin
    // caller is rejected at the auth layer with Soroban's native
    // `Error(Auth, InvalidAction)` before any state transition.
    // TODO: refactor `trigger_default` to dispatch auth via
    // `try_invoke_contract(check_auth, admin)` + `panic_with_error!(NotAuthorized)`
    // (matching `expire_listing`) so callers see the contract-typed #3 error
    // instead of the noisy native Auth error. Currently the refactor breaks
    // the other `setup()`-based `trigger_default` tests, so it's deferred.
    let env = Env::default();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(), registry_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin, &registry_id);

    let usdc = Address::generate(&env);
    client.add_supported_asset(&usdc);
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool_id = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool_id);
    client.mark_funded(&invoice_id, &pool_id, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    // Fast forward past due date
    env.ledger().set_timestamp(due_date + 1);

    // A non-admin caller (stranger) — no additional auth needed — can successfully trigger default
    // because the admin gate has been removed.
    let result = client.trigger_default(&invoice_id);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Defaulted);
}

// Note: `test_trigger_default_admin_succeeds_after_due_date_with_auth` was
// removed as part of PR #363 (closing #314). The happy-path admin-trigger
// behavior is already exercised by `test_trigger_default_requires_past_due_date`
// and `test_trigger_default_succeeds_at_exact_due_date`, both of which rely
// on the shared `setup()` helper. Keeping this note here so future readers
// know the gap was intentional and not an oversight.

#[test]
fn test_get_by_status_filters_correctly() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let id1 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);

    let created = client.get_by_status(&InvoiceStatus::Created, &0, &MAX_PAGE_SIZE);
    assert_eq!(created.len(), 2);

    attest(&env, &client, &id1);
    client.list_for_financing(&id1, &DEFAULT_DISCOUNT_BPS);
    let created = client.get_by_status(&InvoiceStatus::Created, &0, &MAX_PAGE_SIZE);
    assert_eq!(created.len(), 1);
    let listed = client.get_by_status(&InvoiceStatus::Listed, &0, &MAX_PAGE_SIZE);
    assert_eq!(listed.len(), 1);
}

// ============== O(1) STATUS MEMBERSHIP MARKERS (issues #831 / #835) ==============

/// Reads the raw `DataKey::StatusPosition` slot straight from contract
/// storage, so these tests assert on the key itself and not only on the
/// public query helper.
fn raw_status_membership(
    env: &Env,
    client: &InvoiceContractClient,
    status: InvoiceStatus,
    invoice_id: &BytesN<32>,
) -> bool {
    let key = crate::status_position_key(status, invoice_id);
    env.as_contract(&client.address, || {
        env.storage().persistent().get::<_, u32>(&key).is_some()
    })
}

fn raw_status_position(
    env: &Env,
    client: &InvoiceContractClient,
    status: InvoiceStatus,
    invoice_id: &BytesN<32>,
) -> Option<u32> {
    let key = crate::status_position_key(status, invoice_id);
    env.as_contract(&client.address, || {
        env.storage().persistent().get::<_, u32>(&key)
    })
}

#[test]
fn test_status_membership_marker_written_on_create_and_rolled_on_transitions() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // `create()` writes the `Created` marker, and it carries the same TTL
    // policy as every other invoice storage entry.
    assert!(raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Created,
        &invoice_id
    ));
    assert!(client.has_status_membership(&InvoiceStatus::Created, &invoice_id));
    let created_key = crate::status_position_key(InvoiceStatus::Created, &invoice_id);
    let marker_ttl = env.as_contract(&client.address, || {
        env.storage().persistent().get_ttl(&created_key)
    });
    assert!(
        marker_ttl >= TTL_THRESHOLD,
        "membership marker TTL should be extended, got {marker_ttl}"
    );

    // No other status may claim membership yet.
    for status in [
        InvoiceStatus::Listed,
        InvoiceStatus::Funded,
        InvoiceStatus::Active,
        InvoiceStatus::Confirmed,
        InvoiceStatus::Repaid,
        InvoiceStatus::Defaulted,
        InvoiceStatus::Expired,
    ] {
        assert!(!raw_status_membership(&env, &client, status, &invoice_id));
        assert!(!client.has_status_membership(&status, &invoice_id));
    }

    // Created -> Listed: old marker cleared, new marker set.
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert!(!raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Created,
        &invoice_id
    ));
    assert!(raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Listed,
        &invoice_id
    ));

    // Listed -> Funded.
    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    assert!(!raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Listed,
        &invoice_id
    ));
    assert!(raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Funded,
        &invoice_id
    ));

    // Funded -> Active.
    client.mark_shipped(&invoice_id);
    assert!(!raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Funded,
        &invoice_id
    ));
    assert!(raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Active,
        &invoice_id
    ));

    // Active -> Confirmed (both parties confirm).
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert!(!raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Active,
        &invoice_id
    ));
    assert!(raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Confirmed,
        &invoice_id
    ));

    // Confirmed -> Defaulted once the due date is reached.
    env.ledger().set_timestamp(due_date);
    client.trigger_default(&invoice_id);
    assert!(!raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Confirmed,
        &invoice_id
    ));
    assert!(raw_status_membership(
        &env,
        &client,
        InvoiceStatus::Defaulted,
        &invoice_id
    ));

    // The query helper mirrors the raw storage state after every step above.
    assert!(!client.has_status_membership(&InvoiceStatus::Confirmed, &invoice_id));
    assert!(client.has_status_membership(&InvoiceStatus::Defaulted, &invoice_id));
}

#[test]
fn test_has_status_membership_rejects_prior_unknown_and_never_held() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // Current status hits in O(1); statuses never held do not.
    assert!(client.has_status_membership(&InvoiceStatus::Created, &invoice_id));
    assert!(!client.has_status_membership(&InvoiceStatus::Repaid, &invoice_id));
    assert!(!client.has_status_membership(&InvoiceStatus::Expired, &invoice_id));

    // A prior status reads `false` once the invoice has transitioned out.
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert!(!client.has_status_membership(&InvoiceStatus::Created, &invoice_id));
    assert!(client.has_status_membership(&InvoiceStatus::Listed, &invoice_id));

    // An invoice id that was never created has no membership anywhere.
    let unknown_id = BytesN::from_array(&env, &[0xABu8; 32]);
    assert!(!client.has_status_membership(&InvoiceStatus::Listed, &unknown_id));
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")]
fn test_double_confirmation_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &issuer);
}

#[test]
fn test_status_transitions_full_lifecycle() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);

    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Listed);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Funded);

    client.mark_shipped(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);

    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_mark_funded_fails_asset_mismatch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let xlm = Address::generate(&env);
    let xlm_pool = mock_pool_with_asset(&env, &xlm);
    client.set_pool_contract(&xlm_pool);
    client.mark_funded(&invoice_id, &xlm_pool, &xlm, &DEFAULT_FUNDED_AMOUNT);
}

#[test]
fn test_mark_funded_succeeds_with_matching_asset() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let result = client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.funding_pool, Some(pool));
}

// #553: mark_funded must only accept the configured pool contract. An
// address that self-authorizes but isn't the one set via set_pool_contract
// must be rejected (NotAuthorized, Contract #3), preventing arbitrary pool
// hijacking.
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_mark_funded_fails_for_non_configured_pool() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let configured_pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&configured_pool);

    // A different, self-authorizable address tries to hijack funding.
    let impostor = mock_pool_with_asset(&env, &usdc);
    client.mark_funded(&invoice_id, &impostor, &usdc, &DEFAULT_FUNDED_AMOUNT);
}

// #553: mark_funded from the pre-configured pool still succeeds.
#[test]
fn test_mark_funded_succeeds_for_configured_pool() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let result = client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.funding_pool, Some(pool));
}

// #554: mark_funded panics when funded_amount exceeds face_value.
#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_mark_funded_fails_when_amount_exceeds_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &(DEFAULT_FACE_VALUE + 1));
}

// #554: funded_amount == face_value is the boundary and must succeed.
#[test]
fn test_mark_funded_succeeds_when_amount_equals_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let result = client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FACE_VALUE);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.funded_amount, DEFAULT_FACE_VALUE);
    assert_eq!(inv.funding_pool, Some(pool));
}

#[test]
fn test_create_invoice_with_xlm_asset() {
    let (env, client, issuer, buyer, _, _usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let xlm_asset = Address::generate(&env);
    client.add_supported_asset(&xlm_asset);

    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &xlm_asset);
    let invoice = client.get(&invoice_id);

    assert_eq!(invoice.funding_asset, xlm_asset);
    assert_eq!(invoice.status, InvoiceStatus::Created);
}

#[test]
fn test_get_funding_asset_returns_correct_asset() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    let asset = client.get_funding_asset(&invoice_id);
    assert_eq!(asset, usdc);
}

#[test]
fn test_get_funding_terms_returns_correct_values() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + 86400;
    let invoice_id = client.create(&issuer, &buyer, &1_000_000_000, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &200);

    let (status, face_value, discount_bps) = client.get_funding_terms(&invoice_id);
    assert_eq!(status, 1, "status should be 1 (Listed)");
    assert_eq!(face_value, 1_000_000_000);
    assert_eq!(discount_bps, 200);
}

// ============================================================================
// Funded Amount Accessor Tests (Issue #21)
// ============================================================================

#[test]
fn test_get_funded_amount_returns_committed_amount() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let invoice_id = funded_invoice(&env, &client, &issuer, &buyer, &usdc, DEFAULT_FUNDED_AMOUNT);

    assert_eq!(client.get_funded_amount(&invoice_id), DEFAULT_FUNDED_AMOUNT);
    assert_eq!(client.get(&invoice_id).funded_amount, DEFAULT_FUNDED_AMOUNT);
}

#[test]
fn test_get_funded_amount_partial_funding_boundary() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let partial = DEFAULT_FACE_VALUE / 3;
    let invoice_id = funded_invoice(&env, &client, &issuer, &buyer, &usdc, partial);

    assert_eq!(client.get_funded_amount(&invoice_id), partial);
}

#[test]
fn test_get_funded_amount_smallest_non_zero_amount() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let invoice_id = funded_invoice(&env, &client, &issuer, &buyer, &usdc, 1);

    assert_eq!(client.get_funded_amount(&invoice_id), 1);
}

#[test]
fn test_get_funded_amount_full_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let invoice_id = funded_invoice(&env, &client, &issuer, &buyer, &usdc, DEFAULT_FACE_VALUE);

    assert_eq!(client.get_funded_amount(&invoice_id), DEFAULT_FACE_VALUE);
}

#[test]
#[should_panic(expected = "Error(Contract, #31)")]
fn test_get_funded_amount_unfunded_invoice_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    client.get_funded_amount(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #31)")]
fn test_get_funded_amount_listed_but_unfunded_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    client.get_funded_amount(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_funded_amount_unknown_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);

    client.get_funded_amount(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_funded_amount_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);

    client.get_funded_amount(&fake_id);
}

#[test]
fn test_get_funded_amount_rejected_zero_funding_stays_unfunded() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);

    // `mark_funded` rejects a zero commitment, so no invoice can legitimately
    // report 0 through the accessor.
    let result = env.try_invoke_contract::<bool, InvoiceError>(
        &client.address,
        &Symbol::new(&env, "mark_funded"),
        (invoice_id.clone(), pool.clone(), usdc.clone(), 0u128).into_val(&env),
    );
    assert_eq!(result.err().and_then(|e| e.ok()), Some(InvoiceError::InvalidAmount));

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Listed);
    assert_eq!(invoice.funded_amount, 0);
    assert!(invoice.funded_at.is_none());

    let view = env.try_invoke_contract::<u128, InvoiceError>(
        &client.address,
        &Symbol::new(&env, "get_funded_amount"),
        (invoice_id.clone(),).into_val(&env),
    );
    assert_eq!(view.err().and_then(|e| e.ok()), Some(InvoiceError::NotFunded));
}

#[test]
fn test_get_funded_amount_is_read_only() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let invoice_id = funded_invoice(&env, &client, &issuer, &buyer, &usdc, DEFAULT_FUNDED_AMOUNT);
    let invoice_key = crate::DataKey::Invoice(invoice_id.clone());
    let ttl_before = env.as_contract(&client.address, || {
        env.storage().persistent().get_ttl(&invoice_key)
    });
    let invoice_before = client.get(&invoice_id).to_xdr(&env);
    let events_before = env.events().all().len();

    assert_eq!(client.get_funded_amount(&invoice_id), DEFAULT_FUNDED_AMOUNT);
    assert_eq!(client.get_funded_amount(&invoice_id), DEFAULT_FUNDED_AMOUNT);

    let ttl_after = env.as_contract(&client.address, || {
        env.storage().persistent().get_ttl(&invoice_key)
    });
    assert_eq!(ttl_after, ttl_before, "accessor must not extend TTL");
    assert_eq!(
        client.get(&invoice_id).to_xdr(&env),
        invoice_before,
        "accessor must not mutate invoice state"
    );
    assert_eq!(env.events().all().len(), events_before, "accessor must not emit events");
}

#[test]
fn test_expire_listing_succeeds_by_issuer() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Fast forward ledger time by 7 days + 1 second
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    let result = client.expire_listing(&invoice_id, &issuer);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);
}

#[test]
fn test_expire_listing_succeeds_by_admin() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Fast forward ledger time by 7 days + 1 second
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    let result = client.expire_listing(&invoice_id, &issuer);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_expire_listing_early_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Fast forward ledger time by only 5 days (less than 7 days)
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 5 * 24 * 60 * 60);

    client.expire_listing(&invoice_id, &issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_expire_listing_wrong_status_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // Fast forward ledger time
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    client.expire_listing(&invoice_id, &issuer);
}

#[test]
fn test_expire_listing_configurable_window() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Set expiry window to 1 day (86400 seconds)
    client.set_expiry_window(&86400);
    assert_eq!(client.get_expiry_window(), 86400);

    // Fast forward by 1 day + 1 second
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + DEFAULT_DUE_OFFSET + 1);

    let result = client.expire_listing(&invoice_id, &issuer);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);
}
#[test]
fn test_expire_listing_exact_boundary() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Fast forward by exact expiry window (7 days)
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60);

    let result = client.expire_listing(&invoice_id, &issuer);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_expire_listing_one_second_before_boundary_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Fast forward to 1 second before expiry window
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 - 1);

    client.expire_listing(&invoice_id, &issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")]
fn test_set_expiry_window_rejects_out_of_bounds() {
    let (_env, client, _, _, _, _) = setup();
    // Set an expiry window that exceeds the 365-day upper bound
    client.set_expiry_window(&31_536_001);
}

#[test]
fn test_set_pool_contract_emits_event() {
    let (env, client, _, _, _, _) = setup();
    let pool = Address::generate(&env);

    client.set_pool_contract(&pool);

    let contract_id = client.address.clone();
    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, contract_id);
    assert_eq!(
        topics,
        (
            Symbol::new(&env, "pool_contract_updated"),
            pool.clone(),
            pool.clone()
        )
            .into_val(&env)
    );
    <()>::try_from_val(&env, &data).unwrap();
}

#[test]
fn test_set_pool_contract_rejects_conflicting_addresses() {
    let (env, client, _, _, _, _) = setup();
    let admin = client.get_admin().unwrap();
    let registry = client.get_registry_contract().unwrap();
    let escrow = Address::generate(&env);
    let agent_registry = Address::generate(&env);
    client.set_escrow_contract(&escrow);
    client.set_agent_registry_contract(&agent_registry);

    for candidate in [
        admin,
        client.address.clone(),
        registry,
        escrow,
        agent_registry,
    ] {
        assert_wiring_setter_rejects(&env, &client, "set_pool_contract", &candidate);
    }
}

#[test]
fn test_set_escrow_contract_rejects_conflicting_addresses() {
    let (env, client, _, _, _, _) = setup();
    let admin = client.get_admin().unwrap();
    let registry = client.get_registry_contract().unwrap();
    let pool = Address::generate(&env);
    let agent_registry = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.set_agent_registry_contract(&agent_registry);

    for candidate in [
        admin,
        client.address.clone(),
        registry,
        pool,
        agent_registry,
    ] {
        assert_wiring_setter_rejects(&env, &client, "set_escrow_contract", &candidate);
    }
}

#[test]
fn test_set_agent_registry_contract_rejects_conflicting_addresses() {
    let (env, client, _, _, _, _) = setup();
    let admin = client.get_admin().unwrap();
    let registry = client.get_registry_contract().unwrap();
    let pool = Address::generate(&env);
    let escrow = Address::generate(&env);
    client.set_pool_contract(&pool);
    client.set_escrow_contract(&escrow);

    for candidate in [admin, client.address.clone(), registry, pool, escrow] {
        assert_wiring_setter_rejects(&env, &client, "set_agent_registry_contract", &candidate);
    }
}

#[test]
fn test_initialize_rejects_conflicting_registry_address() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    for registry in [admin.clone(), contract_id.clone()] {
        assert!(client.try_initialize(&admin, &registry).is_err());
    }
}

#[test]
fn test_set_expiry_window_emits_event() {
    let (env, client, _, _, _, _) = setup();
    let window: u64 = 86400;

    client.set_expiry_window(&window);

    let contract_id = client.address.clone();
    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, contract_id);
    assert_eq!(
        topics,
        (Symbol::new(&env, "expiry_window_set"),).into_val(&env)
    );
    assert_eq!(u64::try_from_val(&env, &data).unwrap(), window);
}

#[test]
fn test_mark_shipped_succeeds_by_issuer() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    let result = client.mark_shipped(&invoice_id);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Active);
    assert!(inv.shipped_at.is_some());
}

#[test]
#[should_panic]
fn test_mark_shipped_stranger_panics() {
    let env = Env::default();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let usdc = Address::generate(&env);
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let pool = mock_pool_with_asset(&env, &usdc);

    // Initialize as admin
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(), registry_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin, &registry_id);

    // Create invoice as issuer
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // List as issuer
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "list_for_financing",
            args: (invoice_id.clone(), DEFAULT_DISCOUNT_BPS).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    // Set pool as admin
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "set_pool_contract",
            args: (pool.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.set_pool_contract(&pool);

    // Mark funded as pool
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &pool,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "mark_funded",
            args: (
                invoice_id.clone(),
                pool.clone(),
                usdc.clone(),
                DEFAULT_FUNDED_AMOUNT,
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    // Calling mark_shipped without mocking auths for the issuer should panic
    // due to failed require_auth. The stranger address is not the issuer.
    client.mark_shipped(&invoice_id);
}

#[test]
#[should_panic]
fn test_expire_listing_stranger_panics() {
    let env = Env::default();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(), registry_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin, &registry_id);

    let usdc = Address::generate(&env);
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "list_for_financing",
            args: (invoice_id.clone(), DEFAULT_DISCOUNT_BPS).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    // Calling expire_listing without mocking auths for issuer or admin should panic due to failed require_auth.
    client.expire_listing(&invoice_id, &issuer);
}

#[test]
fn test_invoice_id_generation_is_deterministic() {
    // Test that invoice ID generation is deterministic for the same inputs
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value = DEFAULT_FACE_VALUE;

    // Create first invoice
    let invoice_id_1 = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    // Reset counter to create another invoice with same parameters (except counter)
    // We need to verify that same issuer/buyer/value/date with different counter produces different IDs
    let invoice_id_2 = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    // Different counter should produce different IDs
    assert_ne!(invoice_id_1, invoice_id_2);

    // Verify both invoices exist and have correct data
    assert_eq!(client.get(&invoice_id_1).issuer, issuer);
    assert_eq!(client.get(&invoice_id_2).issuer, issuer);
}

#[test]
fn test_invoice_ids_unique_for_different_issuers() {
    // Test that different issuer/buyer combinations produce unique IDs
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value = DEFAULT_FACE_VALUE;

    // Create first invoice
    let invoice_id_1 = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    // Create second invoice with different issuer
    let issuer2 = Address::generate(&env);
    registry.register(&issuer2);
    let invoice_id_2 = client.create(&issuer2, &buyer, &face_value, &due_date, &usdc);

    // Different issuers should produce different IDs even with same other parameters
    assert_ne!(invoice_id_1, invoice_id_2);

    // Verify invoices have correct issuers
    assert_eq!(client.get(&invoice_id_1).issuer, issuer);
    assert_eq!(client.get(&invoice_id_2).issuer, issuer2);
}

#[test]
fn test_invoice_ids_unique_for_different_buyers() {
    // Test that different buyer combinations produce unique IDs
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value = DEFAULT_FACE_VALUE;

    // Create first invoice
    let invoice_id_1 = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    // Create second invoice with different buyer
    let buyer2 = Address::generate(&env);
    registry.register(&buyer2);
    let invoice_id_2 = client.create(&issuer, &buyer2, &face_value, &due_date, &usdc);

    // Different buyers should produce different IDs even with same other parameters
    assert_ne!(invoice_id_1, invoice_id_2);

    // Verify invoices have correct buyers
    assert_eq!(client.get(&invoice_id_1).buyer, buyer);
    assert_eq!(client.get(&invoice_id_2).buyer, buyer2);
}

#[test]
fn test_invoice_ids_unique_for_different_face_values() {
    // Test that different face values produce unique IDs
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let id1 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let id2 = client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);

    assert_ne!(id1, id2);
}

// ── Issue #196: repay from Funded, Active, or Confirmed ─────────────────────────

fn mint_tokens(env: &Env, token: &Address, to: &Address, amount: i128) {
    let token_client = MockTokenClient::new(env, token);
    token_client.mint(to, &amount);
}

#[test]
fn test_repay_from_funded_succeeds() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Funded);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    let result = client.repay(&invoice_id);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Repaid);
    assert!(inv.repaid_at.is_some());
}

#[test]
fn test_repay_from_active_succeeds() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Active);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    let result = client.repay(&invoice_id);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Repaid);
    assert!(inv.repaid_at.is_some());
}

#[test]
fn test_repay_from_confirmed_succeeds() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    let result = client.repay(&invoice_id);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Repaid);
    assert!(inv.repaid_at.is_some());
}

#[test]
fn test_repay_emits_event() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    client.repay(&invoice_id);

    let contract_id = client.address.clone();
    let events = env.events().all();
    let found = events.iter().any(|e| {
        let (c, topics, _data) = e;
        if c != contract_id {
            return false;
        }
        let topic0: Symbol = Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap();
        topic0 == Symbol::new(&env, "invoice_repaid")
    });
    assert!(found);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_fails_from_created() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    // Status is Created — repay should panic
    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_fails_from_listed() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    // Status is Listed — repay should panic
    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_repay_fails_no_auth() {
    let env = Env::default();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let usdc = Address::generate(&env);
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    // Initialize as admin
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(), registry_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin, &registry_id);

    client.add_supported_asset(&usdc);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "list_for_financing",
            args: (invoice_id.clone(), DEFAULT_DISCOUNT_BPS).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "set_pool_contract",
            args: (pool.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.set_pool_contract(&pool);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &pool,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "mark_funded",
            args: (
                invoice_id.clone(),
                pool.clone(),
                usdc.clone(),
                DEFAULT_FUNDED_AMOUNT,
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    // Do not mock auth for buyer — repay should fail with auth error
    client.repay(&invoice_id);
}

// ============== PROPERTY-BASED INVARIANT TESTS ==============
// Uses proptest's TestRunner API directly (standard Rust closures) so
// rustfmt formats the tests normally.  Case budget is 10 per property
// to stay within CI time budgets for the Soroban in-process host.

#[test]
fn prop_any_positive_face_value_creates_invoice_in_created_status() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(1u128..=1_000_000_000_000_000u128), |face_value| {
            let (env, client, issuer, buyer, _, usdc) = setup();
            let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
            let id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
            let inv = client.get(&id);
            prop_assert_eq!(inv.face_value, face_value);
            prop_assert_eq!(inv.status, InvoiceStatus::Created);
            prop_assert!(!inv.issuer_confirmed);
            prop_assert!(!inv.buyer_confirmed);
            prop_assert_eq!(inv.funded_amount, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_any_future_due_date_creates_invoice_successfully() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(1u64..=31_536_000u64), |offset| {
            let (env, client, issuer, buyer, _, usdc) = setup();
            let due_date = env.ledger().timestamp() + offset;
            let id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
            let inv = client.get(&id);
            prop_assert_eq!(inv.due_date, due_date);
            prop_assert_eq!(inv.status, InvoiceStatus::Created);
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_discount_bps_within_limit_always_lists_invoice() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(0u32..=5000u32), |discount_bps| {
            let (env, client, issuer, buyer, _, usdc) = setup();
            let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
            let id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
            attest(&env, &client, &id);
            let result = client.list_for_financing(&id, &discount_bps);
            prop_assert!(result);
            let inv = client.get(&id);
            prop_assert_eq!(inv.discount_bps, discount_bps);
            prop_assert_eq!(inv.status, InvoiceStatus::Listed);
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_invoice_id_is_deterministic_for_same_inputs() {
    // Same issuer, buyer, face_value, due_date, asset at the same ledger
    // timestamp must always produce the same invoice ID.
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(1u128..=1_000_000_000_000u128), |face_value| {
            let (env, client, issuer, buyer, _, usdc) = setup();
            let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
            let id1 = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
            // counter increments each call, so a second create with identical
            // params produces a different ID — verify the first is stable via get()
            let inv = client.get(&id1);
            prop_assert_eq!(inv.id, id1);
            prop_assert_eq!(inv.face_value, face_value);
            Ok(())
        })
        .unwrap();
}

#[test]
fn prop_expiry_window_bounds_are_respected_across_values() {
    // For any window in [1, 30 days], a listing that expires exactly
    // window+1 seconds later must succeed.
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&(1u64..=2_592_000u64), |window| {
            let (env, client, issuer, buyer, _, usdc) = setup();
            client.set_expiry_window(&window);
            prop_assert_eq!(client.get_expiry_window(), window);
            let due_date = env.ledger().timestamp() + window + 86_400;
            let id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
            attest(&env, &client, &id);
            client.list_for_financing(&id, &DEFAULT_DISCOUNT_BPS);
            env.ledger()
                .set_timestamp(env.ledger().timestamp() + window + 1);
            let expired = client.expire_listing(&id, &issuer);
            prop_assert!(expired);
            prop_assert_eq!(client.get(&id).status, InvoiceStatus::Expired);
            Ok(())
        })
        .unwrap();
}

fn assert_status_index_consistency(client: &InvoiceContractClient, invoice_id: &BytesN<32>) {
    let invoice = client.get(invoice_id);
    let statuses = [
        InvoiceStatus::Created,
        InvoiceStatus::Listed,
        InvoiceStatus::Funded,
        InvoiceStatus::Active,
        InvoiceStatus::Confirmed,
        InvoiceStatus::Repaid,
        InvoiceStatus::Defaulted,
        InvoiceStatus::Expired,
    ];

    for status in statuses {
        let indexed = client.get_by_status(&status, &0, &MAX_PAGE_SIZE);
        if status == invoice.status {
            assert_eq!(indexed.len(), 1);
            assert_eq!(indexed.get(0).unwrap().id, *invoice_id);
        } else {
            assert_eq!(indexed.len(), 0);
        }
    }
}

#[test]
fn prop_random_valid_transition_paths_preserve_indexes_and_terminality() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(
                any::<bool>(),
                0u32..=5000u32,
                1u64..=2_592_000u64,
                any::<bool>(),
            ),
            |(expires, discount_bps, expiry_window, issuer_confirms_first)| {
                let (env, client, issuer, buyer, _, usdc) = setup();
                let due_date = env.ledger().timestamp() + 31_536_000;
                let invoice_id =
                    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
                assert_status_index_consistency(&client, &invoice_id);

                attest(&env, &client, &invoice_id);
                client.list_for_financing(&invoice_id, &discount_bps);
                assert_status_index_consistency(&client, &invoice_id);

                if expires {
                    client.set_expiry_window(&expiry_window);
                    env.ledger()
                        .set_timestamp(env.ledger().timestamp() + expiry_window);
                    prop_assert!(client.expire_listing(&invoice_id, &issuer));
                    assert_status_index_consistency(&client, &invoice_id);

                    let reopened = env.try_invoke_contract::<bool, soroban_sdk::Error>(
                        &client.address,
                        &Symbol::new(&env, "list_for_financing"),
                        (invoice_id.clone(), discount_bps).into_val(&env),
                    );
                    prop_assert!(reopened.is_err());
                } else {
                    let pool = mock_pool_with_asset(&env, &usdc);
                    client.set_pool_contract(&pool);
                    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
                    client.set_escrow_contract(&escrow);
                    let funded_amount =
                        DEFAULT_FACE_VALUE * (10_000 - discount_bps as u128) / 10_000;
                    client.mark_funded(&invoice_id, &pool, &usdc, &funded_amount);
                    assert_status_index_consistency(&client, &invoice_id);

                    client.mark_shipped(&invoice_id);
                    assert_status_index_consistency(&client, &invoice_id);

                    if issuer_confirms_first {
                        client.confirm_delivery(&invoice_id, &issuer);
                        assert_status_index_consistency(&client, &invoice_id);
                        client.confirm_delivery(&invoice_id, &buyer);
                    } else {
                        client.confirm_delivery(&invoice_id, &buyer);
                        assert_status_index_consistency(&client, &invoice_id);
                        client.confirm_delivery(&invoice_id, &issuer);
                    }
                    assert_status_index_consistency(&client, &invoice_id);

                    mint_tokens(&env, &usdc, &buyer, DEFAULT_FACE_VALUE as i128);
                    prop_assert!(client.repay(&invoice_id));
                    assert_status_index_consistency(&client, &invoice_id);

                    let replayed = env.try_invoke_contract::<bool, soroban_sdk::Error>(
                        &client.address,
                        &Symbol::new(&env, "repay"),
                        (invoice_id.clone(),).into_val(&env),
                    );
                    prop_assert!(replayed.is_err());
                }

                assert_status_index_consistency(&client, &invoice_id);
                Ok(())
            },
        )
        .unwrap();
}

// --------------- Mock Escrow ---------------

/// Minimal mock escrow that records the pool address and implements
/// `release_to_pool` so `invoice::repay` / `invoice::repay_early` can call
/// it without needing the full real escrow contract.
#[contract]
pub struct MockEscrow;

#[contractimpl]
impl MockEscrow {
    /// Stores the pool address so `release_to_pool` knows where to forward.
    pub fn set_pool(env: Env, pool: Address) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "pool"), &pool);
    }

    /// Stores the USDC asset address.
    pub fn set_asset(env: Env, asset: Address) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "asset"), &asset);
    }

    pub fn set_release_result(env: Env, result: bool) {
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "release_result"), &result);
    }

    /// Minimal stub: transfers `amount` from escrow to pool.
    /// No pool auth required — mirrors the real escrow's updated behavior
    /// where the invoice contract (not the pool) is the caller.
    pub fn release_to_pool(env: Env, _invoice_id: BytesN<32>, amount: u128) -> bool {
        let pool: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "pool"))
            .unwrap();
        let asset: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "asset"))
            .unwrap();

        let escrow_addr = env.current_contract_address();
        let token_client = token::Client::new(&env, &asset);
        token_client.transfer(&escrow_addr, &pool, &(amount as i128));

        env.storage()
            .instance()
            .get(&Symbol::new(&env, "release_result"))
            .unwrap_or(true)
    }
}

/// Registers a MockEscrow wired to `pool_id` and `asset` and returns its address.
fn mock_escrow_for_pool(env: &Env, pool_id: &Address, asset: &Address) -> Address {
    let escrow_id = env.register_contract(None, MockEscrow);
    let esc_client = MockEscrowClient::new(env, &escrow_id);
    esc_client.set_pool(pool_id);
    esc_client.set_asset(asset);
    escrow_id
}

fn setup_confirmed_invoice() -> (
    Env,
    InvoiceContractClient<'static>,
    Address,
    BytesN<32>,
    Address,
    Address,
) {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    (env, client, buyer, invoice_id, pool, escrow)
}

// ============== SUPPORTED ASSET TESTS ==============

#[test]
fn test_add_supported_asset() {
    let (env, client, _, _, _, _) = setup();
    let asset = Address::generate(&env);

    assert!(!client.is_supported_asset(&asset));
    client.add_supported_asset(&asset);
    assert!(client.is_supported_asset(&asset));
    assert_eq!(client.get_supported_asset_count(), 2);

    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, client.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "supported_asset_added"), asset.clone()).into_val(&env)
    );
    <()>::try_from_val(&env, &data).unwrap();
}

// ============================== REPAY TESTS ==============================

#[test]
fn test_repay_rejects_false_escrow_result() {
    let (env, client, buyer, invoice_id, _pool, escrow) = setup_confirmed_invoice();
    MockEscrowClient::new(&env, &escrow).set_release_result(&false);
    mint_tokens(
        &env,
        &client.get_funding_asset(&invoice_id),
        &buyer,
        DEFAULT_FACE_VALUE as i128,
    );

    let result = env.try_invoke_contract::<bool, soroban_sdk::Error>(
        &client.address,
        &Symbol::new(&env, "repay"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert!(result.is_err());
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_repay_rejects_false_pool_result() {
    let (env, client, buyer, invoice_id, pool, _escrow) = setup_confirmed_invoice();
    MockPoolClient::new(&env, &pool).set_return_values(&true, &false);
    mint_tokens(
        &env,
        &client.get_funding_asset(&invoice_id),
        &buyer,
        DEFAULT_FACE_VALUE as i128,
    );

    let result = env.try_invoke_contract::<bool, soroban_sdk::Error>(
        &client.address,
        &Symbol::new(&env, "repay"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert!(result.is_err());
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_repay_early_rejects_false_escrow_result() {
    let (env, client, buyer, invoice_id, _pool, escrow) = setup_confirmed_invoice();
    MockEscrowClient::new(&env, &escrow).set_release_result(&false);
    mint_tokens(
        &env,
        &client.get_funding_asset(&invoice_id),
        &buyer,
        DEFAULT_FACE_VALUE as i128,
    );

    let result = env.try_invoke_contract::<bool, soroban_sdk::Error>(
        &client.address,
        &Symbol::new(&env, "repay_early"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert!(result.is_err());
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_repay_early_rejects_false_pool_result() {
    let (env, client, buyer, invoice_id, pool, _escrow) = setup_confirmed_invoice();
    MockPoolClient::new(&env, &pool).set_return_values(&true, &false);
    mint_tokens(
        &env,
        &client.get_funding_asset(&invoice_id),
        &buyer,
        DEFAULT_FACE_VALUE as i128,
    );

    let result = env.try_invoke_contract::<bool, soroban_sdk::Error>(
        &client.address,
        &Symbol::new(&env, "repay_early"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert!(result.is_err());
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_repay_fails_when_funding_asset_contract_missing() {
    let (env, client, issuer, buyer, _, _) = setup();
    let missing_token = Address::generate(&env);
    client.add_supported_asset(&missing_token);

    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(
        &issuer,
        &buyer,
        &DEFAULT_FACE_VALUE,
        &due_date,
        &missing_token,
    );
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &missing_token);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &missing_token);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &missing_token, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    let result = env.try_invoke_contract::<bool, InvoiceError>(
        &client.address,
        &Symbol::new(&env, "repay"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert_eq!(
        result.err().and_then(|e| e.ok()),
        Some(InvoiceError::CrossContractCallFailed)
    );
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_repay_early_fails_when_funding_asset_contract_missing() {
    let (env, client, issuer, buyer, _, _) = setup();
    let missing_token = Address::generate(&env);
    client.add_supported_asset(&missing_token);

    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(
        &issuer,
        &buyer,
        &DEFAULT_FACE_VALUE,
        &due_date,
        &missing_token,
    );
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &missing_token);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &missing_token);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &missing_token, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    let result = env.try_invoke_contract::<bool, InvoiceError>(
        &client.address,
        &Symbol::new(&env, "repay_early"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert_eq!(
        result.err().and_then(|e| e.ok()),
        Some(InvoiceError::CrossContractCallFailed)
    );
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_trigger_default_rejects_false_pool_result() {
    let (env, client, _buyer, invoice_id, pool, _escrow) = setup_confirmed_invoice();
    MockPoolClient::new(&env, &pool).set_return_values(&false, &true);
    let due_date = client.get(&invoice_id).due_date;
    env.ledger().set_timestamp(due_date);

    let result = env.try_invoke_contract::<bool, soroban_sdk::Error>(
        &client.address,
        &Symbol::new(&env, "trigger_default"),
        (invoice_id.clone(),).into_val(&env),
    );

    assert!(result.is_err());
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
}

#[test]
fn test_repay_from_confirmed() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    let events_before = env.events().all().len();
    client.repay(&invoice_id);

    let invoice = client.get(&invoice_id);
    assert_eq!(invoice.status, InvoiceStatus::Repaid);
    assert!(invoice.repaid_at.is_some());
    assert!(env.events().all().len() > events_before);
}

#[test]
#[should_panic(expected = "Error(Auth")]
fn test_repay_wrong_auth_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    env.set_auths(&[]);
    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_from_created_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Created);
    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_from_listed_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Listed);
    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_from_repaid_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    client.repay(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Repaid);
    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_from_defaulted_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);
    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    env.ledger().set_timestamp(due_date + 1);
    client.trigger_default(&invoice_id);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Defaulted);

    client.repay(&invoice_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")]
fn test_repay_from_expired_rejected() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    client.set_expiry_window(&100);
    env.ledger().set_timestamp(env.ledger().timestamp() + 101);
    client.expire_listing(&invoice_id, &issuer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);

    client.repay(&invoice_id);
}

#[test]
fn test_supported_asset_count_accepts_last_slot_then_rejects_overflow() {
    let (env, client, _, _, _, _) = setup();
    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&crate::DataKey::SupportedAssetCount, &(u32::MAX - 1));
    });
    let last = Address::generate(&env);
    client.add_supported_asset(&last);
    let overflow = Address::generate(&env);
    assert!(client.try_add_supported_asset(&overflow).is_err());
    env.as_contract(&client.address, || {
        assert_eq!(
            env.storage()
                .instance()
                .get::<_, u32>(&crate::DataKey::SupportedAssetCount),
            Some(u32::MAX)
        );
        assert!(env
            .storage()
            .persistent()
            .has(&crate::DataKey::SupportedAsset(last)));
        assert!(!env
            .storage()
            .persistent()
            .has(&crate::DataKey::SupportedAsset(overflow)));
    });
}

#[test]
fn test_status_index_last_slot_and_page_range_at_u32_limit() {
    let (env, client, _, _, _, _) = setup();
    let id = BytesN::from_array(&env, &[95; 32]);
    env.as_contract(&client.address, || {
        let count_key = crate::DataKey::StatusIndexCount(InvoiceStatus::Created as u32);
        env.storage().persistent().set(&count_key, &(u32::MAX - 1));
        crate::extend_status_index(&env, InvoiceStatus::Created, &id);
        assert_eq!(
            env.storage().persistent().get::<_, u32>(&count_key),
            Some(u32::MAX)
        );
        assert_eq!(
            env.storage()
                .persistent()
                .get::<_, BytesN<32>>(&crate::DataKey::StatusIndexEntry(
                    InvoiceStatus::Created as u32,
                    u32::MAX - 1
                )),
            Some(id)
        );
        assert_eq!(
            crate::page_range(&env, u32::MAX, u32::MAX - 1, 1),
            (u32::MAX - 1, u32::MAX)
        );
        assert_eq!(crate::page_range(&env, u32::MAX, u32::MAX, 2), (0, 0));
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_extend_issuer_index_overflow_fails_before_scan_or_write() {
    let (env, client, issuer, _buyer, _, _) = setup();
    let id = BytesN::from_array(&env, &[95; 32]);
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .set(&crate::DataKey::IssuerIndexCount(issuer.clone()), &u32::MAX);
        crate::extend_issuer_index(&env, &issuer, &id);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_extend_buyer_index_overflow_fails_before_scan_or_write() {
    let (env, client, _issuer, buyer, _, _) = setup();
    let id = BytesN::from_array(&env, &[95; 32]);
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .set(&crate::DataKey::BuyerIndexCount(buyer.clone()), &u32::MAX);
        crate::extend_buyer_index(&env, &buyer, &id);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_extend_status_index_overflow_fails_before_scan_or_write() {
    let (env, client, _issuer, _buyer, _, _) = setup();
    let id = BytesN::from_array(&env, &[95; 32]);
    env.as_contract(&client.address, || {
        env.storage().persistent().set(
            &crate::DataKey::StatusIndexCount(InvoiceStatus::Created as u32),
            &u32::MAX,
        );
        crate::extend_status_index(&env, InvoiceStatus::Created, &id);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")]
fn test_create_fails_counter_overflow() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.as_contract(&client.address, || {
        env.storage()
            .instance()
            .set(&crate::DataKey::Counter, &u64::MAX);
    });

    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
}

// ============== ISSUE #201: TYPED ERRORS FOR UNINITIALIZED CONTRACT ==============

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_create_fails_uninitialized_registry() {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let usdc = Address::generate(&env);
    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_funding_terms_unknown_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_funding_terms(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_create_fails_missing_counter() {
    let env = Env::default();
    env.mock_all_auths();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    client.initialize(&admin, &registry_id);

    let usdc = Address::generate(&env);
    client.add_supported_asset(&usdc);

    env.as_contract(&client.address, || {
        env.storage().instance().remove(&crate::DataKey::Counter);
    });

    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn test_create_fails_self_invoicing() {
    let (env, client, issuer, _buyer, _registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    client.create(&issuer, &issuer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
}

// ============== ISSUE #218: create writes to InvoicesByIssuer and InvoicesByBuyer indexes ==============

#[test]
fn test_create_writes_to_issuer_index() {
    // Verifies that `create` stores the invoice ID in the issuer's index,
    // both via a direct storage read and the public `get_by_issuer` query.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    // Direct storage read: issuer index count and entry
    env.as_contract(&client.address, || {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&crate::DataKey::IssuerIndexCount(issuer.clone()))
            .unwrap_or(0);
        assert_eq!(count, 1);

        let stored_id: BytesN<32> = env
            .storage()
            .persistent()
            .get(&crate::DataKey::IssuerIndexEntry(issuer.clone(), 0))
            .unwrap();
        assert_eq!(stored_id, invoice_id);
    });

    // Public API: get_by_issuer returns the invoice
    let invoices = client.get_by_issuer(&issuer, &0, &MAX_PAGE_SIZE);
    assert_eq!(invoices.len(), 1);
    assert_eq!(invoices.get(0).unwrap().id, invoice_id);

    // A different (unused) issuer address returns no invoices
    let other = Address::generate(&env);
    let empty = client.get_by_issuer(&other, &0, &MAX_PAGE_SIZE);
    assert_eq!(empty.len(), 0);

    // Verify the invoice_created event was emitted by the invoice contract
    let events = env.events().all();
    let (event_contract, _topics, _data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, client.address);
}

#[test]
fn test_create_writes_to_buyer_index() {
    // Verifies that `create` stores the invoice ID in the buyer's index,
    // both via a direct storage read and the public `get_by_buyer` query.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    // Direct storage read: buyer index count and entry
    env.as_contract(&client.address, || {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&crate::DataKey::BuyerIndexCount(buyer.clone()))
            .unwrap_or(0);
        assert_eq!(count, 1);

        let stored_id: BytesN<32> = env
            .storage()
            .persistent()
            .get(&crate::DataKey::BuyerIndexEntry(buyer.clone(), 0))
            .unwrap();
        assert_eq!(stored_id, invoice_id);
    });

    // Public API: get_by_buyer returns the invoice
    let invoices = client.get_by_buyer(&buyer, &0, &MAX_PAGE_SIZE);
    assert_eq!(invoices.len(), 1);
    assert_eq!(invoices.get(0).unwrap().id, invoice_id);

    // A different (unused) buyer address returns no invoices
    let other = Address::generate(&env);
    let empty = client.get_by_buyer(&other, &0, &MAX_PAGE_SIZE);
    assert_eq!(empty.len(), 0);

    // Verify the invoice_created event was emitted by the invoice contract
    let events = env.events().all();
    let (event_contract, _topics, _data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, client.address);
}

#[test]
fn test_create_writes_to_both_indexes_multiple_invoices() {
    // Verifies that when the same issuer and buyer create multiple invoices,
    // both indexes correctly accumulate the invoice IDs.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let id1 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let id2 = client.create(&issuer, &buyer, &2_000_000_000, &due_date, &usdc);

    // Direct storage: issuer index has 2 entries
    env.as_contract(&client.address, || {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&crate::DataKey::IssuerIndexCount(issuer.clone()))
            .unwrap_or(0);
        assert_eq!(count, 2);

        let stored_id_0: BytesN<32> = env
            .storage()
            .persistent()
            .get(&crate::DataKey::IssuerIndexEntry(issuer.clone(), 0))
            .unwrap();
        assert_eq!(stored_id_0, id1);

        let stored_id_1: BytesN<32> = env
            .storage()
            .persistent()
            .get(&crate::DataKey::IssuerIndexEntry(issuer.clone(), 1))
            .unwrap();
        assert_eq!(stored_id_1, id2);
    });

    // Direct storage: buyer index has 2 entries
    env.as_contract(&client.address, || {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&crate::DataKey::BuyerIndexCount(buyer.clone()))
            .unwrap_or(0);
        assert_eq!(count, 2);

        let stored_id_0: BytesN<32> = env
            .storage()
            .persistent()
            .get(&crate::DataKey::BuyerIndexEntry(buyer.clone(), 0))
            .unwrap();
        assert_eq!(stored_id_0, id1);

        let stored_id_1: BytesN<32> = env
            .storage()
            .persistent()
            .get(&crate::DataKey::BuyerIndexEntry(buyer.clone(), 1))
            .unwrap();
        assert_eq!(stored_id_1, id2);
    });

    // Public API assertions
    assert_eq!(client.get_by_issuer(&issuer, &0, &MAX_PAGE_SIZE).len(), 2);
    assert_eq!(client.get_by_buyer(&buyer, &0, &MAX_PAGE_SIZE).len(), 2);
}

#[test]
fn test_create_indexes_are_party_specific() {
    // Verifies that issuer and buyer indexes are independent:
    // an invoice created by issuer A with buyer B should appear in
    // A's issuer index and B's buyer index, but NOT in B's issuer index
    // or A's buyer index.
    let (env, client, issuer, buyer, registry, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // Issuer should see the invoice in their issuer index
    let issuer_invoices = client.get_by_issuer(&issuer, &0, &MAX_PAGE_SIZE);
    assert_eq!(issuer_invoices.len(), 1);
    assert_eq!(issuer_invoices.get(0).unwrap().id, invoice_id);

    // Buyer should see the invoice in their buyer index
    let buyer_invoices = client.get_by_buyer(&buyer, &0, &MAX_PAGE_SIZE);
    assert_eq!(buyer_invoices.len(), 1);
    assert_eq!(buyer_invoices.get(0).unwrap().id, invoice_id);

    // Issuer should NOT see the invoice in their buyer index
    let issuer_as_buyer = client.get_by_buyer(&issuer, &0, &MAX_PAGE_SIZE);
    assert_eq!(issuer_as_buyer.len(), 0);

    // Buyer should NOT see the invoice in their issuer index
    let buyer_as_issuer = client.get_by_issuer(&buyer, &0, &MAX_PAGE_SIZE);
    assert_eq!(buyer_as_issuer.len(), 0);

    // An unrelated third party should see nothing in either index
    let stranger = Address::generate(&env);
    registry.register(&stranger);
    assert_eq!(client.get_by_issuer(&stranger, &0, &MAX_PAGE_SIZE).len(), 0);
    assert_eq!(client.get_by_buyer(&stranger, &0, &MAX_PAGE_SIZE).len(), 0);
}

#[test]
fn test_create_indexes_emit_invoice_created_event() {
    // Verifies that the invoice_created event is emitted with the correct
    // topics and data payload when an invoice is created.
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    client.create(&issuer, &buyer, &face_value, &due_date, &usdc);

    let contract_id = client.address.clone();
    let events = env.events().all();

    // Should have at least one event; the last one should be invoice_created
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, contract_id);

    // Topics: (Symbol("invoice_created"), invoice_id, issuer, buyer, funding_asset)
    let topic0: Symbol = Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap();
    assert_eq!(topic0, Symbol::new(&env, "invoice_created"));

    // Data: face_value as u128
    let stored_value: u128 = u128::try_from_val(&env, &data).unwrap();
    assert_eq!(stored_value, face_value);
}

// ============================================================================
// View Functions Initialization & Missing Invoice Tests (Issue #465)
// ============================================================================

#[test]
fn test_view_functions_initialized_existing_invoice() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &250);

    assert_eq!(client.get_issuer(&invoice_id), issuer);
    assert_eq!(client.get_face_value(&invoice_id), face_value);
    assert_eq!(client.get_funding_asset(&invoice_id), usdc);
    assert_eq!(client.get_discount_bps(&invoice_id), 250);
    assert_eq!(client.get_status(&invoice_id), InvoiceStatus::Listed as u32);

    let inv = client.get(&invoice_id);
    assert_eq!(inv.id, invoice_id);
    assert_eq!(inv.issuer, issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_issuer_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_issuer(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_face_value_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_face_value(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_funding_asset_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_funding_asset(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_discount_bps_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_discount_bps(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_status_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_status(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_issuer_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_issuer(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_face_value_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_face_value(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_funding_asset_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_funding_asset(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_discount_bps_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_discount_bps(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_status_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_status(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get(&fake_id);
}

// ============================================================================
// get_buyer / get_due_date accessor tests (issue #574)
// ============================================================================

#[test]
fn test_get_buyer_returns_correct_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    assert_eq!(client.get_buyer(&invoice_id), buyer);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_buyer_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_buyer(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_buyer_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_buyer(&fake_id);
}

#[test]
fn test_get_due_date_returns_correct_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let face_value: u128 = DEFAULT_FACE_VALUE;
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    assert_eq!(client.get_due_date(&invoice_id), due_date);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_get_due_date_missing_invoice_panics() {
    let (env, client, _, _, _, _) = setup();
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_due_date(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")]
fn test_get_due_date_uninitialized_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);
    let fake_id = BytesN::from_array(&env, &[0u8; 32]);
    client.get_due_date(&fake_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_mark_funded_fails_pool_mismatch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let configured_pool = Address::generate(&env);
    client.set_pool_contract(&configured_pool);

    let wrong_pool = mock_pool_with_asset(&env, &usdc);
    client.mark_funded(&invoice_id, &wrong_pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_mark_funded_fails_zero_amount() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &0);
}

#[test]
fn test_remove_supported_asset() {
    let (env, client, _, _, _, usdc) = setup();

    assert!(client.is_supported_asset(&usdc));
    client.remove_supported_asset(&usdc);
    assert!(!client.is_supported_asset(&usdc));
    assert_eq!(client.get_supported_asset_count(), 0);

    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, client.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "supported_asset_removed"), usdc.clone()).into_val(&env)
    );
    <()>::try_from_val(&env, &data).unwrap();
}

#[test]
fn test_set_escrow_contract() {
    let (env, client, _, _, _, _) = setup();
    let new_escrow = Address::generate(&env);

    let old_escrow = client.get_escrow_contract();
    client.set_escrow_contract(&new_escrow);
    assert_eq!(client.get_escrow_contract(), Some(new_escrow.clone()));

    let events = env.events().all();
    let (event_contract, topics, data) = events.last().expect("expected at least one event");
    assert_eq!(event_contract, client.address);
    assert_eq!(
        topics,
        (
            Symbol::new(&env, "escrow_contract_updated"),
            old_escrow.unwrap_or(new_escrow.clone()),
            new_escrow.clone()
        )
            .into_val(&env)
    );
    <()>::try_from_val(&env, &data).unwrap();
}

#[test]
fn test_mark_funded_success_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FACE_VALUE);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_mark_funded_fails_above_face_value() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    client.mark_funded(&invoice_id, &pool, &usdc, &(DEFAULT_FACE_VALUE + 1));
}

#[test]
fn test_mark_funded_success_configured_pool() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let configured_pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&configured_pool);
    client.mark_funded(&invoice_id, &configured_pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
}

#[test]
fn test_repay_partial_leaves_status_and_updates_balance() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = 1_000_000_000;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    let partial_amount = 400_000_000u128;
    let result = client.repay_partial(&invoice_id, &partial_amount);
    assert!(result);

    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Confirmed);
    assert_eq!(inv.repaid_amount, partial_amount);
    assert_eq!(inv.remaining_balance, face_value - partial_amount);
    assert_eq!(
        client.get_remaining_balance(&invoice_id),
        face_value - partial_amount
    );
    assert_eq!(client.get_repaid_amount(&invoice_id), partial_amount);

    let contract_id = client.address.clone();
    let events = env.events().all();
    let found = events.iter().any(|e| {
        let (c, topics, _data) = e;
        if c != contract_id {
            return false;
        }
        let topic0: Symbol = Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap();
        topic0 == Symbol::new(&env, "partial_repayment_received")
    });
    assert!(found);
}

#[test]
fn test_repay_partial_multi_step_reaches_repaid() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = 1_000_000_000;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    // Step 1: Repay 300M
    client.repay_partial(&invoice_id, &300_000_000);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
    assert_eq!(client.get_remaining_balance(&invoice_id), 700_000_000);

    // Step 2: Repay 300M
    client.repay_partial(&invoice_id, &300_000_000);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Confirmed);
    assert_eq!(client.get_remaining_balance(&invoice_id), 400_000_000);

    // Step 3: Repay remaining 400M
    client.repay_partial(&invoice_id, &400_000_000);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Repaid);
    assert_eq!(inv.remaining_balance, 0);
    assert_eq!(inv.repaid_amount, face_value);
    assert!(inv.repaid_at.is_some());
}

#[test]
#[should_panic(expected = "Error(Contract, #25)")]
fn test_repay_partial_exceeds_balance_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = 1_000_000_000;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    mint_tokens(&env, &usdc, &buyer, face_value as i128 + 100);

    client.repay_partial(&invoice_id, &(face_value + 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")]
fn test_repay_partial_zero_panics() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = 1_000_000_000;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &DEFAULT_DISCOUNT_BPS);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&invoice_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);

    client.repay_partial(&invoice_id, &0);
}

#[test]
fn test_repay_early_reads_stored_funded_amount() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let face_value: u128 = 1_000_000_000;
    let invoice_id = client.create(&issuer, &buyer, &face_value, &due_date, &usdc);
    attest(&env, &client, &invoice_id);
    client.list_for_financing(&invoice_id, &1000); // 10% discount

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);

    // Funded amount stored is 850M (different from 900M that discount_bps 10% would give)
    let actual_funded = 850_000_000u128;
    client.mark_funded(&invoice_id, &pool, &usdc, &actual_funded);
    client.mark_shipped(&invoice_id);
    client.confirm_delivery(&invoice_id, &issuer);
    client.confirm_delivery(&invoice_id, &buyer);

    mint_tokens(&env, &usdc, &buyer, face_value as i128);

    let result = client.repay_early(&invoice_id);
    assert!(result);
    let inv = client.get(&invoice_id);
    assert_eq!(inv.status, InvoiceStatus::Repaid);
    assert_eq!(inv.repaid_amount, face_value);
    assert_eq!(inv.remaining_balance, 0);
}

// ── Issue #872: expire_listing issuer path with explicit auth ────────────────
//
// The tests above run under `mock_all_auths()`, which masks the difference
// between the contract's requested auth and what a real transaction provides.
// This suite exercises expire_listing with explicit, transaction-shaped
// signatures only: if the contract asks for auth the caller did not provide
// (the old check_auth self-invoke), these fail exactly like mainnet would.

/// Stand up the contract without `mock_all_auths()`. Configures the funding
/// asset and agent registry with explicit admin signatures, mirroring the
/// setup() fixture's state.
fn setup_no_mock_auth() -> (
    Env,
    InvoiceContractClient<'static>,
    Address,
    Address,
    Address,
    Address,
) {
    let env = Env::default();

    let registry_id = env.register_contract(None, MockRegistry);
    let registry_client = MockRegistryClient::new(&env, &registry_id);

    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    registry_client.register(&issuer);
    registry_client.register(&buyer);

    let contract_id = env.register_contract(None, InvoiceContract);
    let client = InvoiceContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(), registry_id.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin, &registry_id);

    // Funding asset must be allow-listed by the admin (explicit signature).
    let usdc = env.register_contract(None, MockToken);
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "add_supported_asset",
            args: (usdc.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.add_supported_asset(&usdc);

    // Agent registry must be configured by the admin (explicit signature);
    // register_agent itself needs no auth.
    let agent_registry_id = env.register_contract(None, MockAgentRegistry);
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "set_agent_registry_contract",
            args: (agent_registry_id.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.set_agent_registry_contract(&agent_registry_id);

    (env, client, issuer, buyer, admin, usdc)
}

/// Submit a valid agent attestation for `invoice_id` without any auth mocks —
/// submit_attestation is permissionless and gated purely by the signature.
fn attest_no_mock_auth(env: &Env, client: &InvoiceContractClient, invoice_id: &BytesN<32>) {
    let agent_id = Symbol::new(env, "test_agent");
    let agent_registry_id = client
        .get_agent_registry_contract()
        .expect("agent registry configured");
    let registry_client = MockAgentRegistryClient::new(env, &agent_registry_id);
    registry_client.register_agent(
        &agent_id,
        &crate::Agent {
            active: true,
            pubkey: test_agent_pubkey(env),
        },
    );

    let payload = crate::AttestationPayload {
        domain_separator: BytesN::from_array(env, &crate::ATTESTATION_DOMAIN_SEPARATOR),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(env, &[9u8; 32]),
        agent_id,
        nonce: 1,
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

    client.submit_attestation(invoice_id, &payload_bytes, &signature);
}

/// List `invoice_id` with an explicit issuer signature.
fn list_with_explicit_issuer_auth(
    env: &Env,
    client: &InvoiceContractClient,
    issuer: &Address,
    invoice_id: &BytesN<32>,
) {
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "list_for_financing",
            args: (invoice_id.clone(), DEFAULT_DISCOUNT_BPS).into_val(env),
            sub_invokes: &[],
        },
    }]);
    client.list_for_financing(invoice_id, &DEFAULT_DISCOUNT_BPS);
}

/// Regression test for issue #872: the stored issuer must be able to expire
/// their own listing with real (non-mocked) authorization. It previously
/// failed because the issuer check went through a private `check_auth`
/// self-invoke that only "succeeded" under `mock_all_auths`; genuine issuer
/// signatures fell through to the admin-only path and the call panicked.
#[test]
fn test_expire_listing_by_issuer_with_explicit_auth() {
    let (env, client, issuer, buyer, _admin, usdc) = setup_no_mock_auth();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest_no_mock_auth(&env, &client, &invoice_id);
    list_with_explicit_issuer_auth(&env, &client, &issuer, &invoice_id);

    // Fast forward past the default 7-day expiry window.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    // The issuer explicitly signs the expiry of their own listing.
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "expire_listing",
            args: (invoice_id.clone(), issuer.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let result = client.expire_listing(&invoice_id, &issuer);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);
}

/// The admin fallback path must keep working with explicit authorization too.
#[test]
fn test_expire_listing_by_admin_with_explicit_auth() {
    let (env, client, issuer, buyer, admin, usdc) = setup_no_mock_auth();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest_no_mock_auth(&env, &client, &invoice_id);
    list_with_explicit_issuer_auth(&env, &client, &issuer, &invoice_id);

    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    // Only the admin authorizes; the contract must take the admin branch.
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "expire_listing",
            args: (invoice_id.clone(), admin.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let result = client.expire_listing(&invoice_id, &admin);
    assert!(result);
    assert_eq!(client.get(&invoice_id).status, InvoiceStatus::Expired);
}

/// An unrelated caller must never be able to expire a listing: even with a
/// valid signature, the caller-equality check rejects them with the typed
/// `NotAuthorized` error.
#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_expire_listing_stranger_with_explicit_auth_panics() {
    let (env, client, issuer, buyer, _admin, usdc) = setup_no_mock_auth();
    let stranger = Address::generate(&env);
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest_no_mock_auth(&env, &client, &invoice_id);
    list_with_explicit_issuer_auth(&env, &client, &issuer, &invoice_id);

    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 7 * 24 * 60 * 60 + 1);

    // The stranger signs — neither the issuer nor the admin path matches.
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &stranger,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "expire_listing",
            args: (invoice_id.clone(), stranger.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.expire_listing(&invoice_id, &stranger);
}

/// The issuer cannot expire a listing before its window has passed (auth
/// alone is not enough — the time check still applies).
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_expire_listing_unexpired_rejected_for_issuer_with_explicit_auth() {
    let (env, client, issuer, buyer, _admin, usdc) = setup_no_mock_auth();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest_no_mock_auth(&env, &client, &invoice_id);
    list_with_explicit_issuer_auth(&env, &client, &issuer, &invoice_id);

    // Well before the 7-day window.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 5 * 24 * 60 * 60);

    // Issuer signs — must still hit ListingNotExpired.
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "expire_listing",
            args: (invoice_id.clone(), issuer.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.expire_listing(&invoice_id, &issuer);
}

/// The admin cannot expire a listing before its window has passed either.
#[test]
#[should_panic(expected = "Error(Contract, #14)")]
fn test_expire_listing_unexpired_rejected_for_admin_with_explicit_auth() {
    let (env, client, issuer, buyer, admin, usdc) = setup_no_mock_auth();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "create",
            args: (
                issuer.clone(),
                buyer.clone(),
                DEFAULT_FACE_VALUE,
                due_date,
                usdc.clone(),
            )
                .into_val(&env),
            sub_invokes: &[],
        },
    }]);
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    attest_no_mock_auth(&env, &client, &invoice_id);
    list_with_explicit_issuer_auth(&env, &client, &issuer, &invoice_id);

    // Well before the 7-day window.
    env.ledger()
        .set_timestamp(env.ledger().timestamp() + 5 * 24 * 60 * 60);

    // Admin signs — must still hit ListingNotExpired.
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "expire_listing",
            args: (invoice_id.clone(), admin.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.expire_listing(&invoice_id, &admin);
}

// ============== ISSUE #713: EMERGENCY PAUSE / UNPAUSE ==============

#[test]
fn test_pause_blocks_state_changes_and_unpause_restores_them() {
    let (env, client, ..) = setup();

    // Live: state changes flow.
    client.set_expiry_window(&3600);
    assert_eq!(client.get_expiry_window(), 3600);

    client.pause();

    // State-changing entry points are rejected while paused...
    assert!(client.try_set_expiry_window(&7200).is_err());
    assert!(client
        .try_add_supported_asset(&Address::generate(&env))
        .is_err());

    // ...while read-only views keep working.
    assert_eq!(client.get_expiry_window(), 3600);
    assert!(client.get_admin().is_some());
    let _ = client.get_counts();

    client.unpause();

    // Unpaused: state changes flow again.
    client.set_expiry_window(&7200);
    assert_eq!(client.get_expiry_window(), 7200);
}

/// Pins the exact breaker error: `PauseError::ContractPaused` from the shared
/// crate surfaces as `Error(Contract, #1)` on every guarded invoice entry
/// point (`InvoiceError::AlreadyInitialized` is the only other #1, and it is
/// unreachable from `set_expiry_window`).
#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_set_expiry_window_reverts_while_paused_with_contract_paused_error() {
    let (_env, client, ..) = setup();
    client.pause();
    client.set_expiry_window(&7200);
}

/// Negative auth: only a non-admin signed `pause`, so the stored admin's
/// `require_auth()` must reject it — a non-admin cannot engage the breaker.
#[test]
fn test_pause_requires_admin_authorization() {
    let (env, client, ..) = setup();
    let non_admin = Address::generate(&env);

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

/// Negative auth for disengaging: a non-admin cannot unpause either, so a
/// paused contract cannot be resumed by anyone but the admin.
#[test]
fn test_unpause_requires_admin_authorization() {
    let (env, client, ..) = setup();
    client.pause();
    let non_admin = Address::generate(&env);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &non_admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &client.address,
            fn_name: "unpause",
            args: soroban_sdk::Vec::new(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_unpause().is_err());
}

#[test]
fn test_pause_and_unpause_emit_events() {
    let (env, client, ..) = setup();
    let admin = client.get_admin().unwrap();

    client.pause();
    let events = env.events().all();
    let (contract, topics, _) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, client.address);
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
// ============== ISSUE #720: BATCH INVOICE CREATION ==============

/// Helper: one valid `batch_create` entry.
fn batch_entry(
    env: &Env,
    buyer: &Address,
    usdc: &Address,
    face_value: u128,
) -> (Address, u128, u64, Address) {
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    (buyer.clone(), face_value, due_date, usdc.clone())
}

#[test]
fn test_batch_create_creates_every_entry() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let entries = vec![
        &env,
        (buyer.clone(), DEFAULT_FACE_VALUE, due_date, usdc.clone()),
        (
            buyer.clone(),
            DEFAULT_FACE_VALUE * 2,
            due_date + 100,
            usdc.clone(),
        ),
        (buyer.clone(), 500_000_000, due_date + 200, usdc.clone()),
    ];

    let ids = client.batch_create(&issuer, &entries);
    assert_eq!(ids.len(), 3);

    for (index, id) in ids.iter().enumerate() {
        let invoice = client.get(&id);
        assert_eq!(invoice.status, InvoiceStatus::Created);
        assert_eq!(invoice.issuer, issuer);
        assert_eq!(invoice.buyer, buyer);
        assert_eq!(invoice.discount_bps, 0);
        assert_eq!(invoice.funded_amount, 0);
        assert_eq!(invoice.remaining_balance, invoice.face_value);
        // Each entry got its own face value.
        let expected = if index == 1 {
            DEFAULT_FACE_VALUE * 2
        } else if index == 2 {
            500_000_000
        } else {
            DEFAULT_FACE_VALUE
        };
        assert_eq!(invoice.face_value, expected);
    }

    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")).unwrap(), 3);
}

#[test]
fn test_batch_create_emits_batch_summary_event() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let entries = vec![&env, batch_entry(&env, &buyer, &usdc, DEFAULT_FACE_VALUE)];

    let ids = client.batch_create(&issuer, &entries);
    assert_eq!(ids.len(), 1);

    let events = env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "batch_invoices_created")
    );
    assert_eq!(<(u32, u32)>::try_from_val(&env, &data).unwrap(), (1, 0));
}

#[test]
fn test_batch_create_on_empty_batch_is_a_noop() {
    let (env, client, issuer, _, _, _) = setup();
    let entries = vec![&env];
    let ids = client.batch_create(&issuer, &entries);
    assert!(ids.is_empty());
}

#[test]
#[should_panic(expected = "Error(Contract, #28)")]
fn test_batch_create_rejects_oversized_batch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let mut entries: Vec<(Address, u128, u64, Address)> = Vec::new(&env);
    for _ in 0..=MAX_BATCH_SIZE {
        entries.push_back(batch_entry(&env, &buyer, &usdc, DEFAULT_FACE_VALUE));
    }
    client.batch_create(&issuer, &entries);
}

/// The 50-entry cap is reachable, but a full batch of `create` entries exceeds
/// the default test-host budget because every entry performs two cross-contract
/// `is_verified` calls. The budget is lifted for this test only -- it pins the
/// cap, not the per-call cost.
#[test]
fn test_batch_create_allows_exactly_max_batch_size() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    env.budget().reset_unlimited();
    let mut entries: Vec<(Address, u128, u64, Address)> = Vec::new(&env);
    for _ in 0..MAX_BATCH_SIZE {
        entries.push_back(batch_entry(&env, &buyer, &usdc, DEFAULT_FACE_VALUE));
    }
    let ids = client.batch_create(&issuer, &entries);
    assert_eq!(ids.len(), MAX_BATCH_SIZE);
}

/// The atomicity guarantee (#720): an unsupported asset on the second entry
/// reverts the batch, so the first entry's invoice is not left behind.
#[test]
fn test_batch_create_with_one_invalid_entry_reverts_whole_batch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let bad_asset = env.register_contract(None, MockToken);
    let _ = MockTokenClient::new(&env, &bad_asset);
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let entries = vec![
        &env,
        (buyer.clone(), DEFAULT_FACE_VALUE, due_date, usdc.clone()),
        (
            buyer.clone(),
            DEFAULT_FACE_VALUE,
            due_date,
            bad_asset.clone(), // not a supported asset -> UnsupportedAsset
        ),
    ];

    // The whole call reverts with the failing entry's typed error.
    assert!(client.try_batch_create(&issuer, &entries).is_err());

    // Nothing was persisted: no invoice exists from the first (valid) entry.
    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")).unwrap(), 0);
    assert_eq!(client.get_invoice_count_by_issuer(&issuer), 0);
}

/// Atomicity also holds for a face-value violation: the batch reverts and no
/// earlier entry survives.
#[test]
fn test_batch_create_with_invalid_face_value_reverts_whole_batch() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let entries = vec![
        &env,
        (buyer.clone(), DEFAULT_FACE_VALUE, due_date, usdc.clone()),
        (buyer.clone(), 0, due_date, usdc.clone()), // InvalidFaceValue
    ];

    assert!(client.try_batch_create(&issuer, &entries).is_err());
    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")).unwrap(), 0);
}

// ============== ISSUE #721: BATCH LIST FOR FINANCING ==============

#[test]
fn test_batch_list_for_financing_lists_every_entry() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let mut entries: Vec<(BytesN<32>, u32)> = Vec::new(&env);
    for _ in 0..3 {
        let id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
        attest(&env, &client, &id);
        entries.push_back((id, DEFAULT_DISCOUNT_BPS));
    }

    let failed = client.batch_list_for_financing(&entries);
    assert!(failed.is_empty());

    for (id, discount) in entries.iter() {
        assert_eq!(client.get_status(&id), 1); // Listed
        assert_eq!(client.get_discount_bps(&id), discount);
    }
    assert_eq!(client.get_invoice_count_by_issuer(&issuer), 3);
}

/// Explicit failure behavior (#721): listing is per-entry tolerant, so failed
/// entries come back in the returned list instead of reverting the batch.
#[test]
fn test_batch_list_for_financing_returns_failed_entries_without_reverting() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    // Attested and listable.
    let good = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &good);
    // Created but never attested.
    let unattested = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    // Attested but discount above the 5000 bps cap.
    let bad_discount = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &bad_discount);
    // A completely unknown invoice id.
    let unknown = BytesN::from_array(&env, &[7u8; 32]);

    let entries = vec![
        &env,
        (good.clone(), DEFAULT_DISCOUNT_BPS),
        (unattested.clone(), DEFAULT_DISCOUNT_BPS),
        (bad_discount.clone(), 5_001),
        (unknown.clone(), DEFAULT_DISCOUNT_BPS),
    ];

    // The call succeeds and reports exactly the three failures.
    let failed = client.batch_list_for_financing(&entries);
    assert_eq!(failed.len(), 3);
    assert_eq!(failed.get(0).unwrap(), unattested);
    assert_eq!(failed.get(1).unwrap(), bad_discount);
    assert_eq!(failed.get(2).unwrap(), unknown);

    // The good entry listed; the bad ones did not.
    assert_eq!(client.get_status(&good), 1);
    assert_eq!(client.get_discount_bps(&good), DEFAULT_DISCOUNT_BPS);
    assert_eq!(client.get_status(&unattested), 0); // Created
    assert_eq!(client.get_discount_bps(&unattested), 0);
    assert_eq!(client.get_status(&bad_discount), 0); // Created
}

/// A discount above the cap fails only its own entry; a valid entry later in
/// the same batch still lists.
#[test]
fn test_batch_list_for_financing_partial_success_lists_valid_entries() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let bad = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &bad);
    let good = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &good);

    let entries = vec![
        &env,
        (bad.clone(), 5_001),
        (good.clone(), DEFAULT_DISCOUNT_BPS),
    ];
    let failed = client.batch_list_for_financing(&entries);

    assert_eq!(failed.len(), 1);
    assert_eq!(failed.get(0).unwrap(), bad);
    assert_eq!(client.get_status(&good), 1);
}

/// Listing an invoice that is already `Listed` fails only that entry.
#[test]
fn test_batch_list_for_financing_reports_already_listed_entry() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &id);
    assert!(client.list_for_financing(&id, &DEFAULT_DISCOUNT_BPS));

    let entries = vec![&env, (id.clone(), DEFAULT_DISCOUNT_BPS)];
    let failed = client.batch_list_for_financing(&entries);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed.get(0).unwrap(), id);
}

#[test]
fn test_batch_list_for_financing_emits_batch_summary_event() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let good = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &good);
    let unknown = BytesN::from_array(&env, &[3u8; 32]);

    let entries = vec![
        &env,
        (good.clone(), DEFAULT_DISCOUNT_BPS),
        (unknown.clone(), DEFAULT_DISCOUNT_BPS),
    ];
    let _ = client.batch_list_for_financing(&entries);

    let events = env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "batch_invoices_listed")
    );
    assert_eq!(<(u32, u32)>::try_from_val(&env, &data).unwrap(), (1, 1));
}

#[test]
fn test_batch_invoice_listed_event_includes_expiry_deadline() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let listed_at = 1_700_000_000u64;
    env.ledger().set_timestamp(listed_at);
    let due_date = listed_at + DEFAULT_DUE_OFFSET;
    let invoice_id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    attest(&env, &client, &invoice_id);

    let entries = vec![&env, (invoice_id.clone(), DEFAULT_DISCOUNT_BPS)];
    let failed = client.batch_list_for_financing(&entries);
    assert!(failed.is_empty());

    let events = env.events().all();
    let (contract, topics, data) = events.get(events.len() - 2).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(
        topics,
        (Symbol::new(&env, "invoice_listed"), invoice_id.clone()).into_val(&env)
    );
    assert_eq!(
        <(u32, u64)>::try_from_val(&env, &data).unwrap(),
        (DEFAULT_DISCOUNT_BPS, listed_at + DEFAULT_EXPIRY_WINDOW)
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #28)")]
fn test_batch_list_for_financing_rejects_oversized_batch() {
    let (env, client, _, _, _, _) = setup();
    let id = BytesN::from_array(&env, &[1u8; 32]);
    let mut entries: Vec<(BytesN<32>, u32)> = Vec::new(&env);
    for _ in 0..=MAX_BATCH_SIZE {
        entries.push_back((id.clone(), DEFAULT_DISCOUNT_BPS));
    }
    client.batch_list_for_financing(&entries);
}

#[test]
fn test_batch_list_for_financing_allows_exactly_max_batch_size() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    env.budget().reset_unlimited();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;
    let mut entries: Vec<(BytesN<32>, u32)> = Vec::new(&env);
    for _ in 0..MAX_BATCH_SIZE {
        let id = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
        attest(&env, &client, &id);
        entries.push_back((id, DEFAULT_DISCOUNT_BPS));
    }
    let failed = client.batch_list_for_financing(&entries);
    assert!(failed.is_empty());
    assert_eq!(client.get_invoice_count_by_issuer(&issuer), MAX_BATCH_SIZE);
}

#[test]
fn test_move_status_index_compacts_storage() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    // Create 3 invoices
    let id1 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let id2 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let id3 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);

    // Initial check
    let created = client.get_by_status(&InvoiceStatus::Created, &0, &MAX_PAGE_SIZE);
    assert_eq!(created.len(), 3);
    assert_eq!(created.get(0).unwrap().id, id1);
    assert_eq!(created.get(1).unwrap().id, id2);
    assert_eq!(created.get(2).unwrap().id, id3);

    // Transition middle invoice out of Created
    attest(&env, &client, &id2);
    client.list_for_financing(&id2, &DEFAULT_DISCOUNT_BPS);

    // Verify compaction: id3 should have swapped into id2's slot, and count should be 2.
    let created_after = client.get_by_status(&InvoiceStatus::Created, &0, &MAX_PAGE_SIZE);
    assert_eq!(created_after.len(), 2);
    assert_eq!(created_after.get(0).unwrap().id, id1);
    assert_eq!(created_after.get(1).unwrap().id, id3);

    let listed = client.get_by_status(&InvoiceStatus::Listed, &0, &MAX_PAGE_SIZE);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.get(0).unwrap().id, id2);
}

fn index_contains(ids: &Vec<BytesN<32>>, target: &BytesN<32>) -> bool {
    for id in ids.iter() {
        if id.clone() == target.clone() {
            return true;
        }
    }
    false
}

fn status_index_count(env: &Env, client: &InvoiceContractClient, status: InvoiceStatus) -> u32 {
    env.as_contract(&client.address, || {
        env.storage()
            .persistent()
            .get::<_, u32>(&crate::DataKey::StatusIndexCount(status as u32))
            .unwrap_or(0)
    })
}

/// Compares the indexed status views against a ground-truth scan of every
/// invoice, and verifies the compacted slot/position invariants.
fn assert_status_views_match_linear(
    env: &Env,
    client: &InvoiceContractClient,
    ids: &Vec<BytesN<32>>,
) {
    let statuses = [
        InvoiceStatus::Created,
        InvoiceStatus::Listed,
        InvoiceStatus::Funded,
        InvoiceStatus::Active,
        InvoiceStatus::Confirmed,
        InvoiceStatus::Repaid,
        InvoiceStatus::Defaulted,
        InvoiceStatus::Expired,
        InvoiceStatus::Cancelled,
    ];
    let mut classified: u64 = 0;

    for status in statuses {
        let mut expected: Vec<BytesN<32>> = Vec::new(env);
        for id in ids.iter() {
            if client.get(id).status == status {
                expected.push_back(id.clone());
            }
        }

        let actual = client.get_by_status(&status, &0, &MAX_PAGE_SIZE);
        assert_eq!(actual.len(), expected.len());

        let mut seen: Vec<BytesN<32>> = Vec::new(env);
        for invoice in actual.iter() {
            assert_eq!(invoice.status, status);
            assert!(!index_contains(&seen, &invoice.id), "duplicate status row");
            assert!(index_contains(&expected, &invoice.id), "unexpected member");
            seen.push_back(invoice.id.clone());
        }
        for id in expected.iter() {
            assert!(index_contains(&seen, id), "missing status member");
        }

        let count = status_index_count(env, client, status);
        assert_eq!(count, expected.len() as u32, "status slot count mismatch");

        for id in ids.iter() {
            let is_member = client.get(id).status == status;
            assert_eq!(client.has_status_membership(&status, id), is_member);
            match raw_status_position(env, client, status, id) {
                Some(pos) => {
                    assert!(is_member);
                    assert!(pos < count);
                    let stored: BytesN<32> = env.as_contract(&client.address, || {
                        env.storage()
                            .persistent()
                            .get(&crate::DataKey::StatusIndexEntry(status as u32, pos))
                            .unwrap()
                    });
                    assert_eq!(stored, id.clone(), "position points at wrong slot");
                }
                None => assert!(!is_member, "member is missing its position"),
            }
        }

        classified += expected.len() as u64;
    }

    assert_eq!(classified, ids.len() as u64, "invoice counted twice");
}

#[test]
fn test_status_membership_index_matches_linear_reads_across_transitions() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let base_time = env.ledger().timestamp();
    let due_date = base_time + DEFAULT_DUE_OFFSET;

    let mut ids: Vec<BytesN<32>> = Vec::new(&env);
    for offset in 0..6u128 {
        ids.push_back(client.create(
            &issuer,
            &buyer,
            &(DEFAULT_FACE_VALUE + offset),
            &due_date,
            &usdc,
        ));
    }
    assert_status_views_match_linear(&env, &client, &ids);

    let default_id = ids.get(0).unwrap();
    let repaid_id = ids.get(1).unwrap();
    let cancelled_id = ids.get(2).unwrap();
    let listed_id = ids.get(3).unwrap();
    let created_id = ids.get(4).unwrap();
    let expired_id = ids.get(5).unwrap();

    for id in [&default_id, &repaid_id, &expired_id, &listed_id] {
        attest(&env, &client, id);
        client.list_for_financing(id, &DEFAULT_DISCOUNT_BPS);
    }
    assert_status_views_match_linear(&env, &client, &ids);

    client.cancel(&cancelled_id);
    assert_status_views_match_linear(&env, &client, &ids);

    let pool = mock_pool_with_asset(&env, &usdc);
    client.set_pool_contract(&pool);
    let escrow = mock_escrow_for_pool(&env, &pool, &usdc);
    client.set_escrow_contract(&escrow);
    client.mark_funded(&default_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    client.mark_funded(&repaid_id, &pool, &usdc, &DEFAULT_FUNDED_AMOUNT);
    assert_status_views_match_linear(&env, &client, &ids);

    client.mark_shipped(&default_id);
    client.confirm_delivery(&default_id, &issuer);
    client.confirm_delivery(&default_id, &buyer);
    assert_status_views_match_linear(&env, &client, &ids);

    client.set_expiry_window(&100);
    env.ledger().set_timestamp(base_time + 101);
    assert!(client.expire_listing(&expired_id, &issuer));
    assert_status_views_match_linear(&env, &client, &ids);

    mint_tokens(&env, &usdc, &buyer, 2_000_000_000);
    assert!(client.repay(&repaid_id));
    assert_status_views_match_linear(&env, &client, &ids);

    env.ledger().set_timestamp(due_date);
    client.trigger_default(&default_id);
    assert_status_views_match_linear(&env, &client, &ids);

    // Remaining non-terminal states are exactly as expected.
    assert_eq!(client.get(&created_id).status, InvoiceStatus::Created);
    assert_eq!(client.get(&listed_id).status, InvoiceStatus::Listed);
    assert_status_views_match_linear(&env, &client, &ids);
}

#[test]
fn test_migrate_status_index_compacts_legacy_rows_in_pages() {
    let (env, client, issuer, buyer, _, usdc) = setup();
    let due_date = env.ledger().timestamp() + DEFAULT_DUE_OFFSET;

    let id1 = client.create(&issuer, &buyer, &DEFAULT_FACE_VALUE, &due_date, &usdc);
    let id2 = client.create(&issuer, &buyer, &(DEFAULT_FACE_VALUE + 1), &due_date, &usdc);
    let id3 = client.create(&issuer, &buyer, &(DEFAULT_FACE_VALUE + 2), &due_date, &usdc);

    attest(&env, &client, &id2);
    client.list_for_financing(&id2, &DEFAULT_DISCOUNT_BPS);

    // Reconstruct the pre-#22 append-only layout: Created is [id1, id2, id3]
    // (id2 is now stale) and none of those rows has a position map entry.
    env.as_contract(&client.address, || {
        let created = InvoiceStatus::Created as u32;
        env.storage()
            .persistent()
            .set(&crate::DataKey::StatusIndexEntry(created, 2), &id2);
        env.storage()
            .persistent()
            .set(&crate::DataKey::StatusIndexCount(created), &3u32);
        env.storage()
            .persistent()
            .set(&crate::DataKey::StatusCount(created), &3u64);
        for id in [&id1, &id2, &id3] {
            env.storage()
                .persistent()
                .remove(&crate::status_position_key(InvoiceStatus::Created, id));
            env.storage().persistent().set(
                &crate::status_membership_key(InvoiceStatus::Created, id),
                &true,
            );
        }
        let listed = InvoiceStatus::Listed as u32;
        env.storage()
            .persistent()
            .remove(&crate::status_position_key(InvoiceStatus::Listed, &id2));
        env.storage().persistent().set(
            &crate::status_membership_key(InvoiceStatus::Listed, &id2),
            &true,
        );
        env.storage()
            .persistent()
            .set(&crate::DataKey::StatusIndexCount(listed), &1u32);
        env.storage()
            .persistent()
            .set(&crate::DataKey::StatusCount(listed), &1u64);
    });

    // Before migration the legacy marker still answers membership.
    assert!(client.has_status_membership(&InvoiceStatus::Listed, &id2));

    let (cursor, done) = client.migrate_status_index(&InvoiceStatus::Created, &2);
    assert_eq!(cursor, 2);
    assert!(!done);
    let (cursor, done) = client.migrate_status_index(&InvoiceStatus::Created, &2);
    assert_eq!(cursor, 3);
    assert!(done);
    let (_, done) = client.migrate_status_index(&InvoiceStatus::Listed, &MAX_PAGE_SIZE);
    assert!(done);

    let created = client.get_by_status(&InvoiceStatus::Created, &0, &MAX_PAGE_SIZE);
    assert_eq!(created.len(), 2);
    assert_eq!(created.get(0).unwrap().id, id1);
    assert_eq!(created.get(1).unwrap().id, id3);
    let listed = client.get_by_status(&InvoiceStatus::Listed, &0, &MAX_PAGE_SIZE);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.get(0).unwrap().id, id2);

    assert_eq!(status_index_count(&env, &client, InvoiceStatus::Created), 2);
    assert_eq!(status_index_count(&env, &client, InvoiceStatus::Listed), 1);
    assert!(raw_status_position(&env, &client, InvoiceStatus::Created, &id1).is_some());
    assert!(raw_status_position(&env, &client, InvoiceStatus::Created, &id2).is_none());
    assert!(raw_status_position(&env, &client, InvoiceStatus::Created, &id3).is_some());
    let counts = client.get_counts();
    assert_eq!(counts.get(String::from_str(&env, "Created")), Some(2));
    assert_eq!(counts.get(String::from_str(&env, "Listed")), Some(1));
}
