use soroban_sdk::{Address, Env, Symbol};

pub fn contract_initialized(env: &Env, admin: &Address) {
    env.events().publish(
        (Symbol::new(env, "contract_initialized"), admin.clone()),
        (),
    );
}

pub fn issuer_registered(env: &Env, address: &Address) {
    env.events()
        .publish((Symbol::new(env, "issuer_registered"), address.clone()), ());
}

pub fn buyer_registered(env: &Env, address: &Address) {
    env.events()
        .publish((Symbol::new(env, "buyer_registered"), address.clone()), ());
}

pub fn address_revoked(env: &Env, address: &Address) {
    env.events()
        .publish((Symbol::new(env, "address_revoked"), address.clone()), ());
}

pub fn batch_registered(env: &Env, registered: u32, skipped: u32) {
    env.events().publish(
        (Symbol::new(env, "batch_registered"),),
        (registered, skipped),
    );
}

pub fn profile_verified(env: &Env, address: &Address, status: bool) {
    env.events().publish(
        (Symbol::new(env, "profile_verified"), address.clone()),
        status,
    );
}

pub fn profile_updated(env: &Env, address: &Address) {
    env.events()
        .publish((Symbol::new(env, "profile_updated"), address.clone()), ());
}

pub fn address_reinstated(env: &Env, address: &Address) {
    env.events().publish(
        (Symbol::new(env, "address_reinstated"), address.clone()),
        (),
    );
}

pub fn ownership_transferred(env: &Env, old_admin: &Address, new_admin: &Address) {
    env.events().publish(
        (Symbol::new(env, "ownership_transferred"), old_admin.clone()),
        new_admin.clone(),
    );
}

pub fn admin_transferred(env: &Env, old_admin: &Address, new_admin: &Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transferred"), old_admin.clone()),
        new_admin.clone(),
    );
}

/// Emitted by `pause` when the emergency circuit breaker is engaged. `admin` is
/// the address that authorized the pause, indexed so indexers can track who
/// flipped the breaker.
pub fn paused(env: &Env, admin: &Address) {
    env.events()
        .publish((Symbol::new(env, "paused"), admin.clone()), ());
}

/// Emitted by `unpause` when the emergency circuit breaker is disengaged.
pub fn unpaused(env: &Env, admin: &Address) {
    env.events()
        .publish((Symbol::new(env, "unpaused"), admin.clone()), ());
}
