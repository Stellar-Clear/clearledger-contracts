use soroban_sdk::{Address, BytesN, Env, Symbol};

pub fn upgraded(env: &Env, wasm_hash: &BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "upgraded"), wasm_hash.clone()), ());
}

pub fn funds_locked(env: &Env, invoice_id: &BytesN<32>, issuer: &Address, amount: u128) {
    env.events().publish(
        (
            Symbol::new(env, "funds_locked"),
            invoice_id.clone(),
            issuer.clone(),
        ),
        amount,
    );
}

pub fn released_to_issuer(env: &Env, invoice_id: &BytesN<32>, issuer: &Address, amount: u128) {
    env.events().publish(
        (
            Symbol::new(env, "released_to_issuer"),
            invoice_id.clone(),
            issuer.clone(),
        ),
        amount,
    );
}

pub fn released_to_pool(env: &Env, invoice_id: &BytesN<32>, pool: &Address, amount: u128) {
    env.events().publish(
        (
            Symbol::new(env, "released_to_pool"),
            invoice_id.clone(),
            pool.clone(),
        ),
        amount,
    );
}

pub fn default_resolved(
    env: &Env,
    invoice_id: &BytesN<32>,
    pool: &Address,
    caller: &Address,
    amount: u128,
) {
    env.events().publish(
        (
            Symbol::new(env, "default_resolved"),
            invoice_id.clone(),
            pool.clone(),
            caller.clone(),
        ),
        amount,
    );
}

#[allow(dead_code)]
pub fn ownership_transferred(env: &Env, old_admin: &Address, new_admin: &Address) {
    env.events().publish(
        (Symbol::new(env, "ownership_transferred"), old_admin.clone()),
        new_admin.clone(),
    );
}

/// Emitted whenever on-chain escrow history is pruned, either automatically
/// while appending or explicitly via `compact_history`.
///
/// `retained` is the number of events still stored on-chain after the prune and
/// `removed` is the number dropped from storage in this operation. Pruned
/// entries remain reconstructible from the contract's per-action events
/// (`funds_locked`, `released_to_issuer`, `released_to_pool`,
/// `default_resolved`) by indexers that ingested them.
pub fn history_compacted(env: &Env, invoice_id: &BytesN<32>, retained: u32, removed: u32) {
    env.events().publish(
        (Symbol::new(env, "history_compacted"), invoice_id.clone()),
        (retained, removed),
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
