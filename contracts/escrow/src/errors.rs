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
    /// Returned when releasing funds to a recipient that is invalid (e.g., not the invoice issuer).
    InvalidRecipient = 7,
    /// Returned when attempting to initialize with colliding addresses (e.g., pool_contract == usdc_asset).
    InvalidConfig = 8,
}
