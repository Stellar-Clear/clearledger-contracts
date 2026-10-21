use soroban_sdk::{contracttype, BytesN, Vec};

/// Record of an escrow lock stored for a given invoice.
///
/// # Invariants
///
/// - `amount` must be greater than zero — guaranteed by [`EscrowContract::lock`].
/// - `locked_at` is set to `env.ledger().timestamp()` at lock time and is never
///   updated for the lifetime of the record.
/// - Partial pool releases reduce `amount` and retain this record. Terminal
///   release or default removes it; afterward `get_locked` / `get_locked_at`
///   return `0`.
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
    /// Set from the `issuer` argument of [`EscrowContract::lock`] and treated as
    /// the sole source of truth by [`EscrowContract::release_to_issuer`]: a
    /// caller-supplied address must match this value, and the release transfer
    /// always uses this stored value.
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
/// Events are stored individually under [`DataKey::HistoryEvent`] with their
/// append index, so a paginated read hydrates only the requested slice. The
/// append index remains stable when older entries are pruned; `HistoryStart`
/// identifies the oldest retained index. Full event history remains in the
/// ledger event stream for indexers.
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

/// One bounded page of an invoice's escrow history.
///
/// Returned by [`EscrowContract::get_history_page`]. Pass [`Self::next_cursor`]
/// back to the next call to continue the walk; `None` means the walk is
/// complete. Because the cursor is the append index and history is
/// append-only, events added between pages never shift, duplicate, or skip
/// previously returned entries.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryPage {
    /// Events in chronological (append) order, at most `limit` long.
    pub items: Vec<EscrowEvent>,
    /// Append index to pass to the next call, or `None` when the walk is complete.
    pub next_cursor: Option<u32>,
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
    /// Legacy event history for a given invoice. The inner `BytesN<32>` is the
    /// invoice ID. Value type: `Vec<EscrowEvent>`.
    ///
    /// Only read as a fallback for history written before pagination. On the
    /// first append it is migrated to [`DataKey::HistoryEvent`] entries and
    /// removed.
    History(BytesN<32>),
    /// A single history event at `(invoice_id, index)`. Value type:
    /// [`EscrowEvent`]. The index is also the pagination cursor.
    HistoryEvent(BytesN<32>, u32),
    /// Number of history events stored for an invoice. Value type: `u32`.
    HistoryCount(BytesN<32>),
    /// First retained append index after automatic or manual history pruning.
    /// The monotonically increasing `HistoryCount` remains the cursor ceiling.
    HistoryStart(BytesN<32>),
}
