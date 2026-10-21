use crate::types::RevocationReason;
use soroban_sdk::{Address, Env, Symbol};

use crate::Role;

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

/// Emitted by `revoke`. The profile role is indexed in `topic[2]`; the
/// machine-readable revocation reason is encoded in the event data.
pub fn address_revoked(env: &Env, address: &Address, role: &Role, reason: &RevocationReason) {
    env.events().publish(
        (
            Symbol::new(env, "address_revoked"),
            address.clone(),
            role.clone(),
        ),
        reason.clone(),
    );
}

pub fn batch_registered(env: &Env, registered: u32, skipped: u32) {
    env.events().publish(
        (Symbol::new(env, "batch_registered"),),
        (registered, skipped),
    );
}

/// Emitted by `verify_profile`. `role` is indexed in `topic[2]` so indexers
/// can classify the profile without an extra `get_profile` read. `data`
/// remains the `bool` verification status; the authorizing admin added by
/// #165 should be appended after `role` (e.g. `topic[3]`) to keep this ABI
/// additive.
pub fn profile_verified(env: &Env, address: &Address, status: bool, role: &Role) {
    env.events().publish(
        (
            Symbol::new(env, "profile_verified"),
            address.clone(),
            role.clone(),
        ),
        status,
    );
}

pub fn profile_updated(env: &Env, address: &Address) {
    env.events()
        .publish((Symbol::new(env, "profile_updated"), address.clone()), ());
}

/// Emitted by `reinstate`. `role` is indexed in `topic[2]` so indexers can
/// classify the profile without an extra `get_profile` read.
pub fn address_reinstated(env: &Env, address: &Address, role: &Role) {
    env.events().publish(
        (
            Symbol::new(env, "address_reinstated"),
            address.clone(),
            role.clone(),
        ),
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
