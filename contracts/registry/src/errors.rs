use soroban_sdk::contracterror;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    AlreadyInitialized = 1,
    AlreadyRegistered = 2,
    NotFound = 3,
    NotInitialized = 4,
    BatchSizeExceeded = 5,
    InvalidMetadata = 6,
    NotRegistered = 7,
    /// The requested `list_profiles` page size is above
    /// `MAX_LIST_LIMIT` (50), mirroring `BatchSizeExceeded` for the batch
    /// registration entry points. Pagination must be requested in bounded
    /// pages so a single call can never walk the whole index.
    PageSizeExceeded = 8,
    /// `initialize` was given an admin or dependency address equal to this
    /// contract, which cannot authorize calls or be a valid external
    /// dependency.
    InvalidConfiguration = 9,
}
