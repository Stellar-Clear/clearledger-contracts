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
    /// `get_assets_page` was given a zero limit or a limit above
    /// [`MAX_ASSET_PAGE_LIMIT`](crate::MAX_ASSET_PAGE_LIMIT), which would
    /// defeat the per-call cost bound pagination exists to provide.
    InvalidLimit = 10,
    /// `get_assets_page` was given a cursor past the end of the asset index.
    InvalidCursor = 11,
}
