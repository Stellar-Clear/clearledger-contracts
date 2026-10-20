use soroban_sdk::{contracttype, BytesN};

/// Record of an escrow lock stored for a given invoice.
///
/// # Invariants
///
/// - `amount` must be greater than zero — guaranteed by [`EscrowContract::lock`].
/// - `locked_at` is set to `env.ledger().timestamp()` at lock time and is never
///   updated for the lifetime of the record.
/// - A record is removed (via `persistent().remove`) when escrow is released
///   or default-handled; after removal `get_locked` / `get_locked_at` return `0`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct EscrowRecord {
    /// 32-byte invoice identifier scoped to this escrow lock.
    /// Used as the key suffix in [`DataKey::Locked`].
    pub invoice_id: BytesN<32>,
    /// Amount of USDC (in stroops, 7 decimals) held in escrow.
    /// Set from the `amount` argument of [`EscrowContract::lock`].
    pub amount: u128,
    /// Unix timestamp (seconds) recorded when the lock was created.
    /// Used to enforce the minimum lock period (grace period) before
    /// a [`default`](EscrowAction::DefaultHandled) can be triggered.
    pub locked_at: u64,
    /// The invoice issuer address that is authorized to receive released funds.
    /// Set from the `issuer` argument of [`EscrowContract::lock`] and validated
    /// in [`EscrowContract::release_to_issuer`].
    pub issuer: soroban_sdk::Address,
}

/// Actions that can be recorded in the escrow event history.
///
/// Each variant corresponds to a state transition of an escrow lock.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscrowAction {
    /// Funds were locked in escrow against an invoice.
    Locked,
    /// Locked funds were released to the invoice issuer.
    ReleasedToIssuer,
    /// Locked funds were returned to the pool (partial or full repayment).
    ReleasedToPool,
    /// Lock was resolved as a default and funds returned to the pool.
    DefaultHandled,
}

/// An entry in the escrow event history for a given invoice.
///
/// History is stored under [`DataKey::History`] as a `Vec<EscrowEvent>`
/// and is append-only — see [`EscrowContract::append_history`].
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowEvent {
    /// 32-byte invoice identifier this event relates to.
    pub invoice_id: BytesN<32>,
    /// The action that produced this event. See [`EscrowAction`].
    pub action: EscrowAction,
    /// Amount of USDC (in stroops) involved in this action.
    pub amount: u128,
    /// Unix timestamp (seconds) when this event was recorded.
    pub timestamp: u64,
    /// The caller address that triggered this event (for DefaultHandled events).
    pub caller: Option<soroban_sdk::Address>,
}

/// Storage keys used by the escrow contract.
///
/// Instance keys (no parameter) are stored with
/// [`env.storage().instance().set()`](soroban_sdk::storage::InstanceAccessor::set)
/// and are managed under the instance TTL. Persistent keys (`Locked`, `History`)
/// are stored with
/// [`env.storage().persistent().set()`](soroban_sdk::storage::PersistentAccessor::set)
/// and their TTL is explicitly extended on every write.
#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    /// Admin address authorised to trigger default resolution.
    Admin,
    /// Address of the pool contract that can lock / release / default escrow.
    PoolContract,
    /// Address of the USDC token contract held in escrow.
    UsdcAsset,
    /// Active escrow lock for a given invoice. The inner `BytesN<32>` is the
    /// invoice ID. Value type: [`EscrowRecord`].
    Locked(BytesN<32>),
    /// Event history for a given invoice. The inner `BytesN<32>` is the
    /// invoice ID. Value type: `Vec<EscrowEvent>`.
    History(BytesN<32>),
}
