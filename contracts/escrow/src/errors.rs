use soroban_sdk::contracterror;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscrowError {
    /// Returned when attempting to initialize a contract that has already been initialized.
    AlreadyInitialized = 1,
    /// Returned when no escrow record exists for the given invoice ID.
    NotFound = 2,
    /// Returned when the caller (e.g. not the invoice contract) lacks authorization for the action.
    NotAuthorized = 3,
    /// Returned when attempting to lock funds for an invoice that already has locked funds.
    AlreadyLocked = 4,
    /// Returned when a zero amount is provided for locking, release, or repayment.
    InvalidAmount = 5,
    /// Returned when an action is attempted before the contract has been initialized.
    NotInitialized = 6,
    /// Returned when releasing funds to a recipient that is invalid (e.g., not
    /// the issuer recorded at lock time, or the escrow/pool contract address).
    InvalidRecipient = 7,
    /// Returned when attempting to initialize with colliding addresses (e.g., pool_contract == usdc_asset).
    InvalidConfig = 8,
    /// Returned when a paginated history read requests a zero limit or a limit
    /// above [`MAX_HISTORY_LIMIT`](crate::MAX_HISTORY_LIMIT).
    InvalidLimit = 14,
    /// Returned when a paginated history read is given a cursor beyond the
    /// number of recorded events.
    InvalidCursor = 15,

    /// Returned when the requested history retention is outside the supported
    /// range (`MIN_HISTORY_EVENTS..=MAX_HISTORY_EVENTS`).
    ///
    /// Codes 9-12 are reserved by open issues #83 and #50.
    InvalidRetention = 13,
    /// Returned when `initialize` is given a pool/admin address that aliases
    /// another configured contract role.
    InvalidContractAddress = 16,
    /// Returned when `initialize` is given a `usdc_asset` that aliases the
    /// admin, pool, or this escrow contract instead of a distinct token.
    InvalidAsset = 17,
}
