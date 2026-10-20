use soroban_sdk::{contracttype, Address};

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
