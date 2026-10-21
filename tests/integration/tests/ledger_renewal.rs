use clearledger_escrow::{EscrowContract, EscrowContractClient, EscrowRecord};
use clearledger_invoice::{Invoice, InvoiceContract, InvoiceContractClient, InvoiceStatus};
use clearledger_pool::{
    PoolContract, PoolContractClient, DEFAULT_MIN_INITIAL_DEPOSIT, DEFAULT_SHARE_DECIMALS,
};
use clearledger_registry::{
    RegistryContract, RegistryContractClient, TTL_EXTEND_TO, TTL_THRESHOLD,
};
use soroban_sdk::{
    testutils::{
        storage::{Instance as _, Persistent as _},
        Address as _, Ledger as _,
    },
    Address, BytesN, Env, Map, String, Symbol, Val, Vec,
};
fn targets(env: &Env) -> (Address, [Address; 4]) {
    env.mock_all_auths(); // Test-only authorization for initialized state.
    let admin = Address::generate(env);
    let registry = env.register_contract(None, RegistryContract);
    let invoice = env.register_contract(None, InvoiceContract);
    let escrow = env.register_contract(None, EscrowContract);
    let pool = env.register_contract(None, PoolContract);
    let asset = Address::generate(env);
    RegistryContractClient::new(env, &registry).initialize(&admin);
    InvoiceContractClient::new(env, &invoice).initialize(&admin, &registry);
    EscrowContractClient::new(env, &escrow).initialize(&admin, &pool, &asset);
    PoolContractClient::new(env, &pool).initialize(
        &admin,
        &invoice,
        &escrow,
        &asset,
        &registry,
        &admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(env, "Test shares"),
        &String::from_str(env, "TEST"),
        &DEFAULT_SHARE_DECIMALS,
    );

    (admin, [registry, invoice, escrow, pool])
}

#[test]
fn storage_remains_readable_on_both_sides_of_renewal_threshold() {
    for remaining in [TTL_THRESHOLD + 1, TTL_THRESHOLD, TTL_THRESHOLD - 1] {
        let env = Env::default();
        let (admin, ids) = targets(&env);
        let owner = Address::generate(&env);
        let invoice_id = BytesN::from_array(&env, &[42; 32]);
        let asset = Address::generate(&env);
        let invoice = Invoice {
            id: invoice_id.clone(),
            issuer: owner.clone(),
            buyer: admin.clone(),
            face_value: 100,
            discount_bps: 0,
            funded_amount: 0,
            due_date: 86400,
            status: InvoiceStatus::Created,
            created_at: 0,
            listed_at: None,
            funded_at: None,
            shipped_at: None,
            issuer_confirmed: false,
            buyer_confirmed: false,
            repaid_at: None,
            funding_asset: asset,
            funding_pool: None,
            repaid_amount: 0,
            remaining_balance: 100,
        };
        // Seed valid record shapes directly: this suite tests storage lifetime,
        // independently of attestation, token transfer and financing workflows.
        let profile_key = clearledger_registry::DataKey::Profile(owner.clone());
        env.as_contract(&ids[0], || {
            let profile = clearledger_registry::Profile::new(
                clearledger_registry::Role::Issuer,
                true,
                0,
                Map::new(&env),
            );
            env.storage().persistent().set(&profile_key, &profile);
            env.storage()
                .persistent()
                .extend_ttl(&profile_key, TTL_EXTEND_TO, TTL_EXTEND_TO);
        });
        let invoice_key = clearledger_invoice::DataKey::Invoice(invoice_id.clone());
        env.as_contract(&ids[1], || {
            env.storage().persistent().set(&invoice_key, &invoice);
            env.storage()
                .persistent()
                .extend_ttl(&invoice_key, TTL_EXTEND_TO, TTL_EXTEND_TO);
        });
        let escrow_key = clearledger_escrow::DataKey::Locked(invoice_id.clone());
        env.as_contract(&ids[2], || {
            env.storage().persistent().set(
                &escrow_key,
                &EscrowRecord {
                    invoice_id: invoice_id.clone(),
                    amount: 77,
                    locked_at: 0,
                    issuer: owner.clone(),
                },
            );
            env.storage()
                .persistent()
                .extend_ttl(&escrow_key, TTL_EXTEND_TO, TTL_EXTEND_TO);
        });
        let shares_key = clearledger_pool::DataKey::LPShares(owner.clone());
        env.as_contract(&ids[3], || {
            env.storage().persistent().set(&shares_key, &10u128);
            env.storage()
                .persistent()
                .extend_ttl(&shares_key, TTL_EXTEND_TO, TTL_EXTEND_TO);
        });
        let initial = env.as_contract(&ids[0], || env.storage().instance().get_ttl());
        env.ledger()
            .set_sequence_number(env.ledger().sequence() + initial - remaining);
        for id in &ids {
            let before = env.as_contract(id, || env.storage().instance().get_ttl());
            assert_eq!(before, remaining);
            env.invoke_contract::<()>(id, &Symbol::new(&env, "pause"), Vec::<Val>::new(&env));
            let after = env.as_contract(id, || env.storage().instance().get_ttl());
            assert_eq!(
                after,
                if remaining <= TTL_THRESHOLD {
                    TTL_EXTEND_TO
                } else {
                    remaining
                }
            );
        }
        assert_eq!(
            RegistryContractClient::new(&env, &ids[0])
                .get_profile(&owner)
                .role(),
            clearledger_registry::Role::Issuer
        );
        assert_eq!(
            InvoiceContractClient::new(&env, &ids[1])
                .get(&invoice_id)
                .face_value,
            100
        );
        assert_eq!(
            EscrowContractClient::new(&env, &ids[2]).get_locked(&invoice_id),
            77
        );
        assert_eq!(
            PoolContractClient::new(&env, &ids[3])
                .get_lp_position(&owner)
                .shares,
            10
        );
        // Registry/pool getters renew existing persistent rows. Invoice/escrow
        // views are intentionally read-only; the test pins their current policy.
        let renewed = if remaining <= TTL_THRESHOLD {
            TTL_EXTEND_TO
        } else {
            remaining
        };
        assert_eq!(
            env.as_contract(&ids[0], || env.storage().persistent().get_ttl(&profile_key)),
            renewed
        );
        assert_eq!(
            env.as_contract(&ids[3], || env.storage().persistent().get_ttl(&shares_key)),
            renewed
        );
        assert_eq!(
            env.as_contract(&ids[1], || env.storage().persistent().get_ttl(&invoice_key)),
            remaining
        );
        assert_eq!(
            env.as_contract(&ids[2], || env.storage().persistent().get_ttl(&escrow_key)),
            remaining
        );
    }
}
