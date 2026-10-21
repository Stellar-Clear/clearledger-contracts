use soroban_sdk::contracterror;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAmount = 4,
    InsufficientLiquidity = 5,
    NoShares = 6,
    InsufficientShares = 7,
    InvoiceNotListed = 8,
    InvoiceNotFound = 10,
    AssetMismatch = 11,
    UtilizationCapExceeded = 12,
    Overflow = 13,
    MinimumDeposit = 14,
    InvalidConfiguration = 15,
    AlreadyFunded = 16,
    ActiveCountUnderflow = 17,
    IssuerNotVerified = 18,
    BuyerNotVerified = 19,
    EscrowAssetMismatch = 20,
    EscrowDefaultNotReleased = 21,
    /// Protocol fee basis points exceed `MAX_PROTOCOL_FEE_BPS` (2000 bps = 20%).
    FeeTooHigh = 22,
    /// Transfer amount exceeds sender's share balance.
    InsufficientBalance = 23,
    /// `transfer_from` amount exceeds the remaining allowance (or the allowance
    /// has expired, which reads back as `0`).
    InsufficientAllowance = 24,
    /// `approve` was given an `expiration_ledger` at or before the current
    /// ledger sequence while also granting a non-zero amount.
    InvalidExpiration = 25,
    /// A batch entry point was given more entries than `MAX_BATCH_SIZE`. The
    /// cap bounds the worst-case cost of a single batch so a caller cannot
    /// exceed the per-transaction budget by batching.
    BatchSizeExceeded = 26,
    /// `set_max_utilization` was given a cap above `10_000` bps (100%).
    /// Utilization is expressed in basis points, so any larger value is not a
    /// meaningful risk parameter.
    InvalidUtilizationCap = 27,
    /// A treasury address aliases the pool itself or one of the contracts in
    /// its settlement wiring (invoice, escrow, registry, or funding asset).
    /// Protocol fees must not be routed back into infrastructure addresses.
    InvalidTreasury = 28,
    /// A configured contract address aliases `admin`, the pool itself, or
    /// another already-wired contract role.
    InvalidContractAddress = 29,
    /// A configured funding-asset address aliases `admin` or another contract
    /// role, so it cannot be a distinct token contract.
    InvalidAsset = 30,
    /// The SEP-41 share name or symbol passed to `initialize` was empty.
    InvalidShareMetadata = 31,
}
