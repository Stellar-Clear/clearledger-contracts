use soroban_sdk::{contracttype, Address, Vec};

#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    Admin,
    AssetCount,
    AssetIndex(u32),
    /// The pool instance the factory manages for a given asset.
    ///
    /// Written by both `register_asset` (freshly deployed instance) and
    /// `register_existing_pool` (pre-existing instance adopted during
    /// migration) so `get_pool_for_asset` reads one key regardless of how the
    /// instance came to exist.
    PoolForAsset(Address),
}

/// One bounded page of the factory's registered asset list.
///
/// Returned by [`PoolFactoryContract::get_assets_page`](crate::PoolFactoryContract::get_assets_page).
/// `items` preserves registration order, and `next_cursor` is the cursor to
/// pass to the next call, or `None` when the walk is complete. Because the
/// underlying index is append-only, cursors are stable across registrations:
/// an asset appended between two reads is returned at the end of the walk.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetPage {
    /// Assets in registration order, at most `limit` long.
    pub items: Vec<Address>,
    /// Zero-based index to pass to the next call, or `None` on the final page.
    pub next_cursor: Option<u32>,
}

/// Aggregate, pool-wide accounting snapshot returned by `get_pool_stats`.
/// This is a verbatim copy of `clearledger_pool::PoolStats` so the factory
/// can return it from `get_aggregate_stats` without introducing a cyclic or
/// bloated dependency on the pool crate.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PoolStats {
    pub total_deposits: u128,
    pub total_funded: u128,
    pub available_liquidity: u128,
    pub utilization_rate_bps: u32,
    pub total_yield_distributed: u128,
    pub total_loss_realised: u128,
    pub active_invoice_count: u32,
    pub total_shares: u128,
    pub max_utilization_bps: u32,
}
