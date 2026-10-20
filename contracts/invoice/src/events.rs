use soroban_sdk::{Address, BytesN, Env, Symbol};

pub fn invoice_created(
    env: &Env,
    invoice_id: &BytesN<32>,
    issuer: &Address,
    buyer: &Address,
    face_value: u128,
    funding_asset: &Address,
) {
    env.events().publish(
        (
            Symbol::new(env, "invoice_created"),
            invoice_id.clone(),
            issuer.clone(),
            buyer.clone(),
            funding_asset.clone(),
        ),
        face_value,
    );
}

pub fn invoice_listed(env: &Env, invoice_id: &BytesN<32>, discount_bps: u32) {
    env.events().publish(
        (Symbol::new(env, "invoice_listed"), invoice_id.clone()),
        discount_bps,
    );
}

pub fn invoice_cancelled(env: &Env, invoice_id: &BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "invoice_cancelled"), invoice_id.clone()),
        (),
    );
}

pub fn upgraded(env: &Env, wasm_hash: &BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "upgraded"), wasm_hash.clone()), ());
}

pub fn invoice_funded(env: &Env, invoice_id: &BytesN<32>, funded_amount: u128) {
    env.events().publish(
        (Symbol::new(env, "invoice_funded"), invoice_id.clone()),
        funded_amount,
    );
}

pub fn invoice_shipped(env: &Env, invoice_id: &BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "invoice_shipped"), invoice_id.clone()),
        (),
    );
}

pub fn delivery_confirmed(env: &Env, invoice_id: &BytesN<32>, confirmer: &Address) {
    env.events().publish(
        (
            Symbol::new(env, "delivery_confirmed"),
            invoice_id.clone(),
            confirmer.clone(),
        ),
        (),
    );
}

pub fn both_confirmed(env: &Env, invoice_id: &BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "both_confirmed"), invoice_id.clone()), ());
}

pub fn invoice_repaid(env: &Env, invoice_id: &BytesN<32>, amount: u128) {
    env.events().publish(
        (Symbol::new(env, "invoice_repaid"), invoice_id.clone()),
        amount,
    );
}

pub fn partial_repayment_received(
    env: &Env,
    invoice_id: &BytesN<32>,
    amount: u128,
    remaining_balance: u128,
) {
    env.events().publish(
        (
            Symbol::new(env, "partial_repayment_received"),
            invoice_id.clone(),
        ),
        (amount, remaining_balance),
    );
}

pub fn invoice_defaulted(env: &Env, invoice_id: &BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "invoice_defaulted"), invoice_id.clone()),
        (),
    );
}

pub fn invoice_expired(env: &Env, invoice_id: &BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "invoice_expired"), invoice_id.clone()),
        (),
    );
}

pub fn expiry_window_set(env: &Env, window: u64) {
    env.events()
        .publish((Symbol::new(env, "expiry_window_set"),), window);
}

/// Emitted once per `batch_create` call, after the per-invoice
/// `invoice_created` events, summarizing the batch outcome. `created` is the
/// number of invoices persisted; `failed` is the number of rejected entries
/// (always `0` today, because `batch_create` is atomic and reverts the whole
/// batch on the first invalid entry — the field exists so indexers can key off
/// a stable batch summary shape if partial-success semantics are added later).
pub fn batch_invoices_created(env: &Env, created: u32, failed: u32) {
    env.events().publish(
        (Symbol::new(env, "batch_invoices_created"),),
        (created, failed),
    );
}

/// Emitted once per `batch_list_for_financing` call, summarizing the outcome
/// as `(listed, failed)` counts. Unlike `batch_create`, listing is
/// per-entry tolerant: entries that fail validation are reported in the
/// returned failed-list rather than reverting the batch, so this event can
/// legitimately report a non-zero `failed`.
pub fn batch_invoices_listed(env: &Env, listed: u32, failed: u32) {
    env.events().publish(
        (Symbol::new(env, "batch_invoices_listed"),),
        (listed, failed),
    );
}

pub fn ownership_transferred(env: &Env, from: &Address, to: &Address) {
    env.events().publish(
        (
            Symbol::new(env, "ownership_transferred"),
            from.clone(),
            to.clone(),
        ),
        (),
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

pub fn pool_contract_updated(env: &Env, old: &Address, new: &Address) {
    env.events().publish(
        (
            Symbol::new(env, "pool_contract_updated"),
            old.clone(),
            new.clone(),
        ),
        (),
    );
}

pub fn attestation_submitted(
    env: &Env,
    invoice_id: &BytesN<32>,
    agent_id: &Symbol,
    risk_score: u32,
) {
    env.events().publish(
        (
            Symbol::new(env, "attestation_submitted"),
            invoice_id.clone(),
            agent_id.clone(),
        ),
        risk_score,
    );
}

pub fn agent_registry_contract_updated(env: &Env, old: &Address, new: &Address) {
    env.events().publish(
        (
            Symbol::new(env, "agent_registry_contract_updated"),
            old.clone(),
            new.clone(),
        ),
        (),
    );
}

pub fn contract_initialized(env: &Env, admin: &Address, registry_contract: &Address) {
    env.events().publish(
        (
            Symbol::new(env, "contract_initialized"),
            admin.clone(),
            registry_contract.clone(),
        ),
        (),
    );
}

pub fn supported_asset_added(env: &Env, asset: &Address) {
    env.events().publish(
        (Symbol::new(env, "supported_asset_added"), asset.clone()),
        (),
    );
}

pub fn supported_asset_removed(env: &Env, asset: &Address) {
    env.events().publish(
        (Symbol::new(env, "supported_asset_removed"), asset.clone()),
        (),
    );
}

pub fn escrow_contract_updated(env: &Env, old: &Address, new: &Address) {
    env.events().publish(
        (
            Symbol::new(env, "escrow_contract_updated"),
            old.clone(),
            new.clone(),
        ),
        (),
    );
}
