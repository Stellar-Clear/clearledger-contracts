//! Shared TTL constants re-exported from the workspace-level `clearledger-ttl`
//! crate so the bump policy stays consistent across all contracts.
//!
//! `TTL_THRESHOLD` is the minimum number of ledgers an entry must have
//! remaining before it is extended (25% of `TTL_EXTEND_TO`), and
//! `TTL_EXTEND_TO` is the number of ledgers the entry is extended to.

pub use clearledger_ttl::EXTEND_TO as TTL_EXTEND_TO;
pub use clearledger_ttl::THRESHOLD as TTL_THRESHOLD;

/// Default minimum initial deposit floor, used when `initialize` is not given
/// an explicit `min_initial_deposit` and as the fallback for pool instances
/// that predate the admin-configurable minimum. Prevents share-price griefing
/// by requiring the initial deposit in an empty pool to be at least this
/// floor.
///
/// This value assumes 7-decimal stroops (1 unit = 10_000_000 stroops), which
/// only holds for the pool's originally supported asset. Under the factory
/// model each instance funds a different asset, so a deploy for an asset with
/// different decimals should pass its own `min_initial_deposit` to
/// `initialize` rather than rely on this default.
pub const DEFAULT_MIN_INITIAL_DEPOSIT: u128 = 10_000_000;

/// Maximum protocol fee in basis points (2000 bps = 20%).
/// Prevents excessive fee extraction by capping the protocol cut at 20% of yield spread,
/// mirroring the bounds-check pattern used by `list_for_financing`'s discount cap.
pub const MAX_PROTOCOL_FEE_BPS: u32 = 2000;

/// Maximum number of entries accepted by `batch_fund_invoice`.
///
/// Mirrors the 50-entry cap that `RegistryContract::batch_register_issuers` and
/// `InvoiceContract::batch_create` use. The cap bounds the worst-case cost of a
/// single batch: every entry performs the same cross-contract reads as
/// `fund_invoice`, so an unbounded `Vec` would let a caller blow the
/// per-transaction budget that `docs/LIMITATIONS.md` already flags as the
/// protocol's most expensive operation.
pub const MAX_BATCH_SIZE: u32 = 50;

/// Floor of the suggested-discount curve returned by
/// `get_suggested_discount_bps`: the rate suggested while the pool is idle
/// (0% utilization). 100 bps = 1%, i.e. an issuer is always quoted a
/// non-zero haircut so capital is not deployed for free.
pub const SUGGESTED_DISCOUNT_FLOOR_BPS: u32 = 100;

/// Ceiling of the suggested-discount curve: the rate suggested at 100%
/// utilization. Deliberately equal to the hard 5000 bps cap that
/// `invoice::list_for_financing` enforces via `InvoiceError::DiscountTooHigh`,
/// so a suggestion can never be a value the invoice contract would reject.
pub const SUGGESTED_DISCOUNT_CEILING_BPS: u32 = 5_000;

/// Default decimal places reported by an LP share token's SEP-41 `decimals()`,
/// used as the fallback for pool instances that predate `initialize` taking a
/// `share_decimals` argument.
///
/// Shares are issued against the funding asset's base units — 1 USDC is
/// `DEFAULT_MIN_INITIAL_DEPOSIT` = 10_000_000 stroops — so the fallback matches
/// the 7 decimals of the USDC pools that existed before the factory model. Like
/// `DEFAULT_MIN_INITIAL_DEPOSIT`, this only holds for that originally supported
/// asset: a pool funding something with different decimals must pass its own
/// `share_decimals`, otherwise wallets render its share balances at the wrong
/// precision.
pub const DEFAULT_SHARE_DECIMALS: u32 = 7;
