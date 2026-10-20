use soroban_sdk::contracterror;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvoiceError {
    AlreadyInitialized = 1,
    NotFound = 2,
    NotAuthorized = 3,
    IssuerNotVerified = 4,
    BuyerNotVerified = 5,
    InvalidFaceValue = 6,
    InvalidDueDate = 7,
    InvalidStatusTransition = 8,
    DiscountTooHigh = 9,
    AlreadyConfirmed = 10,
    DueDateNotPassed = 11,
    UnsupportedAsset = 13,
    ListingNotExpired = 14,
    MathOverflow = 15,
    InvalidAmount = 16,
    InvalidDiscount = 12,
    CounterOverflow = 17,
    InvalidExpiryWindow = 18,
    InvalidParticipants = 19,
    NotInitialized = 20,
    UntrustedSigner = 21,
    AlreadyAttested = 22,
    VerificationRequired = 23,
    CrossContractCallFailed = 24,
    RepaymentExceedsBalance = 25,
    InvalidConfiguration = 26,
    /// A paginated `get_by_*` query was given a `page_size` larger than
    /// [`MAX_PAGE_SIZE`](crate::MAX_PAGE_SIZE), which would defeat the
    /// per-call gas bound pagination exists to provide.
    InvalidPageSize = 27,
    /// A batch entry point was given more entries than
    /// [`MAX_BATCH_SIZE`](crate::MAX_BATCH_SIZE), which would defeat the
    /// per-call cost bound batching exists to provide.
    BatchSizeExceeded = 28,
}
