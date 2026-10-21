//! No mock-all authorization is active during any adversarial invocation.
use clearledger_escrow::{EscrowContract, EscrowContractClient};
use clearledger_invoice::{InvoiceContract, InvoiceContractClient};
use clearledger_pool::{
    PoolContract, PoolContractClient, DEFAULT_MIN_INITIAL_DEPOSIT, DEFAULT_SHARE_DECIMALS,
};
use clearledger_registry::{RegistryContract, RegistryContractClient};
use soroban_sdk::{
    contract, contractimpl, testutils::Address as _, vec, xdr::ScErrorType, Address, BytesN, Env,
    Error, IntoVal, String, Symbol, Val, Vec,
};

#[contract]
pub struct UntrustedRelay;
#[contractimpl]
impl UntrustedRelay {
    pub fn forward(env: Env, target: Address, method: Symbol, args: Vec<Val>) -> Val {
        env.invoke_contract(&target, &method, args)
    }
}

fn targets(env: &Env) -> (Address, [Address; 4]) {
    env.mock_all_auths(); // Initialization only; cleared before returning.
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
    env.mock_auths(&[]);
    (admin, [registry, invoice, escrow, pool])
}

fn rejected(env: &Env, target: &Address, method: &str, args: Vec<Val>) {
    let result = env.try_invoke_contract::<Val, Error>(target, &Symbol::new(env, method), args);
    // Reject specifically at the authorization boundary, not at a later business
    // validation check (notably the deliberately uninstalled upgrade hash).
    assert!(matches!(result, Err(Ok(error)) if error.is_type(ScErrorType::Auth)));
}

#[test]
fn missing_account_authorization_cannot_pause_or_unpause() {
    let env = Env::default();
    let (admin, contracts) = targets(&env);
    for target in &contracts {
        rejected(&env, target, "pause", vec![&env]);
        rejected(&env, target, "unpause", vec![&env]);
    }
    assert_eq!(
        RegistryContractClient::new(&env, &contracts[0]).get_admin(),
        admin
    );
    assert_eq!(
        InvoiceContractClient::new(&env, &contracts[1]).get_admin(),
        Some(admin.clone())
    );
    assert_eq!(
        EscrowContractClient::new(&env, &contracts[2]).get_admin(),
        admin
    );
    assert_eq!(
        PoolContractClient::new(&env, &contracts[3]).get_admin(),
        admin
    );
}

#[test]
fn unrelated_contract_cannot_relay_privileged_pause_calls() {
    let env = Env::default();
    let (_, contracts) = targets(&env);
    let relay = env.register_contract(None, UntrustedRelay);
    for target in &contracts {
        for method in ["pause", "unpause"] {
            rejected(
                &env,
                &relay,
                "forward",
                vec![
                    &env,
                    target.clone().into_val(&env),
                    Symbol::new(&env, method).into_val(&env),
                    Vec::<Val>::new(&env).into_val(&env),
                ],
            );
        }
    }
}

#[test]
fn upgrade_requires_admin_even_through_an_unrelated_contract() {
    let env = Env::default();
    let (_, contracts) = targets(&env);
    let relay = env.register_contract(None, UntrustedRelay);
    let hash = BytesN::from_array(&env, &[7; 32]);
    // Invoice and escrow expose upgrades; registry and pool do not.
    for target in [&contracts[1], &contracts[2]] {
        rejected(
            &env,
            target,
            "upgrade",
            vec![&env, hash.clone().into_val(&env)],
        );
        rejected(
            &env,
            &relay,
            "forward",
            vec![
                &env,
                target.clone().into_val(&env),
                Symbol::new(&env, "upgrade").into_val(&env),
                vec![&env, hash.clone().into_val(&env)].into_val(&env),
            ],
        );
    }
}

#[test]
fn unrelated_account_credentials_do_not_authorize_the_admin() {
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    let env = Env::default();
    let (_, contracts) = targets(&env);
    let attacker = Address::from_string(&String::from_str(
        &env,
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
    ));
    for target in &contracts {
        env.mock_auths(&[MockAuth {
            address: &attacker,
            invoke: &MockAuthInvoke {
                contract: target,
                fn_name: "pause",
                args: vec![&env],
                sub_invokes: &[],
            },
        }]);
        rejected(&env, target, "pause", vec![&env]);
    }
}
