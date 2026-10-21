pub const TTL_THRESHOLD: u32 = clearledger_ttl::THRESHOLD;
pub const TTL_EXTEND_TO: u32 = clearledger_ttl::EXTEND_TO;

/// Maximum number of assets a single [`get_assets_page`] call may return.
///
/// The asset index lives in instance storage, so an unbounded read hydrates
/// every registered asset in one invocation and its cost grows with the
/// registry. `50` mirrors the workspace's other batch ceilings (invoice
/// `MAX_PAGE_SIZE`, escrow `MAX_HISTORY_LIMIT`), keeping each page within a
/// predictable Soroban resource budget.
///
/// Requests with `limit == 0` or `limit > MAX_ASSET_PAGE_LIMIT` panic with
/// [`PoolFactoryError::InvalidLimit`](crate::PoolFactoryError::InvalidLimit).
///
/// [`get_assets_page`]: crate::PoolFactoryContract::get_assets_page
pub const MAX_ASSET_PAGE_LIMIT: u32 = 50;


/// Maximum number of registered assets one [`get_aggregate_stats_page`] call
/// may walk (and therefore at most that many pool `get_stats` cross-contract
/// calls).
///
/// An unbounded aggregate read hydrates every registered asset and invokes
/// every pool in a single invocation, so its resource use grows without bound
/// as the registry grows. Capping a page keeps each call inside a predictable
/// Soroban budget; `50` mirrors the workspace's other batch ceilings (invoice
/// `MAX_PAGE_SIZE`, escrow `MAX_HISTORY_LIMIT`).
///
/// Requests with `limit == 0` or `limit > MAX_AGGREGATE_STATS_PAGE_LIMIT`
/// panic with
/// [`PoolFactoryError::InvalidAggregateStatsLimit`](crate::PoolFactoryError::InvalidAggregateStatsLimit).
///
/// [`get_aggregate_stats_page`]: crate::PoolFactoryContract::get_aggregate_stats_page
pub const MAX_AGGREGATE_STATS_PAGE_LIMIT: u32 = 50;

/// Minimum initial deposit the factory passes to a newly deployed pool
/// instance's `initialize`. Mirrors `clearledger_pool::DEFAULT_MIN_INITIAL_DEPOSIT`;
/// duplicated here rather than imported because `clearledger-pool` is a
/// dev-dependency only (see Cargo.toml), so production code cannot reference
/// its constants. Callers deploying a pool for an asset with different
/// decimals than this default assumes should register it via
/// `register_existing_pool` instead, after initializing it directly with a
/// suitable `min_initial_deposit`.
pub const DEFAULT_MIN_INITIAL_DEPOSIT: u128 = 10_000_000;

/// SEP-41 share token name given to every pool instance this factory deploys.
///
/// `pool::initialize` requires a share name, symbol and decimals, and the
/// factory is the only caller on that path, but `register_asset` receives just
/// the asset *address* — it is never told what to call the share token, and it
/// cannot ask the asset either (a Stellar asset is not required to expose
/// `name()`/`symbol()`, and `register_asset` accepts plain account addresses).
/// So every instance starts from these defaults. Threading per-asset metadata
/// through `register_asset` is the follow-up that would make factory pools
/// distinguishable in a wallet; until then, a deploy that needs its own
/// metadata should initialize the pool directly and wire it up with
/// `register_existing_pool`, which is also [`DEFAULT_MIN_INITIAL_DEPOSIT`]'s
/// escape hatch.
pub const DEFAULT_SHARE_NAME: &str = "ClearLedger Pool Shares";
/// See [`DEFAULT_SHARE_NAME`].
pub const DEFAULT_SHARE_SYMBOL: &str = "TT-POOL";
/// See [`DEFAULT_SHARE_NAME`]. Matches the 7 decimals that
/// [`DEFAULT_MIN_INITIAL_DEPOSIT`] assumes.
pub const DEFAULT_SHARE_DECIMALS: u32 = 7;
