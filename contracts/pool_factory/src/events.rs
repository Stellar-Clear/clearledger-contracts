use soroban_sdk::{Address, Env, Symbol};

/// Published when the pool factory is initialized.
pub fn contract_initialized(env: &Env, admin: &Address) {
    env.events().publish(
        (Symbol::new(env, "contract_initialized"), admin.clone()),
        (),
    );
}

/// Published after `transfer_ownership` moves the factory admin from
/// `old_admin` to `new_admin`, so indexers can follow which key now controls
/// asset registration.
pub fn ownership_transferred(env: &Env, old_admin: &Address, new_admin: &Address) {
    env.events().publish(
        (Symbol::new(env, "ownership_transferred"), old_admin.clone()),
        new_admin.clone(),
    );
}

/// Published after `register_asset` deploys and initializes a new pool
/// instance, so off-chain indexers (e.g. ClearLedger-app) learn about the new
/// instance the same way they learn about other state-changing calls via
/// `contracts/pool/src/events.rs`.
pub fn pool_instance_created(env: &Env, asset: &Address, pool_address: &Address) {
    env.events().publish(
        (Symbol::new(env, "pool_instance_created"), asset.clone()),
        pool_address.clone(),
    );
}

/// Published after an existing pool has been registered for an asset.
pub fn existing_pool_registered(env: &Env, asset: &Address, pool_address: &Address) {
    env.events().publish(
        (Symbol::new(env, "existing_pool_registered"), asset.clone()),
        pool_address.clone(),
    );
}
