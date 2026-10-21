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
    /// The requested `list_profiles_page` page size is zero. Cursor
    /// pagination requires a positive limit so every call makes progress;
    /// use the cursor alone to detect the end of the list.
    InvalidPageSize = 9,
    /// An admin-transfer entry point was given the registry contract's own
    /// address, which can never be a valid external admin/controller.
    InvalidAddress = 10,
    /// The `list_profiles_page` cursor is past the end of the role's current
    /// enumeration index. Cursors are opaque positions returned as
    /// `ProfilePage::next_cursor`; a cursor beyond the current
    /// `get_profile_count(role)` cannot be resumed and is rejected instead of
    /// silently returning an empty page.
    InvalidCursor = 11,
}
