use soroban_sdk::contracterror;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolFactoryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    AssetAlreadyRegistered = 3,
    /// `register_asset` could not read the registry contract from the invoice
    /// contract it was handed, because that invoice contract has not been
    /// initialized (or is not an invoice contract at all).
    InvoiceNotInitialized = 4,
    /// `register_existing_pool` was given an address that does not expose a
    /// pool initialized for the requested asset.
    InvalidPool = 5,
    /// `get_aggregate_stats_page` was given a zero limit or a limit above
    /// [`MAX_AGGREGATE_STATS_PAGE_LIMIT`](crate::MAX_AGGREGATE_STATS_PAGE_LIMIT),
    /// which would defeat the per-call cost bound pagination exists to
    /// provide.
    ///
    /// Codes 6 through 11 are reserved by other in-flight factory changes, so
    /// this allocation starts at 12 and existing codes are never renumbered.
    InvalidAggregateStatsLimit = 12,
    /// `get_aggregate_stats_page` was given a cursor past the end of the asset
    /// index.
    InvalidAggregateStatsCursor = 13,
}
