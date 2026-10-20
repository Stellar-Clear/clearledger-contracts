#![no_std]
#![cfg(not(target_arch = "wasm32"))]

use soroban_sdk::{
    contract, contractimpl, contracttype, testutils::Address as _, Address, BytesN, Env, Map,
    String, Symbol,
};

pub const TEST_AGENT_SEED: [u8; 32] = [7u8; 32];

pub fn test_agent_signing_key() -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_slice(&TEST_AGENT_SEED).unwrap()
}

pub fn test_agent_pubkey(env: &Env) -> BytesN<65> {
    let point = test_agent_signing_key()
        .verifying_key()
        .to_encoded_point(false);
    let mut bytes = [0u8; 65];
    bytes.copy_from_slice(point.as_bytes());
    BytesN::from_array(env, &bytes)
}

pub fn test_agent_id(env: &Env) -> Symbol {
    Symbol::new(env, "test_agent")
}

// ---------------- Mock Token ----------------
#[contract]
pub struct MockToken;

#[contracttype]
pub struct TKey(pub Address);

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

// ---------------- Mock Agent Registry ----------------
#[contract]
pub struct MockAgentRegistry;

#[contracttype]
pub struct AgentKey(pub Symbol);

#[contractimpl]
impl MockAgentRegistry {
    pub fn get_agent(env: Env, agent_id: Symbol) -> Option<clearledger_invoice::Agent> {
        env.storage().persistent().get(&AgentKey(agent_id))
    }

    pub fn register_agent(env: Env, agent_id: Symbol, agent: clearledger_invoice::Agent) {
        env.storage().persistent().set(&AgentKey(agent_id), &agent);
    }
}

pub struct FullProtocol {
    pub env: Env,
    pub registry: clearledger_registry::RegistryContractClient<'static>,
    pub registry_id: Address,
    pub invoice: clearledger_invoice::InvoiceContractClient<'static>,
    pub invoice_id: Address,
    pub pool: clearledger_pool::PoolContractClient<'static>,
    pub pool_id: Address,
    pub escrow: clearledger_escrow::EscrowContractClient<'static>,
    pub escrow_id: Address,
    pub admin: Address,
    pub issuer: Address,
    pub buyer: Address,
    pub lp: Address,
    pub usdc_id: Address,
}

pub fn setup_full_protocol() -> FullProtocol {
    let env = Env::default();
    env.mock_all_auths_allowing_non_root_auth();

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let lp = Address::generate(&env);

    // 1. Registry
    let registry_id = env.register_contract(None, clearledger_registry::RegistryContract);
    let registry = clearledger_registry::RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);

    let empty_meta = Map::new(&env);
    registry.register_issuer(&issuer, &empty_meta);
    registry.register_buyer(&buyer, &empty_meta);

    // 2. Token
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

    // 3. Contracts
    let invoice_id = env.register_contract(None, clearledger_invoice::InvoiceContract);
    let escrow_id = env.register_contract(None, clearledger_escrow::EscrowContract);
    let pool_id = env.register_contract(None, clearledger_pool::PoolContract);

    let invoice = clearledger_invoice::InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);

    let escrow = clearledger_escrow::EscrowContractClient::new(&env, &escrow_id);
    escrow.initialize(&admin, &pool_id, &usdc_id);

    let pool = clearledger_pool::PoolContractClient::new(&env, &pool_id);
    pool.initialize(
        &admin,
        &invoice_id,
        &escrow_id,
        &usdc_id,
        &registry_id,
        &admin,
        &10_000_000,
        &String::from_str(&env, "ClearLedger USDC Pool Shares"),
        &String::from_str(&env, "TT-USDC"),
        &7,
    );

    invoice.add_supported_asset(&usdc_id);
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

    pool.set_max_utilization(&admin, &10000);

    FullProtocol {
        env,
        registry,
        registry_id,
        invoice,
        invoice_id,
        pool,
        pool_id,
        escrow,
        escrow_id,
        admin,
        issuer,
        buyer,
        lp,
        usdc_id,
    }
}
