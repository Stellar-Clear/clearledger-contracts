#![cfg(test)]

extern crate std;

use crate::{
    DataKey, Profile, RegistryContract, RegistryContractClient, RegistryError, RevocationReason,
    Role, VerificationStatus, TTL_EXTEND_TO, TTL_THRESHOLD,
};
use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};
use soroban_sdk::{
    map,
    testutils::{
        storage::{Instance as _, Persistent as _},
        Address as _, Events as _, Ledger,
    },
    vec, Address, Env, Error, IntoVal, String, Symbol, TryFromVal, TryIntoVal, Val, Vec,
};

fn setup() -> (Env, RegistryContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);
    (env, client)
}

/// Asserts the last emitted event has the `(name, address, role)` topic shape
/// used by the registry verification events.
fn assert_last_event_role(
    env: &Env,
    client: &RegistryContractClient<'_>,
    name: &str,
    address: &Address,
    role: Role,
) {
    let events = env.events().all();
    let (contract, topics, _data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, client.address);
    assert_eq!(topics.len(), 3);
    assert_eq!(
        Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(env, name)
    );
    assert_eq!(
        Address::try_from_val(env, &topics.get(1).unwrap()).unwrap(),
        address.clone()
    );
    assert_eq!(
        Role::try_from_val(env, &topics.get(2).unwrap()).unwrap(),
        role
    );
}

#[test]
fn test_initialize() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    assert_eq!(client.get_admin(), admin);

    // Assert the contract_initialized event was emitted
    let all_events = env.events().all();
    assert_eq!(all_events.len(), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_get_admin_before_initialize_panics_with_not_initialized() {
    let (_env, client) = setup();
    // get_admin should panic with NotInitialized (#4) instead of NotFound (#3)
    client.get_admin();
}

#[test]
fn test_register_issuer() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme Corp")
        )
    ];
    let result = client.register_issuer(&issuer, &metadata);
    assert!(result);
    let profile = client.get_profile(&issuer);
    assert_eq!(profile.role(), crate::Role::Issuer);
    assert!(!profile.verified());
}

#[test]
fn test_register_buyer() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);
    let metadata = map![&env];
    let result = client.register_buyer(&buyer, &metadata);
    assert!(result);
    let profile = client.get_profile(&buyer);
    assert_eq!(profile.role(), crate::Role::Buyer);
    assert!(!profile.verified());
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_register_issuer_before_initialize_panics() {
    let (env, client) = setup();
    client.register_issuer(&Address::generate(&env), &map![&env]);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_register_buyer_before_initialize_panics() {
    let (env, client) = setup();
    client.register_buyer(&Address::generate(&env), &map![&env]);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_batch_register_issuers_before_initialize_panics() {
    let (env, client) = setup();
    client.batch_register_issuers(&vec![&env]);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_revoke_before_initialize_panics() {
    let (env, client) = setup();
    client.revoke(&Address::generate(&env));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_reinstate_before_initialize_panics() {
    let (env, client) = setup();
    client.reinstate(&Address::generate(&env));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_verify_profile_before_initialize_panics() {
    let (env, client) = setup();
    client.verify_profile(&Address::generate(&env), &true);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_transfer_ownership_before_initialize_panics() {
    let (env, client) = setup();
    client.transfer_ownership(&Address::generate(&env));
}

#[test]
fn test_is_verified_returns_false_for_registered_but_unverified() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    assert!(!client.is_verified(&issuer));
}

#[test]
fn test_is_verified_returns_false_for_unknown() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    assert!(!client.is_verified(&unknown));
}

#[test]
fn test_revoke_sets_verified_false() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    assert!(!client.is_verified(&issuer));
    client.verify_profile(&issuer, &true);
    assert!(client.is_verified(&issuer));
    let result = client.revoke(&issuer);
    assert!(result);
    assert!(!client.is_verified(&issuer));
}

#[test]
fn test_revoke_already_revoked_returns_true_no_reemit() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    assert!(!client.is_verified(&issuer));

    // First revoke — should be a no-op (already unverified) and return true.
    let result = client.revoke(&issuer);
    assert!(result);
    assert!(!client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Pending
    );

    // Second revoke on already-unverified profile — should succeed
    // without panic and without altering state.
    let result2 = client.revoke(&issuer);
    assert!(result2);
    assert!(!client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Pending
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_revoke_unregistered_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    client.revoke(&unknown);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_revoke_wrong_auth_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let metadata = map![&env];
    let profile = Profile::new(Role::Issuer, true, env.ledger().timestamp(), metadata);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::Profile(issuer.clone()), &profile);
        env.storage().persistent().extend_ttl(
            &DataKey::Profile(issuer.clone()),
            TTL_THRESHOLD,
            TTL_EXTEND_TO,
        );
    });

    assert!(client.is_verified(&issuer));
    client.revoke(&issuer);
    assert!(client.is_verified(&issuer));
    assert!(env.events().all().is_empty());
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_re_register_revoked_issuer_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    assert!(!client.is_verified(&issuer));
    client.verify_profile(&issuer, &true);
    assert!(client.is_verified(&issuer));
    client.revoke(&issuer);
    assert!(!client.is_verified(&issuer));
    // A revoked address still has a profile in storage, so re-registering
    // must panic with AlreadyRegistered (#2).
    client.register_issuer(&issuer, &map![&env]);
}

#[test]
fn test_reinstate_revoked_issuer_restores_verification() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    assert!(!client.is_verified(&issuer));
    client.verify_profile(&issuer, &true);
    assert!(client.is_verified(&issuer));

    client.revoke(&issuer);
    assert!(!client.is_verified(&issuer));

    // Reinstate the revoked issuer via admin.verify_profile.
    client.verify_profile(&issuer, &true);
    assert!(client.is_verified(&issuer));
}

// ============== REINSTATE TESTS ==============

#[test]
fn test_reinstate_restores_verified_and_emits_event() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    assert!(!client.is_verified(&issuer));
    client.verify_profile(&issuer, &true);
    assert!(client.is_verified(&issuer));

    client.revoke(&issuer);
    assert!(!client.is_verified(&issuer));

    let result = client.reinstate(&issuer);
    assert!(result);
    assert!(client.is_verified(&issuer));

    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "issuer_registered"), issuer.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (
                    Symbol::new(&env, "profile_verified"),
                    issuer.clone(),
                    Role::Issuer
                )
                    .into_val(&env),
                true.into_val(&env),
            ),
            (
                client.address.clone(),
                (
                    Symbol::new(&env, "address_revoked"),
                    issuer.clone(),
                    Role::Issuer
                )
                    .into_val(&env),
                RevocationReason::Unspecified.into_val(&env),
            ),
            (
                client.address.clone(),
                (
                    Symbol::new(&env, "address_reinstated"),
                    issuer.clone(),
                    Role::Issuer
                )
                    .into_val(&env),
                ().into_val(&env),
            ),
        ]
    );
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_reinstate_wrong_auth_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let metadata = map![&env];
    let profile = Profile::new(Role::Issuer, false, env.ledger().timestamp(), metadata);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::Profile(issuer.clone()), &profile);
        env.storage().persistent().extend_ttl(
            &DataKey::Profile(issuer.clone()),
            TTL_THRESHOLD,
            TTL_EXTEND_TO,
        );
    });

    // The issuer is not the admin and env.mock_all_auths() was not called,
    // so calling reinstate should panic with an auth error.
    client.reinstate(&issuer);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_reinstate_unregistered_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    client.reinstate(&unknown);
}

#[test]
fn test_update_metadata_self_succeeds() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme Corp"),
        )
    ];
    client.register_issuer(&issuer, &metadata);

    let updated_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme LLC"),
        )
    ];
    let result = client.update_metadata(&issuer, &updated_metadata);
    assert!(result);

    let profile = client.get_profile(&issuer);
    assert_eq!(profile.metadata, updated_metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_update_metadata_unregistered_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    let metadata = map![&env];
    client.update_metadata(&unknown, &metadata);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_update_metadata_wrong_auth_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);

    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme Corp"),
        )
    ];
    let profile = Profile::new(Role::Issuer, true, env.ledger().timestamp(), metadata);

    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::Profile(issuer.clone()), &profile);
        env.storage().persistent().extend_ttl(
            &DataKey::Profile(issuer.clone()),
            TTL_THRESHOLD,
            TTL_EXTEND_TO,
        );
    });

    let updated_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Bad Actor"),
        )
    ];
    client.update_metadata(&issuer, &updated_metadata);
}

#[test]
fn test_update_profile_happy_path() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme Corp"),
        )
    ];
    client.register_issuer(&issuer, &metadata);

    let updated_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme LLC"),
        ),
        (
            String::from_str(&env, "tax_id"),
            String::from_str(&env, "12-3456789"),
        ),
    ];
    let result = client.update_profile(&issuer, &updated_metadata);
    assert!(result);

    let profile = client.get_profile(&issuer);
    assert_eq!(profile.metadata, updated_metadata);
    assert_eq!(profile.role(), crate::Role::Issuer);
    assert!(!profile.verified());

    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "issuer_registered"), issuer.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "profile_updated"), issuer.clone()).into_val(&env),
                ().into_val(&env),
            ),
        ]
    );
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_update_profile_wrong_auth_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);

    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme Corp"),
        )
    ];
    let profile = Profile::new(Role::Issuer, true, env.ledger().timestamp(), metadata);

    env.as_contract(&contract_id, || {
        env.storage()
            .persistent()
            .set(&DataKey::Profile(issuer.clone()), &profile);
        env.storage().persistent().extend_ttl(
            &DataKey::Profile(issuer.clone()),
            TTL_THRESHOLD,
            TTL_EXTEND_TO,
        );
    });

    let updated_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Bad Actor"),
        )
    ];
    client.update_profile(&issuer, &updated_metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn test_update_profile_unregistered_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    let metadata = map![&env];
    client.update_profile(&unknown, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_duplicate_registration_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.register_issuer(&issuer, &map![&env]);
}

// ============== CROSS-ROLE REGISTRATION GUARD (Issue #189) ==============

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_register_issuer_then_buyer_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let issuer = Address::generate(&env);
    let metadata = map![&env];

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_issuer",
            args: (issuer.clone(), metadata.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.register_issuer(&issuer, &metadata);

    assert!(!client.is_verified(&issuer));
    assert_eq!(client.get_profile(&issuer).role(), Role::Issuer);

    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "issuer_registered"), issuer.clone()).into_val(&env),
                ().into_val(&env),
            ),
        ]
    );

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &issuer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_buyer",
            args: (issuer.clone(), metadata.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.register_buyer(&issuer, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")]
fn test_register_buyer_then_issuer_panics() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let buyer = Address::generate(&env);
    let metadata = map![&env];

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &buyer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_buyer",
            args: (buyer.clone(), metadata.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.register_buyer(&buyer, &metadata);

    assert!(!client.is_verified(&buyer));
    assert_eq!(client.get_profile(&buyer).role(), Role::Buyer);

    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "buyer_registered"), buyer.clone()).into_val(&env),
                ().into_val(&env),
            ),
        ]
    );

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &buyer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "register_issuer",
            args: (buyer.clone(), metadata.clone()).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.register_issuer(&buyer, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_double_initialize_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    client.initialize(&admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_get_profile_unknown_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    client.get_profile(&unknown);
}

#[test]
fn test_batch_register_issuers_empty_vec() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let entries = Vec::new(&env);
    let skipped = client.batch_register_issuers(&entries);
    assert_eq!(skipped.len(), 0);
}

#[test]
fn test_batch_register_issuers_all_new() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let issuer1 = Address::generate(&env);
    let issuer2 = Address::generate(&env);
    let issuer3 = Address::generate(&env);

    let metadata1 = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Issuer 1")
        )
    ];
    let metadata2 = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Issuer 2")
        )
    ];
    let metadata3 = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Issuer 3")
        )
    ];

    let entries = vec![
        &env,
        (issuer1.clone(), metadata1),
        (issuer2.clone(), metadata2),
        (issuer3.clone(), metadata3),
    ];

    let skipped = client.batch_register_issuers(&entries);
    assert_eq!(skipped.len(), 0);

    assert!(!client.is_verified(&issuer1));
    assert!(!client.is_verified(&issuer2));
    assert!(!client.is_verified(&issuer3));

    assert_eq!(client.get_profile(&issuer1).role(), crate::Role::Issuer);
    assert_eq!(client.get_profile(&issuer2).role(), crate::Role::Issuer);
    assert_eq!(client.get_profile(&issuer3).role(), crate::Role::Issuer);
}

#[test]
fn test_batch_register_issuers_all_duplicate() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let issuer1 = Address::generate(&env);
    let issuer2 = Address::generate(&env);

    client.register_issuer(&issuer1, &map![&env]);
    client.register_issuer(&issuer2, &map![&env]);

    let entries = vec![
        &env,
        (issuer1.clone(), map![&env]),
        (issuer2.clone(), map![&env]),
    ];

    let skipped = client.batch_register_issuers(&entries);
    // Both were already registered — both are reported as skipped.
    assert_eq!(skipped.len(), 2);
    assert!(skipped.contains(&issuer1));
    assert!(skipped.contains(&issuer2));
}

#[test]
fn test_batch_register_issuers_duplicate_within_batch() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let issuer = Address::generate(&env);
    let entries = vec![
        &env,
        (issuer.clone(), map![&env]),
        (issuer.clone(), map![&env]),
    ];

    let skipped = client.batch_register_issuers(&entries);
    // First occurrence registers, the second is reported as skipped.
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped.get(0).unwrap(), issuer);
    assert_eq!(client.get_profile(&issuer).role(), Role::Issuer);
    assert_eq!(client.get_profile_count(&Role::Issuer), 1);
}

#[test]
fn test_batch_register_issuers_mixed() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let issuer1 = Address::generate(&env); // existing
    let issuer2 = Address::generate(&env); // new
    let issuer3 = Address::generate(&env); // new

    client.register_issuer(&issuer1, &map![&env]);

    let entries = vec![
        &env,
        (issuer1.clone(), map![&env]),
        (issuer2.clone(), map![&env]),
        (issuer3.clone(), map![&env]),
    ];

    let skipped = client.batch_register_issuers(&entries);
    // Only issuer1 was already registered.
    assert_eq!(skipped.len(), 1);
    assert!(skipped.contains(&issuer1));

    assert!(!client.is_verified(&issuer1));
    assert!(!client.is_verified(&issuer2));
    assert!(!client.is_verified(&issuer3));
}

// ============== ISSUE #446: PRE-VALIDATION ==============

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_batch_register_issuers_invalid_metadata_rejects_all() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let issuer1 = Address::generate(&env); // would be valid
    let issuer2 = Address::generate(&env); // would be valid
    let issuer3 = Address::generate(&env); // has invalid metadata (oversized)

    let valid_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Good Issuer")
        )
    ];

    let mut oversized_metadata = map![&env];
    for i in 0..21 {
        let key = String::from_str(&env, &std::format!("key_{}", i));
        let value = String::from_str(&env, &std::format!("value_{}", i));
        oversized_metadata.set(key, value);
    }

    let entries = vec![
        &env,
        (issuer1.clone(), valid_metadata.clone()),
        (issuer2.clone(), valid_metadata.clone()),
        (issuer3.clone(), oversized_metadata),
    ];

    // This should panic because issuer3 has invalid metadata.
    // With pre-validation, issuer1 and issuer2 are NOT persisted.
    client.batch_register_issuers(&entries);
}

#[test]
fn test_batch_register_issuers_invalid_metadata_leaves_state_clean() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let issuer1 = Address::generate(&env); // would be valid
    let issuer2 = Address::generate(&env); // has invalid metadata (empty key)

    let valid_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Good Issuer")
        )
    ];

    let bad_metadata = map![
        &env,
        (String::from_str(&env, ""), String::from_str(&env, "value"))
    ];

    let entries = vec![
        &env,
        (issuer1.clone(), valid_metadata),
        (issuer2.clone(), bad_metadata),
    ];

    // Panic expected because issuer2 has invalid metadata.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.batch_register_issuers(&entries);
    }));
    assert!(
        result.is_err(),
        "batch_register_issuers should panic on invalid metadata"
    );

    // Verify that issuer1 was NOT registered — the entire batch was rejected.
    assert!(!client.is_verified(&issuer1));
    assert_eq!(
        client.get_verification_status(&issuer1),
        VerificationStatus::Unregistered
    );
}

// ============== BATCH REGISTER BUYERS (#448) ==============

#[test]
fn test_batch_register_buyers_empty_vec() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let entries = Vec::new(&env);
    let skipped = client.batch_register_buyers(&entries);
    assert_eq!(skipped.len(), 0);
}

#[test]
fn test_batch_register_buyers_all_new() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let buyer1 = Address::generate(&env);
    let buyer2 = Address::generate(&env);
    let buyer3 = Address::generate(&env);

    let metadata1 = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Buyer 1")
        )
    ];
    let metadata2 = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Buyer 2")
        )
    ];
    let metadata3 = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Buyer 3")
        )
    ];

    let entries = vec![
        &env,
        (buyer1.clone(), metadata1),
        (buyer2.clone(), metadata2),
        (buyer3.clone(), metadata3),
    ];

    let skipped = client.batch_register_buyers(&entries);
    assert_eq!(skipped.len(), 0);

    assert!(!client.is_verified(&buyer1));
    assert!(!client.is_verified(&buyer2));
    assert!(!client.is_verified(&buyer3));

    assert_eq!(client.get_profile(&buyer1).role(), crate::Role::Buyer);
    assert_eq!(client.get_profile(&buyer2).role(), crate::Role::Buyer);
    assert_eq!(client.get_profile(&buyer3).role(), crate::Role::Buyer);
}

#[test]
fn test_batch_register_buyers_all_duplicate() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let buyer1 = Address::generate(&env);
    let buyer2 = Address::generate(&env);

    client.register_buyer(&buyer1, &map![&env]);
    client.register_buyer(&buyer2, &map![&env]);

    let entries = vec![
        &env,
        (buyer1.clone(), map![&env]),
        (buyer2.clone(), map![&env]),
    ];

    let skipped = client.batch_register_buyers(&entries);
    // Both were already registered — both are reported as skipped.
    assert_eq!(skipped.len(), 2);
    assert!(skipped.contains(&buyer1));
    assert!(skipped.contains(&buyer2));
}

#[test]
fn test_batch_register_buyers_duplicate_within_batch() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let buyer = Address::generate(&env);
    let entries = vec![
        &env,
        (buyer.clone(), map![&env]),
        (buyer.clone(), map![&env]),
    ];

    let skipped = client.batch_register_buyers(&entries);
    // First occurrence registers, the second is reported as skipped.
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped.get(0).unwrap(), buyer);
    assert_eq!(client.get_profile(&buyer).role(), Role::Buyer);
    assert_eq!(client.get_profile_count(&Role::Buyer), 1);
}

#[test]
fn test_batch_register_buyers_mixed() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let buyer1 = Address::generate(&env); // existing
    let buyer2 = Address::generate(&env); // new
    let buyer3 = Address::generate(&env); // new

    client.register_buyer(&buyer1, &map![&env]);

    let entries = vec![
        &env,
        (buyer1.clone(), map![&env]),
        (buyer2.clone(), map![&env]),
        (buyer3.clone(), map![&env]),
    ];

    let skipped = client.batch_register_buyers(&entries);
    // Only buyer1 was already registered.
    assert_eq!(skipped.len(), 1);
    assert!(skipped.contains(&buyer1));

    assert!(!client.is_verified(&buyer1));
    assert!(!client.is_verified(&buyer2));
    assert!(!client.is_verified(&buyer3));

    assert_eq!(client.get_profile(&buyer2).role(), crate::Role::Buyer);
    assert_eq!(client.get_profile(&buyer3).role(), crate::Role::Buyer);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_batch_register_buyers_invalid_metadata_rejects_all() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let buyer1 = Address::generate(&env);
    let buyer2 = Address::generate(&env);
    let bad_metadata = map![
        &env,
        (String::from_str(&env, ""), String::from_str(&env, "value"))
    ];

    let entries = vec![
        &env,
        (buyer1.clone(), map![&env]),
        (buyer2.clone(), bad_metadata),
    ];

    // Invalid metadata is never skipped: it rejects the whole batch.
    client.batch_register_buyers(&entries);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_batch_register_buyers_exceeds_limit() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let mut entries = Vec::new(&env);
    for _ in 0..51 {
        let address = Address::generate(&env);
        entries.push_back((address, map![&env]));
    }
    client.batch_register_buyers(&entries);
}

#[test]
fn test_verify_profile_updates_status() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);

    assert!(!client.is_verified(&issuer));

    // Verify
    let result = client.verify_profile(&issuer, &true);
    assert!(result);
    assert!(client.is_verified(&issuer));

    // Revoke
    client.revoke(&issuer);
    assert!(!client.is_verified(&issuer));

    // Re-verify
    let result = client.verify_profile(&issuer, &true);
    assert!(result);
    assert!(client.is_verified(&issuer));

    // Un-verify again
    let result2 = client.verify_profile(&issuer, &false);
    assert!(result2);
    assert!(!client.is_verified(&issuer));
}

#[test]
fn test_verify_profile_event_includes_issuer_role() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);

    assert_last_event_role(&env, &client, "profile_verified", &issuer, Role::Issuer);

    // The data payload remains the bool verification status.
    let events = env.events().all();
    let (_, _, data) = events.get(events.len() - 1).unwrap();
    assert!(bool::try_from_val(&env, &data).unwrap());
}

#[test]
fn test_verify_profile_event_includes_buyer_role() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);
    client.register_buyer(&buyer, &map![&env]);
    client.verify_profile(&buyer, &true);

    assert_last_event_role(&env, &client, "profile_verified", &buyer, Role::Buyer);

    let events = env.events().all();
    let (_, _, data) = events.get(events.len() - 1).unwrap();
    assert!(bool::try_from_val(&env, &data).unwrap());
}

#[test]
fn test_revoke_and_reinstate_events_include_buyer_role() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);
    client.register_buyer(&buyer, &map![&env]);
    client.verify_profile(&buyer, &true);

    client.revoke(&buyer);
    assert_last_event_role(&env, &client, "address_revoked", &buyer, Role::Buyer);

    client.reinstate(&buyer);
    assert_last_event_role(&env, &client, "address_reinstated", &buyer, Role::Buyer);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_verify_profile_unknown_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    client.verify_profile(&unknown, &true);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_verify_profile_wrong_auth_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);

    // Clear all mocked auths — a non-admin caller should be rejected
    env.set_auths(&[]);
    client.verify_profile(&issuer, &true);
}

#[test]
fn test_get_verification_status_unregistered() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let unknown = Address::generate(&env);
    assert_eq!(
        client.get_verification_status(&unknown),
        VerificationStatus::Unregistered
    );
}

#[test]
fn test_get_verification_status_verified() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Verified
    );
}

#[test]
fn test_get_verification_status_revoked() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    client.revoke(&issuer);
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Revoked
    );
}

#[test]
fn test_get_verification_status_distinguishes_pending_from_unregistered() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let never_registered = Address::generate(&env);
    let pending = Address::generate(&env);

    client.register_issuer(&pending, &map![&env]);

    // is_verified returns false for both — indistinguishable
    assert!(!client.is_verified(&never_registered));
    assert!(!client.is_verified(&pending));

    // get_verification_status tells them apart
    assert_eq!(
        client.get_verification_status(&never_registered),
        VerificationStatus::Unregistered
    );
    assert_eq!(
        client.get_verification_status(&pending),
        VerificationStatus::Pending
    );
}

#[test]
fn test_get_verification_status_revoked_distinct_from_pending() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let pending = Address::generate(&env);
    let revoked = Address::generate(&env);

    client.register_issuer(&pending, &map![&env]);
    client.register_issuer(&revoked, &map![&env]);
    client.verify_profile(&revoked, &true);
    client.revoke(&revoked);

    assert_eq!(
        client.get_verification_status(&pending),
        VerificationStatus::Pending
    );
    assert_eq!(
        client.get_verification_status(&revoked),
        VerificationStatus::Revoked
    );
}

#[test]
fn test_get_verification_status_re_verified_returns_verified() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    client.revoke(&issuer);
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Revoked
    );
    client.verify_profile(&issuer, &true);
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Verified
    );
}

// ============== ISSUE #173: TRANSFER ADMIN ==============

#[test]
fn test_transfer_admin_changes_admin() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.initialize(&admin);
    client.transfer_admin(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
fn test_transfer_admin_bypasses_transfer_ownership_dual_auth() {
    let env = Env::default();
    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "initialize",
            args: (admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.initialize(&admin);

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "transfer_ownership",
            args: (new_admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    assert!(client.try_transfer_ownership(&new_admin).is_err());

    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "transfer_admin",
            args: (new_admin.clone(),).into_val(&env),
            sub_invokes: &[],
        },
    }]);
    client.transfer_admin(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_transfer_admin_by_non_admin_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.initialize(&admin);
    env.set_auths(&[]);
    client.transfer_admin(&new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_transfer_admin_before_initialize_panics() {
    let (env, client) = setup();
    let new_admin = Address::generate(&env);
    client.transfer_admin(&new_admin);
}

#[test]
fn test_transfer_admin_emits_event() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.initialize(&admin);
    client.transfer_admin(&new_admin);
    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "admin_transferred"), admin.clone()).into_val(&env),
                new_admin.clone().into_val(&env),
            ),
        ]
    );
}

// ============== ISSUE #61: TRANSFER OWNERSHIP ==============

#[test]
fn test_registry_transfer_ownership_changes_admin() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.initialize(&admin);
    client.transfer_ownership(&new_admin);
    assert_eq!(client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_registry_transfer_ownership_requires_both_auths() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    client.initialize(&admin);
    env.set_auths(&[]);
    client.transfer_ownership(&new_admin);
}

// ============== PROPERTY-BASED INVARIANT TESTS ==============

fn build_metadata(
    env: &Env,
    entries: &std::vec::Vec<(std::string::String, std::string::String)>,
) -> soroban_sdk::Map<String, String> {
    let mut metadata = map![env];
    for (k, v) in entries {
        metadata.set(
            String::from_str(env, k.as_str()),
            String::from_str(env, v.as_str()),
        );
    }
    metadata
}

fn metadata_entries(
) -> impl Strategy<Value = std::vec::Vec<(std::string::String, std::string::String)>> {
    prop::collection::vec(("[a-zA-Z_][a-zA-Z0-9_]{0,9}", "[a-zA-Z0-9_]{1,20}"), 0..=5)
}

#[test]
fn prop_is_verified_always_consistent_with_get_verification_status_after_register() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(any::<bool>(), metadata_entries()),
            |(is_buyer, entries)| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);
                let address = Address::generate(&env);
                let metadata = build_metadata(&env, &entries);
                if is_buyer {
                    client.register_buyer(&address, &metadata);
                } else {
                    client.register_issuer(&address, &metadata);
                }
                let verified = client.is_verified(&address);
                let status = client.get_verification_status(&address);
                prop_assert!(!verified);
                prop_assert_eq!(status, VerificationStatus::Pending);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn prop_revoke_always_sets_is_verified_false_and_status_revoked() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(any::<bool>(), metadata_entries(), 1usize..=3),
            |(is_buyer, entries, verify_count)| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);
                let address = Address::generate(&env);
                let metadata = build_metadata(&env, &entries);
                if is_buyer {
                    client.register_buyer(&address, &metadata);
                } else {
                    client.register_issuer(&address, &metadata);
                }
                for _ in 0..verify_count {
                    client.verify_profile(&address, &true);
                }
                client.revoke(&address);
                prop_assert!(!client.is_verified(&address));
                prop_assert_eq!(
                    client.get_verification_status(&address),
                    VerificationStatus::Revoked
                );
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn prop_unregistered_address_never_verified() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &prop::collection::vec((any::<bool>(), metadata_entries()), 1..=5),
            |other_profiles| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);
                for (is_buyer, entries) in &other_profiles {
                    let addr = Address::generate(&env);
                    let metadata = build_metadata(&env, entries);
                    if *is_buyer {
                        client.register_buyer(&addr, &metadata);
                    } else {
                        client.register_issuer(&addr, &metadata);
                    }
                }
                let unknown = Address::generate(&env);
                prop_assert!(!client.is_verified(&unknown));
                prop_assert_eq!(
                    client.get_verification_status(&unknown),
                    VerificationStatus::Unregistered
                );
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn prop_re_verify_after_revoke_restores_verified_state() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(any::<bool>(), metadata_entries(), 1usize..=3),
            |(is_buyer, entries, cycles)| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);
                let address = Address::generate(&env);
                let metadata = build_metadata(&env, &entries);
                if is_buyer {
                    client.register_buyer(&address, &metadata);
                } else {
                    client.register_issuer(&address, &metadata);
                }
                client.verify_profile(&address, &true);
                for _ in 0..cycles {
                    client.revoke(&address);
                    prop_assert_eq!(
                        client.get_verification_status(&address),
                        VerificationStatus::Revoked
                    );
                    client.verify_profile(&address, &true);
                    prop_assert!(client.is_verified(&address));
                    prop_assert_eq!(
                        client.get_verification_status(&address),
                        VerificationStatus::Verified
                    );
                }
                Ok(())
            },
        )
        .unwrap();
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_batch_register_issuers_exceeds_limit() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let mut entries = Vec::new(&env);
    for _ in 0..51 {
        let address = Address::generate(&env);
        entries.push_back((address, map![&env]));
    }
    client.batch_register_issuers(&entries);
}

#[test]
fn test_profile_packing_correctness() {
    let env = Env::default();
    let _addr = Address::generate(&env);
    let metadata = map![&env];

    // Issuer, verified = true
    let p1 = Profile::new(Role::Issuer, true, 100, metadata.clone());
    assert_eq!(p1.role(), Role::Issuer);
    assert!(p1.verified());

    // Issuer, verified = false
    let p2 = Profile::new(Role::Issuer, false, 100, metadata.clone());
    assert_eq!(p2.role(), Role::Issuer);
    assert!(!p2.verified());

    // Buyer, verified = true
    let p3 = Profile::new(Role::Buyer, true, 100, metadata.clone());
    assert_eq!(p3.role(), Role::Buyer);
    assert!(p3.verified());

    // Buyer, verified = false
    let p4 = Profile::new(Role::Buyer, false, 100, metadata.clone());
    assert_eq!(p4.role(), Role::Buyer);
    assert!(!p4.verified());
}

// ============== METADATA EDGE CASE TESTS (#190) ==============

#[test]
fn test_metadata_empty_map_accepted() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let metadata = map![&env];
    let result = client.register_issuer(&issuer, &metadata);
    assert!(result);
    let profile = client.get_profile(&issuer);
    assert_eq!(profile.metadata.len(), 0);
}

#[test]
fn test_metadata_max_size_map_accepted() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let mut metadata = map![&env];
    for i in 0..20 {
        let key = String::from_str(&env, &std::format!("key_{}", i));
        let value = String::from_str(&env, &std::format!("value_{}", i));
        metadata.set(key, value);
    }
    assert_eq!(metadata.len(), 20);
    let result = client.register_issuer(&issuer, &metadata);
    assert!(result);
    let profile = client.get_profile(&issuer);
    assert_eq!(profile.metadata.len(), 20);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_metadata_oversize_map_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let mut metadata = map![&env];
    for i in 0..21 {
        let key = String::from_str(&env, &std::format!("key_{}", i));
        let value = String::from_str(&env, &std::format!("value_{}", i));
        metadata.set(key, value);
    }
    client.register_issuer(&issuer, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_metadata_oversize_map_via_update_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);

    let mut oversized = map![&env];
    for i in 0..21 {
        let key = String::from_str(&env, &std::format!("key_{}", i));
        let value = String::from_str(&env, &std::format!("value_{}", i));
        oversized.set(key, value);
    }
    client.update_metadata(&issuer, &oversized);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_metadata_empty_key_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (String::from_str(&env, ""), String::from_str(&env, "value"),)
    ];
    client.register_issuer(&issuer, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_metadata_empty_value_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let metadata = map![
        &env,
        (String::from_str(&env, "key"), String::from_str(&env, ""),)
    ];
    client.register_issuer(&issuer, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_metadata_empty_key_via_update_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);

    let bad_metadata = map![
        &env,
        (String::from_str(&env, ""), String::from_str(&env, "value"),)
    ];
    client.update_metadata(&issuer, &bad_metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_metadata_empty_value_via_update_panics() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);

    let bad_metadata = map![
        &env,
        (String::from_str(&env, "key"), String::from_str(&env, ""),)
    ];
    client.update_metadata(&issuer, &bad_metadata);
}

#[test]
fn test_metadata_buyer_empty_map_accepted() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);
    let result = client.register_buyer(&buyer, &map![&env]);
    assert!(result);
    let profile = client.get_profile(&buyer);
    assert_eq!(profile.metadata.len(), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_register_buyer_rejects_oversized_metadata_key() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);
    let long_key = "k".repeat((crate::MAX_METADATA_KEY_LEN + 1) as usize);
    let metadata = map![
        &env,
        (
            String::from_str(&env, &long_key),
            String::from_str(&env, "value")
        )
    ];
    client.register_buyer(&buyer, &metadata);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn test_register_issuer_rejects_oversized_metadata_value() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    let long_value = "v".repeat((crate::MAX_METADATA_VALUE_LEN + 1) as usize);
    let metadata = map![
        &env,
        (
            String::from_str(&env, "key"),
            String::from_str(&env, &long_value)
        )
    ];
    client.register_issuer(&issuer, &metadata);
}

#[test]
fn test_register_metadata_at_limits_accepted() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);
    let key_prefix = "k".repeat((crate::MAX_METADATA_KEY_LEN - 2) as usize);
    let value = "v".repeat(crate::MAX_METADATA_VALUE_LEN as usize);
    let mut metadata = map![&env];
    for i in 0..crate::MAX_METADATA_SIZE {
        let key = std::format!("{key_prefix}{i:02}");
        metadata.set(String::from_str(&env, &key), String::from_str(&env, &value));
    }
    assert_eq!(metadata.len(), crate::MAX_METADATA_SIZE);
    assert!(client.register_buyer(&buyer, &metadata));
    let profile = client.get_profile(&buyer);
    assert_eq!(profile.metadata.len(), crate::MAX_METADATA_SIZE);
}

// ============== EVENT-EMISSION TESTS (#188) ==============

#[test]
fn test_register_issuer_emits_event() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);

    client.register_issuer(&issuer, &map![&env]);

    // State after: the issuer is registered but not yet verified.
    assert!(!client.is_verified(&issuer));

    // Registration emits exactly one `issuer_registered` event carrying the
    // issuer address in the topics and an empty data payload.
    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "issuer_registered"), issuer.clone()).into_val(&env),
                ().into_val(&env),
            ),
        ]
    );
}

#[test]
fn test_register_buyer_emits_event() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let buyer = Address::generate(&env);

    client.register_buyer(&buyer, &map![&env]);

    // State after: the buyer is registered but not yet verified.
    assert!(!client.is_verified(&buyer));

    // Registration emits exactly one `buyer_registered` event carrying the
    // buyer address in the topics and an empty data payload.
    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "buyer_registered"), buyer.clone()).into_val(&env),
                ().into_val(&env),
            ),
        ]
    );
}

#[test]
fn test_revoke_emits_event() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);

    client.revoke(&issuer);

    // State after: verification has been revoked.
    assert!(!client.is_verified(&issuer));

    // The full event stream: registration, admin verification, then revoke.
    assert_eq!(
        env.events().all(),
        vec![
            &env,
            (
                client.address.clone(),
                (Symbol::new(&env, "contract_initialized"), admin.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (Symbol::new(&env, "issuer_registered"), issuer.clone()).into_val(&env),
                ().into_val(&env),
            ),
            (
                client.address.clone(),
                (
                    Symbol::new(&env, "profile_verified"),
                    issuer.clone(),
                    Role::Issuer
                )
                    .into_val(&env),
                true.into_val(&env),
            ),
            (
                client.address.clone(),
                (
                    Symbol::new(&env, "address_revoked"),
                    issuer.clone(),
                    Role::Issuer
                )
                    .into_val(&env),
                RevocationReason::Unspecified.into_val(&env),
            ),
        ]
    );
}

#[test]
fn test_revoke_with_reason_emits_explicit_reason_code() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);

    client.revoke_with_reason(&issuer, &RevocationReason::Fraud);

    assert!(!client.is_verified(&issuer));
    let events = env.events().all();
    let event = events.last().unwrap();
    assert_eq!(
        event.1,
        (
            Symbol::new(&env, "address_revoked"),
            issuer.clone(),
            Role::Issuer
        )
            .into_val(&env)
    );
    assert_eq!(event.2, RevocationReason::Fraud.into_val(&env));
    assert_eq!(event.2, 2u32.into_val(&env));
}

#[test]
fn test_revocation_reason_discriminants_are_stable() {
    let env = Env::default();
    let cases = [
        (RevocationReason::Unspecified, 0u32),
        (RevocationReason::Compliance, 1u32),
        (RevocationReason::Fraud, 2u32),
        (RevocationReason::Inactivity, 3u32),
        (RevocationReason::Other, 4u32),
    ];
    for (reason, code) in cases {
        assert_eq!(reason.into_val(&env), code.into_val(&env));
    }
}

// ============== ISSUE #179: TTL EXTENSION ON READ ==============

#[test]
fn test_get_profile_extends_ttl() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    // New profiles start unverified (#130) — verify so the read below
    // exercises a fully-registered profile.
    client.verify_profile(&issuer, &true);

    let contract_id = client.address.clone();
    let key = DataKey::Profile(issuer.clone());

    // Record the initial remaining TTL, then advance the ledger so the
    // remaining TTL drops below the write-path threshold.
    let ttl_before_drain: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));
    // Advance to leave ~50 ledgers remaining.
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + ttl_before_drain - 50);

    let ttl_before_read: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));
    assert!(
        ttl_before_read < TTL_THRESHOLD,
        "TTL should be below threshold before read, got {ttl_before_read}"
    );

    // Read the profile — this should extend the entry's TTL.
    let profile = client.get_profile(&issuer);
    assert!(profile.verified());

    let ttl_after_read: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));

    assert!(
        ttl_after_read > ttl_before_read,
        "get_profile should extend TTL: before={ttl_before_read}, after={ttl_after_read}"
    );
    assert!(
        ttl_after_read >= 1_999_000,
        "TTL should be extended close to EXTEND_TO (2_000_000), got {ttl_after_read}"
    );

    // A profile remains readable after a long supported-activity interval,
    // and the read renews it before the persistent entry can expire.
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + ttl_after_read - 50);
    assert!(client.get_profile(&issuer).verified());
    let ttl_after_activity: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));
    assert!(
        ttl_after_activity >= 1_999_000,
        "active profile TTL should be renewed before expiry, got {ttl_after_activity}"
    );
}

#[test]
fn test_is_verified_extends_ttl() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    // New profiles start unverified (#130) — verify so is_verified is true.
    client.verify_profile(&issuer, &true);

    let contract_id = client.address.clone();
    let key = DataKey::Profile(issuer.clone());

    // Drain TTL below the threshold.
    let ttl_before_drain: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + ttl_before_drain - 50);

    let ttl_before_read: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));
    assert!(
        ttl_before_read < TTL_THRESHOLD,
        "TTL should be below threshold before read, got {ttl_before_read}"
    );

    // Call is_verified — this should extend the entry's TTL.
    assert!(client.is_verified(&issuer));

    let ttl_after_read: u32 =
        env.as_contract(&contract_id, || env.storage().persistent().get_ttl(&key));

    assert!(
        ttl_after_read > ttl_before_read,
        "is_verified should extend TTL: before={ttl_before_read}, after={ttl_after_read}"
    );
    assert!(
        ttl_after_read >= 1_999_000,
        "TTL should be extended close to EXTEND_TO (2_000_000), got {ttl_after_read}"
    );
}

#[test]
fn test_is_verified_does_not_extend_ttl_for_unknown() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    // Calling is_verified on an unknown address must return false and must
    // not panic (no TTL extension attempted for a non-existent entry).
    let unknown = Address::generate(&env);
    assert!(!client.is_verified(&unknown));

    // Also verify the same function still works for a registered issuer.
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    // New profiles start unverified (#130) — verify before asserting.
    client.verify_profile(&issuer, &true);
    assert!(client.is_verified(&issuer));
}

#[test]
fn test_get_admin_extends_instance_ttl() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let contract_id = client.address.clone();

    // Drain instance TTL below the threshold.
    let ttl_before_drain: u32 =
        env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + ttl_before_drain - 50);

    let ttl_before_read: u32 = env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    assert!(
        ttl_before_read < TTL_THRESHOLD,
        "Instance TTL should be below threshold before read, got {ttl_before_read}"
    );

    // Read the admin — this should extend the instance TTL.
    assert_eq!(client.get_admin(), admin);

    let ttl_after_read: u32 = env.as_contract(&contract_id, || env.storage().instance().get_ttl());

    assert!(
        ttl_after_read > ttl_before_read,
        "get_admin should extend instance TTL: before={ttl_before_read}, after={ttl_after_read}"
    );
    assert!(
        ttl_after_read >= 1_999_000,
        "Instance TTL should be extended close to EXTEND_TO (2_000_000), got {ttl_after_read}"
    );
}

// ============== ISSUE #447: update_metadata extends instance TTL ==============

#[test]
fn test_update_metadata_extends_instance_ttl() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(
        &issuer,
        &map![
            &env,
            (
                String::from_str(&env, "name"),
                String::from_str(&env, "Acme Corp")
            )
        ],
    );

    let contract_id = client.address.clone();

    // Drain instance TTL below the threshold (100).
    let ttl_before_drain: u32 =
        env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + ttl_before_drain - 50);

    let ttl_before_update: u32 =
        env.as_contract(&contract_id, || env.storage().instance().get_ttl());
    assert!(
        ttl_before_update < TTL_THRESHOLD,
        "Instance TTL should be below threshold before update, got {ttl_before_update}"
    );

    // Update metadata — this should extend the instance TTL.
    let updated_metadata = map![
        &env,
        (
            String::from_str(&env, "name"),
            String::from_str(&env, "Acme LLC")
        )
    ];
    let result = client.update_metadata(&issuer, &updated_metadata);
    assert!(result);

    let ttl_after_update: u32 =
        env.as_contract(&contract_id, || env.storage().instance().get_ttl());

    assert!(
        ttl_after_update > ttl_before_update,
        "update_metadata should extend instance TTL: before={ttl_before_update}, after={ttl_after_update}"
    );
    assert!(
        ttl_after_update >= 1_999_000,
        "Instance TTL should be extended close to EXTEND_TO (2_000_000), got {ttl_after_update}"
    );
}

// ============== PROPERTY-BASED TESTS: METADATA BOUNDARY (Issue #449) ==============

/// Strategy that generates metadata entries with valid key/value patterns.
fn valid_metadata_entries(
) -> impl Strategy<Value = std::vec::Vec<(std::string::String, std::string::String)>> {
    prop::collection::vec(("[a-zA-Z_][a-zA-Z0-9_]{0,9}", "[a-zA-Z0-9_]{1,20}"), 0..=5)
}

/// Strategy that generates metadata entries that may contain empty keys or
/// values (approximately 30% chance per entry).
fn metadata_entries_with_empty_field(
) -> impl Strategy<Value = std::vec::Vec<(std::string::String, std::string::String)>> {
    prop::collection::vec(
        (
            prop_oneof![
                Just(std::string::String::new()),
                "[a-zA-Z][a-zA-Z0-9_]{0,9}"
            ],
            prop_oneof![Just(std::string::String::new()), "[a-zA-Z0-9_]{1,20}"],
        ),
        1..=5,
    )
}

/// Proptest: metadata maps with sizes in 0..=20 are accepted, sizes in
/// 21..=25 are rejected with `InvalidMetadata`.
#[test]
fn prop_metadata_size_boundary_accepted_and_rejected() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(0u32..=25, valid_metadata_entries()),
            |(target_size, entries)| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);
                let address = Address::generate(&env);

                // Build a metadata map with exactly `target_size` entries.
                let mut metadata = map![&env];
                for i in 0..target_size {
                    if let Some((k, v)) = entries.get(i as usize) {
                        metadata.set(
                            String::from_str(&env, k.as_str()),
                            String::from_str(&env, v.as_str()),
                        );
                    } else {
                        // Generate a fallback key/value pair when the
                        // strategy provided fewer entries than target_size.
                        metadata.set(
                            String::from_str(&env, &std::format!("key_{i}")),
                            String::from_str(&env, &std::format!("value_{i}")),
                        );
                    }
                }

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    client.register_issuer(&address, &metadata);
                }));

                if target_size <= 20 {
                    prop_assert!(
                        result.is_ok(),
                        "metadata with {} entries should be accepted, but panicked",
                        target_size
                    );
                    // Verify the profile was registered successfully.
                    prop_assert_eq!(client.get_profile(&address).role(), Role::Issuer);
                } else {
                    prop_assert!(
                        result.is_err(),
                        "metadata with {} entries should be rejected, but succeeded",
                        target_size
                    );
                }
                Ok(())
            },
        )
        .unwrap();
}

/// Proptest: metadata containing at least one empty key or empty value is
/// always rejected with `InvalidMetadata`, regardless of total size.
#[test]
fn prop_metadata_empty_key_or_value_always_rejected() {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(&metadata_entries_with_empty_field(), |entries| {
            let (env, client) = setup();
            let admin = Address::generate(&env);
            client.initialize(&admin);
            let address = Address::generate(&env);

            let metadata = build_metadata(&env, &entries);

            let has_empty = entries.iter().any(|(k, v)| k.is_empty() || v.is_empty());

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                client.register_issuer(&address, &metadata);
            }));

            if has_empty {
                prop_assert!(
                    result.is_err(),
                    "metadata with empty key/value should be rejected, but succeeded"
                );
            }
            // When all keys/values are non-empty the registration must
            // succeed (the strategy can generate up to 5 entries, well
            // below MAX_METADATA_SIZE).
            if !has_empty {
                prop_assert!(
                    result.is_ok(),
                    "metadata with all non-empty fields should be accepted, but panicked"
                );
            }
            Ok(())
        })
        .unwrap();
}

// ============== ISSUE #851: BATCH REGISTRATION CONSERVATION PROPTESTS ==============

type BatchEntryPlan = (
    usize,
    std::vec::Vec<usize>,
    std::collections::BTreeSet<usize>,
    std::vec::Vec<std::vec::Vec<(std::string::String, std::string::String)>>,
);

/// Plan for a batch registration proptest: a pool of `unique_count` addresses,
/// a vector of indices into that pool forming the batch (with possible
/// duplicates), a subset of pool indices to pre-register, and a valid
/// metadata map for every batch entry.
fn batch_entry_plan() -> impl Strategy<Value = BatchEntryPlan> {
    (1usize..=50).prop_flat_map(|unique_count| {
        prop::collection::vec(0..unique_count, 1..=50).prop_flat_map(move |entry_indices| {
            let entry_count = entry_indices.len();
            (
                Just(unique_count),
                Just(entry_indices),
                prop::collection::btree_set(0..unique_count, 0..=unique_count),
                prop::collection::vec(valid_metadata_entries(), entry_count..=entry_count),
            )
        })
    })
}

/// Asserts that the most recent `batch_registered` event carries exactly
/// `expected_registered` and `expected_skipped` in its data payload.
fn assert_batch_event_counts(env: &Env, expected_registered: u32, expected_skipped: u32) {
    let all_events = env.events().all();
    let mut found = false;
    for i in (0..all_events.len()).rev() {
        let event = all_events.get(i).expect("event index in range");
        let topics: Vec<Val> = match event.1.clone().try_into_val(env) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if topics.len() != 1 {
            continue;
        }
        let topic_symbol: Symbol = match topics.get(0).unwrap().try_into_val(env) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if topic_symbol == Symbol::new(env, "batch_registered") {
            let (registered, skipped): (u32, u32) = event
                .2
                .clone()
                .try_into_val(env)
                .expect("batch event data should be (u32, u32)");
            assert_eq!(registered, expected_registered);
            assert_eq!(skipped, expected_skipped);
            found = true;
            break;
        }
    }
    assert!(found, "batch_registered event not found");
}

/// Runs the batch-registration conservation proptest for either issuers
/// (`is_buyer = false`) or buyers (`is_buyer = true`).
fn run_batch_register_prop_test(is_buyer: bool) {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(20));
    runner
        .run(
            &batch_entry_plan(),
            |(unique_count, entry_indices, preregistered, metadata_per_entry)| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);

                // Build the deterministic address pool.
                let addresses: std::vec::Vec<Address> =
                    (0..unique_count).map(|_| Address::generate(&env)).collect();

                // Pre-register the chosen addresses and snapshot their profiles
                // so we can prove skipped entries are unchanged.
                let mut preregistered_profiles = std::vec::Vec::new();
                let preregister_metadata = build_metadata(&env, &metadata_per_entry[0]);
                for idx in &preregistered {
                    let addr = addresses[*idx].clone();
                    if is_buyer {
                        client.register_buyer(&addr, &preregister_metadata);
                    } else {
                        client.register_issuer(&addr, &preregister_metadata);
                    }
                    preregistered_profiles.push((addr, client.get_profile(&addresses[*idx])));
                }

                // Build the batch entries from the generated plan.
                let mut entries = Vec::new(&env);
                for (i, idx) in entry_indices.iter().enumerate() {
                    let addr = addresses[*idx].clone();
                    let metadata = build_metadata(&env, &metadata_per_entry[i]);
                    entries.push_back((addr, metadata));
                }

                // Invoke the batch function under test.
                let skipped = if is_buyer {
                    client.batch_register_buyers(&entries)
                } else {
                    client.batch_register_issuers(&entries)
                };

                // Recompute the expected outcome from the plan.
                let mut seen = std::collections::HashSet::new();
                let mut expected_registered: u32 = 0;
                let mut expected_skipped = Vec::new(&env);
                for idx in &entry_indices {
                    if preregistered.contains(idx) || seen.contains(idx) {
                        expected_skipped.push_back(addresses[*idx].clone());
                    } else {
                        seen.insert(*idx);
                        expected_registered += 1;
                    }
                }

                // Conservation: every entry is either registered or skipped.
                prop_assert_eq!(
                    expected_registered + expected_skipped.len(),
                    entry_indices.len() as u32
                );
                prop_assert_eq!(skipped, expected_skipped.clone());

                // Newly registered addresses are Pending with the right role;
                // pre-registered skipped addresses are untouched.
                for (i, idx) in entry_indices.iter().enumerate() {
                    let addr = addresses[*idx].clone();
                    let already_registered =
                        preregistered.contains(idx) || entry_indices[..i].contains(idx);
                    if already_registered {
                        if let Some((_, before)) =
                            preregistered_profiles.iter().find(|(a, _)| a == &addr)
                        {
                            let after = client.get_profile(&addr);
                            prop_assert_eq!(before.packed_flags, after.packed_flags);
                            prop_assert_eq!(before.registered_at, after.registered_at);
                            prop_assert_eq!(before.metadata.clone(), after.metadata.clone());
                        }
                    } else {
                        let profile = client.get_profile(&addr);
                        prop_assert_eq!(
                            profile.role(),
                            if is_buyer { Role::Buyer } else { Role::Issuer }
                        );
                        prop_assert_eq!(
                            client.get_verification_status(&addr),
                            VerificationStatus::Pending
                        );
                        if is_buyer {
                            let expected_metadata = build_metadata(&env, &metadata_per_entry[i]);
                            prop_assert_eq!(profile.metadata, expected_metadata);
                        } else {
                            prop_assert_eq!(profile.metadata.len(), 0);
                        }
                    }
                }

                // The emitted batch event carries the same counts.
                assert_batch_event_counts(&env, expected_registered, expected_skipped.len());

                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn prop_batch_register_issuers_conserves_entries() {
    run_batch_register_prop_test(false);
}

#[test]
fn prop_batch_register_buyers_conserves_entries() {
    run_batch_register_prop_test(true);
}

#[test]
fn test_batch_register_issuers_exceeds_limit_leaves_state_clean() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut entries = Vec::new(&env);
    for _ in 0..51 {
        let address = Address::generate(&env);
        entries.push_back((address, map![&env]));
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.batch_register_issuers(&entries);
    }));
    assert!(result.is_err(), "batch over 50 entries should panic");

    for (address, _) in entries.iter() {
        assert_eq!(
            client.get_verification_status(&address),
            VerificationStatus::Unregistered
        );
    }
}

#[test]
fn test_batch_register_buyers_exceeds_limit_leaves_state_clean() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut entries = Vec::new(&env);
    for _ in 0..51 {
        let address = Address::generate(&env);
        entries.push_back((address, map![&env]));
    }

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.batch_register_buyers(&entries);
    }));
    assert!(result.is_err(), "batch over 50 entries should panic");

    for (address, _) in entries.iter() {
        assert_eq!(
            client.get_verification_status(&address),
            VerificationStatus::Unregistered
        );
    }
}
// ============== ISSUE #715: EMERGENCY PAUSE / UNPAUSE ==============

#[test]
fn test_pause_blocks_state_changes_and_unpause_restores_them() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    // Live: registration works. New profiles start `Pending` until an admin
    // verifies them, so `is_verified` is false and the status is `Pending`.
    let issuer = Address::generate(&env);
    assert!(client.register_issuer(&issuer, &map![&env]));
    let flags_before_pause = client.get_profile(&issuer).packed_flags;

    client.pause();

    // State-changing entry points are rejected while paused...
    let new_issuer = Address::generate(&env);
    assert!(client
        .try_register_issuer(&new_issuer, &map![&env])
        .is_err());
    assert!(client.try_update_metadata(&issuer, &map![&env]).is_err());
    assert!(client.try_revoke(&issuer).is_err());
    assert!(client.try_transfer_admin(&Address::generate(&env)).is_err());
    assert!(client
        .try_batch_register_issuers(&vec![&env, (Address::generate(&env), map![&env])])
        .is_err());

    // ...while read-only views keep working.
    assert_eq!(client.get_admin(), admin);
    assert!(!client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Pending
    );
    // A view that deserializes a full record stays live too, and the rejected
    // writes above left the record untouched.
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before_pause);

    client.unpause();

    // Unpaused: state changes flow again.
    assert!(client.register_issuer(&new_issuer, &map![&env]));
    assert!(client.update_metadata(&issuer, &map![&env]));
}

/// Pins the exact breaker error: `PauseError::ContractPaused` from the shared
/// crate surfaces as `Error(Contract, #1)`. `RegistryError::AlreadyInitialized`
/// is the only other #1 and is unreachable from `register_issuer`.
#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_register_issuer_reverts_while_paused_with_contract_paused_error() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    client.pause();
    client.register_issuer(&Address::generate(&env), &map![&env]);
}

/// Negative auth: only a non-admin signed `pause`, so the stored admin's
/// `require_auth()` must reject it — a non-admin cannot engage the breaker.
#[test]
fn test_pause_requires_admin_authorization() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
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
/// paused registry cannot be resumed by anyone but the admin.
#[test]
fn test_unpause_requires_admin_authorization() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
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
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

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

/// Admin ownership transfer is a state change, so the breaker gates it too:
/// while paused it reverts and the admin is unchanged.
#[test]
fn test_transfer_ownership_blocked_while_paused() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let new_admin = Address::generate(&env);

    client.pause();
    assert!(client.try_transfer_ownership(&new_admin).is_err());
    assert_eq!(client.get_admin(), admin);
}

/// The breaker is storage-local: pausing one registry must not affect a second,
/// independently deployed instance.
#[test]
fn test_pause_is_scoped_to_one_instance() {
    let env = Env::default();
    env.mock_all_auths();
    let id_a = env.register_contract(None, RegistryContract);
    let id_b = env.register_contract(None, RegistryContract);
    let admin = Address::generate(&env);
    let a = RegistryContractClient::new(&env, &id_a);
    let b = RegistryContractClient::new(&env, &id_b);
    a.initialize(&admin);
    b.initialize(&admin);

    a.pause();

    let issuer = Address::generate(&env);
    assert!(a.try_register_issuer(&issuer, &map![&env]).is_err());
    assert!(b.register_issuer(&issuer, &map![&env]));
}

const ENUMERATION_POOL_SIZE: usize = 10;
const ENUMERATION_MAX_OPS: usize = 24;

const OP_REGISTER: u8 = 0;
const OP_DUPLICATE: u8 = 1;
const OP_BATCH: u8 = 2;
const OP_REVOKE: u8 = 3;
const OP_REINSTATE: u8 = 4;

type EnumerationOpPlan = std::vec::Vec<(u8, usize, usize, usize)>;

fn enumeration_op_plan() -> impl Strategy<Value = EnumerationOpPlan> {
    prop::collection::vec(
        (
            0u8..5,
            0usize..ENUMERATION_POOL_SIZE,
            0usize..ENUMERATION_POOL_SIZE,
            0usize..ENUMERATION_POOL_SIZE,
        ),
        1..=ENUMERATION_MAX_OPS,
    )
}

fn register_for_role(
    env: &Env,
    client: &RegistryContractClient<'static>,
    is_buyer: bool,
    address: &Address,
) {
    if is_buyer {
        client.register_buyer(address, &map![env]);
    } else {
        client.register_issuer(address, &map![env]);
    }
}

fn batch_register_for_role(
    client: &RegistryContractClient<'static>,
    is_buyer: bool,
    entries: &Vec<(Address, soroban_sdk::Map<String, String>)>,
) -> Vec<Address> {
    if is_buyer {
        client.batch_register_buyers(entries)
    } else {
        client.batch_register_issuers(entries)
    }
}

fn contract_error(code: u32) -> Result<Error, soroban_sdk::InvokeError> {
    Ok(Error::from_contract_error(code))
}

fn assert_duplicate_register_rejected(
    env: &Env,
    client: &RegistryContractClient<'static>,
    is_buyer: bool,
    address: &Address,
) {
    let result = if is_buyer {
        client.try_register_buyer(address, &map![env])
    } else {
        client.try_register_issuer(address, &map![env])
    };
    assert_eq!(
        result.unwrap_err(),
        contract_error(RegistryError::AlreadyRegistered as u32)
    );
}

fn assert_enumeration_matches(
    client: &RegistryContractClient<'static>,
    role: &Role,
    expected: &[Address],
    page_size: u32,
) {
    assert!((1u32..=50).contains(&page_size));
    let count = expected.len() as u32;
    let mut start: u32 = 0;
    let mut seen: usize = 0;
    loop {
        let page = client.list_profiles(role, &start, &page_size);
        assert!(page.len() <= page_size);
        assert_eq!(page, client.list_profiles(role, &start, &page_size));
        let expected_len = (count.saturating_sub(start) as usize).min(page_size as usize);
        assert_eq!(page.len() as usize, expected_len);
        for i in 0..page.len() {
            assert_eq!(page.get(i).unwrap(), expected[seen]);
            seen += 1;
        }

        start = start.saturating_add(page_size);
        if page.len() < page_size {
            break;
        }
    }
    assert_eq!(seen, expected.len());
}

fn run_enumeration_prop_test(is_buyer: bool) {
    let mut runner = TestRunner::new(ProptestConfig::with_cases(10));
    runner
        .run(
            &(enumeration_op_plan(), 1u32..=50, 1u32..=50),
            |(ops, page_size_a, page_size_b)| {
                let (env, client) = setup();
                let admin = Address::generate(&env);
                client.initialize(&admin);
                let role = if is_buyer { Role::Buyer } else { Role::Issuer };

                let addresses: std::vec::Vec<Address> = (0..ENUMERATION_POOL_SIZE)
                    .map(|_| Address::generate(&env))
                    .collect();

                let mut registered = std::vec![false; ENUMERATION_POOL_SIZE];
                let mut expected: std::vec::Vec<Address> = std::vec::Vec::new();

                for &(action, a, b, c) in ops.iter() {
                    match action {
                        OP_REGISTER => {
                            let address = addresses[a].clone();
                            if registered[a] {
                                assert_duplicate_register_rejected(
                                    &env, &client, is_buyer, &address,
                                );
                            } else {
                                register_for_role(&env, &client, is_buyer, &address);
                                registered[a] = true;
                                expected.push(address);
                            }
                        }
                        OP_DUPLICATE => {
                            let address = addresses[a].clone();
                            if !registered[a] {
                                register_for_role(&env, &client, is_buyer, &address);
                                registered[a] = true;
                                expected.push(address.clone());
                            }
                            assert_duplicate_register_rejected(&env, &client, is_buyer, &address);
                        }
                        OP_BATCH => {
                            let indices = [a, b, c];
                            let mut entries = Vec::new(&env);
                            for index in indices.iter() {
                                entries.push_back((addresses[*index].clone(), map![&env]));
                            }
                            let skipped = batch_register_for_role(&client, is_buyer, &entries);

                            let mut seen_in_batch = std::collections::BTreeSet::new();
                            let mut expected_skipped = Vec::new(&env);
                            for index in indices.iter() {
                                if registered[*index] || seen_in_batch.contains(index) {
                                    expected_skipped.push_back(addresses[*index].clone());
                                } else {
                                    seen_in_batch.insert(*index);
                                    registered[*index] = true;
                                    expected.push(addresses[*index].clone());
                                }
                            }
                            assert_eq!(skipped, expected_skipped);
                        }
                        OP_REVOKE => {
                            let address = addresses[a].clone();
                            if registered[a] {
                                client.verify_profile(&address, &true);
                                client.revoke(&address);
                                assert!(!client.is_verified(&address));
                                assert_eq!(
                                    client.get_verification_status(&address),
                                    VerificationStatus::Revoked
                                );
                                assert_eq!(client.get_profile_count(&role), expected.len() as u32);
                            } else {
                                assert_eq!(
                                    client.try_revoke(&address).unwrap_err(),
                                    contract_error(RegistryError::NotFound as u32)
                                );
                            }
                        }
                        OP_REINSTATE => {
                            let address = addresses[a].clone();
                            if registered[a] {
                                client.reinstate(&address);
                                assert_eq!(
                                    client.get_verification_status(&address),
                                    VerificationStatus::Verified
                                );
                            } else {
                                assert_eq!(
                                    client.try_reinstate(&address).unwrap_err(),
                                    contract_error(RegistryError::NotFound as u32)
                                );
                            }
                        }
                        _ => unreachable!(),
                    }
                }

                assert_eq!(client.get_profile_count(&role), expected.len() as u32);

                assert_enumeration_matches(&client, &role, &expected, 1);
                assert_enumeration_matches(&client, &role, &expected, 50);
                assert_enumeration_matches(&client, &role, &expected, page_size_a);
                assert_enumeration_matches(&client, &role, &expected, page_size_b);

                assert_eq!(client.list_profiles(&role, &0, &0).len(), 0);
                assert_eq!(
                    client
                        .list_profiles(&role, &(expected.len() as u32 + 1), &25)
                        .len(),
                    0
                );
                assert_eq!(client.list_profiles(&role, &u32::MAX, &50).len(), 0);

                for invalid_limit in [51u32, 100, u32::MAX] {
                    assert_eq!(
                        client
                            .try_list_profiles(&role, &0, &invalid_limit)
                            .unwrap_err(),
                        contract_error(RegistryError::PageSizeExceeded as u32)
                    );
                }

                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn prop_issuer_enumeration_unique_stable_and_registration_ordered() {
    run_enumeration_prop_test(false);
}

#[test]
fn prop_buyer_enumeration_unique_stable_and_registration_ordered() {
    run_enumeration_prop_test(true);
}

fn persistent_ttl(env: &Env, contract_id: &Address, key: &DataKey) -> u32 {
    env.as_contract(contract_id, || env.storage().persistent().get_ttl(key))
}

fn advance_to_remaining_ttl(env: &Env, contract_id: &Address, key: &DataKey, remaining: u32) {
    let current = persistent_ttl(env, contract_id, key);
    assert!(
        current >= remaining,
        "cannot rewind ledger: current TTL {current} < target {remaining}"
    );
    env.ledger()
        .set_sequence_number(env.ledger().sequence() + (current - remaining));
}

#[test]
fn test_pagination_across_ledger_advancement_keeps_profiles_available() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);

    let mut registered = Vec::new(&env);
    for _ in 0..7 {
        let issuer = Address::generate(&env);
        client.register_issuer(&issuer, &map![&env]);
        registered.push_back(issuer);
    }

    let contract_id = client.address.clone();
    let count_key = DataKey::ProfileCount(Role::Issuer);
    assert_eq!(client.get_profile_count(&Role::Issuer), 7);

    advance_to_remaining_ttl(&env, &contract_id, &count_key, TTL_THRESHOLD - 1);

    let mut paged = Vec::new(&env);
    let pages = [(0u32, 3u32), (3, 3), (6, 3)];
    for (start, limit) in pages {
        let page = client.list_profiles(&Role::Issuer, &start, &limit);
        let expected_len = (7 - start).min(limit);
        assert_eq!(page.len(), expected_len);

        for offset in 0..expected_len {
            let expected = registered.get(start + offset).unwrap();
            assert_eq!(page.get(offset).unwrap(), expected);
            paged.push_back(expected);

            let index_key = DataKey::ProfileIndex(Role::Issuer, start + offset);
            let ttl = persistent_ttl(&env, &contract_id, &index_key);
            assert!(
                ttl >= 1_999_000,
                "index slot {} should be extended on page read, got TTL {ttl}",
                start + offset
            );
        }

        let count_ttl = persistent_ttl(&env, &contract_id, &count_key);
        assert!(
            count_ttl > TTL_THRESHOLD,
            "profile count should remain within the TTL policy, got {count_ttl}"
        );

        if start + expected_len < 7 {
            env.ledger()
                .set_sequence_number(env.ledger().sequence() + TTL_THRESHOLD / 4);
        }
    }

    assert_eq!(paged, registered);

    env.ledger()
        .set_sequence_number(env.ledger().sequence() + TTL_THRESHOLD / 10);
    let first_page = vec![
        &env,
        registered.get(0).unwrap(),
        registered.get(1).unwrap(),
        registered.get(2).unwrap(),
    ];
    assert_eq!(client.list_profiles(&Role::Issuer, &0, &3), first_page);

    assert_eq!(client.get_profile_count(&Role::Issuer), 7);
    for address in registered.iter() {
        let profile_key = DataKey::Profile(address.clone());
        assert!(persistent_ttl(&env, &contract_id, &profile_key) > 0);
        let profile = client.get_profile(&address);
        assert_eq!(profile.role(), Role::Issuer);
        let ttl = persistent_ttl(&env, &contract_id, &profile_key);
        assert!(
            ttl >= 1_999_000,
            "profile TTL should be extended on read, got {ttl}"
        );
    }
}

#[test]
fn test_list_profiles_ttl_extension_at_threshold_boundaries() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);

    let contract_id = client.address.clone();
    let index_key = DataKey::ProfileIndex(Role::Issuer, 0);
    let count_key = DataKey::ProfileCount(Role::Issuer);
    let expected = vec![&env, issuer.clone()];

    advance_to_remaining_ttl(&env, &contract_id, &index_key, TTL_THRESHOLD + 1);
    let index_before = persistent_ttl(&env, &contract_id, &index_key);
    let count_before = persistent_ttl(&env, &contract_id, &count_key);
    assert!(index_before > TTL_THRESHOLD);

    assert_eq!(client.list_profiles(&Role::Issuer, &0, &1), expected);
    assert_eq!(persistent_ttl(&env, &contract_id, &index_key), index_before);
    assert_eq!(persistent_ttl(&env, &contract_id, &count_key), count_before);

    advance_to_remaining_ttl(&env, &contract_id, &index_key, TTL_THRESHOLD - 1);
    let index_just_after = persistent_ttl(&env, &contract_id, &index_key);
    assert!(index_just_after <= TTL_THRESHOLD);

    assert_eq!(client.list_profiles(&Role::Issuer, &0, &1), expected);
    let index_extended = persistent_ttl(&env, &contract_id, &index_key);
    let count_extended = persistent_ttl(&env, &contract_id, &count_key);
    assert!(index_extended > index_just_after);
    assert!(index_extended >= 1_999_000);
    assert!(count_extended >= 1_999_000);
}

// ============== ISSUE #14: NEGATIVE AUTHORIZATION COVERAGE ==============
//
// Every admin entry point must reject a caller that is not the stored admin,
// and must reject calls with no authorization at all. These tests pin both
// cases with explicit auth mocks and assert that a rejected call leaves the
// stored state untouched.

/// Installs exactly one mocked authorization for `signer` invoking
/// `contract.fn_name` with `args`. Any other auth is unavailable, so calls
/// that check `require_auth` on a different address fail.
fn mock_single_auth(
    env: &Env,
    signer: &Address,
    contract: &Address,
    fn_name: &str,
    args: Vec<Val>,
) {
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: signer,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract,
            fn_name,
            args,
            sub_invokes: &[],
        },
    }]);
}

#[test]
fn test_verify_profile_true_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    let flags_before = client.get_profile(&issuer).packed_flags;

    let non_admin = Address::generate(&env);
    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "verify_profile",
        (issuer.clone(), true).into_val(&env),
    );

    assert!(client.try_verify_profile(&issuer, &true).is_err());
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before);
    assert!(!client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Pending
    );
}

#[test]
fn test_verify_profile_false_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    let flags_before = client.get_profile(&issuer).packed_flags;

    let non_admin = Address::generate(&env);
    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "verify_profile",
        (issuer.clone(), false).into_val(&env),
    );

    // A non-admin cannot un-verify a profile either.
    assert!(client.try_verify_profile(&issuer, &false).is_err());
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before);
    assert!(client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Verified
    );
}

#[test]
fn test_verify_profile_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    let flags_before = client.get_profile(&issuer).packed_flags;

    env.set_auths(&[]);
    assert!(client.try_verify_profile(&issuer, &true).is_err());
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before);
    assert!(!client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Pending
    );
}

#[test]
fn test_revoke_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    let flags_before = client.get_profile(&issuer).packed_flags;

    let non_admin = Address::generate(&env);
    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "revoke",
        (issuer.clone(),).into_val(&env),
    );

    assert!(client.try_revoke(&issuer).is_err());
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before);
    assert!(client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Verified
    );
}

#[test]
fn test_revoke_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    let flags_before = client.get_profile(&issuer).packed_flags;

    env.set_auths(&[]);
    assert!(client.try_revoke(&issuer).is_err());
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before);
    assert!(client.is_verified(&issuer));
}

#[test]
fn test_reinstate_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let issuer = Address::generate(&env);
    client.register_issuer(&issuer, &map![&env]);
    client.verify_profile(&issuer, &true);
    client.revoke(&issuer);
    let flags_before = client.get_profile(&issuer).packed_flags;

    let non_admin = Address::generate(&env);
    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "reinstate",
        (issuer.clone(),).into_val(&env),
    );

    assert!(client.try_reinstate(&issuer).is_err());
    assert_eq!(client.get_profile(&issuer).packed_flags, flags_before);
    assert!(!client.is_verified(&issuer));
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Revoked
    );
}

#[test]
fn test_batch_register_issuers_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    let entries = vec![
        &env,
        (first.clone(), map![&env]),
        (second.clone(), map![&env]),
    ];

    let non_admin = Address::generate(&env);
    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "batch_register_issuers",
        (entries.clone(),).into_val(&env),
    );

    assert!(client.try_batch_register_issuers(&entries).is_err());
    assert_eq!(client.get_profile_count(&Role::Issuer), 0);
    assert_eq!(
        client.get_verification_status(&first),
        VerificationStatus::Unregistered
    );
    assert_eq!(
        client.get_verification_status(&second),
        VerificationStatus::Unregistered
    );
}

#[test]
fn test_batch_register_buyers_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    let entries = vec![
        &env,
        (first.clone(), map![&env]),
        (second.clone(), map![&env]),
    ];

    let non_admin = Address::generate(&env);
    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "batch_register_buyers",
        (entries.clone(),).into_val(&env),
    );

    assert!(client.try_batch_register_buyers(&entries).is_err());
    assert_eq!(client.get_profile_count(&Role::Buyer), 0);
    assert_eq!(
        client.get_verification_status(&first),
        VerificationStatus::Unregistered
    );
    assert_eq!(
        client.get_verification_status(&second),
        VerificationStatus::Unregistered
    );
}

#[test]
fn test_batch_registration_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    let issuer = Address::generate(&env);
    let buyer = Address::generate(&env);
    let issuer_entries = vec![&env, (issuer.clone(), map![&env])];
    let buyer_entries = vec![&env, (buyer.clone(), map![&env])];

    env.set_auths(&[]);
    assert!(client.try_batch_register_issuers(&issuer_entries).is_err());
    assert!(client.try_batch_register_buyers(&buyer_entries).is_err());

    assert_eq!(client.get_profile_count(&Role::Issuer), 0);
    assert_eq!(client.get_profile_count(&Role::Buyer), 0);
    assert_eq!(
        client.get_verification_status(&issuer),
        VerificationStatus::Unregistered
    );
    assert_eq!(
        client.get_verification_status(&buyer),
        VerificationStatus::Unregistered
    );
}

#[test]
fn test_pause_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));

    env.set_auths(&[]);
    assert!(client.try_pause().is_err());

    // Still live after the rejected pause.
    env.mock_all_auths();
    assert!(client.register_issuer(&Address::generate(&env), &map![&env]));
}

#[test]
fn test_unpause_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    client.initialize(&Address::generate(&env));
    client.pause();

    env.set_auths(&[]);
    assert!(client.try_unpause().is_err());

    // Still paused after the rejected unpause.
    env.mock_all_auths();
    assert!(client
        .try_register_issuer(&Address::generate(&env), &map![&env])
        .is_err());

    // The real admin can still resume it.
    client.unpause();
    assert!(client.register_issuer(&Address::generate(&env), &map![&env]));
}

#[test]
fn test_transfer_ownership_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let new_admin = Address::generate(&env);
    let non_admin = Address::generate(&env);

    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "transfer_ownership",
        (new_admin.clone(),).into_val(&env),
    );

    assert!(client.try_transfer_ownership(&new_admin).is_err());
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_transfer_ownership_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let new_admin = Address::generate(&env);

    env.set_auths(&[]);
    assert!(client.try_transfer_ownership(&new_admin).is_err());
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_transfer_admin_non_admin_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let new_admin = Address::generate(&env);
    let non_admin = Address::generate(&env);

    mock_single_auth(
        &env,
        &non_admin,
        &client.address,
        "transfer_admin",
        (new_admin.clone(),).into_val(&env),
    );

    assert!(client.try_transfer_admin(&new_admin).is_err());
    assert_eq!(client.get_admin(), admin);
}

#[test]
fn test_transfer_admin_missing_auth_rejected_and_state_unchanged() {
    let (env, client) = setup();
    let admin = Address::generate(&env);
    client.initialize(&admin);
    let new_admin = Address::generate(&env);

    env.set_auths(&[]);
    assert!(client.try_transfer_admin(&new_admin).is_err());
    assert_eq!(client.get_admin(), admin);
}
