//! Pool contract data types.
//!
//! # Basis points (bps)
//!
//! Ratios in this module use basis-point (bps) scaling, where `10_000` bps
//! equals `100%` and `1` bp equals `0.01%`. For example, a
//! `utilization_rate_bps` value of `7500` represents a utilization of `75%`.

use soroban_sdk::{contracttype, Address, BytesN};

/// Aggregate, pool-wide accounting snapshot returned by `get_pool_stats`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct PoolStats {
    /// Total USDC principal currently held by the pool, in stroops
    /// (1 USDC = 10_000_000 stroops). Grows when LPs deposit or when yield
    /// is distributed back into the pool, and shrinks on LP withdrawals and
    /// invoice funding.
    pub total_deposits: u128,
    /// Total USDC (in stroops) currently deployed to fund outstanding
    /// invoices. Increases on `fund_invoice` and decreases on repayment.
    pub total_funded: u128,
    /// USDC (in stroops) available for new invoice funding or LP
    /// withdrawals. Equal to `total_deposits - total_funded`.
    pub available_liquidity: u128,
    /// Current pool utilization, expressed in basis points
    /// (`0` = 0%, `10_000` = 100%). Computed as
    /// `total_funded * 10_000 / total_deposits`.
    pub utilization_rate_bps: u32,
    /// Cumulative USDC (in stroops) of yield that has been distributed to
    /// the pool from repaid invoices over the pool's lifetime.
    pub total_yield_distributed: u128,
    /// Cumulative USDC principal (in stroops) written off when funded
    /// invoices default. This is lifetime accounting and is not reduced by
    /// later deposits.
    pub total_loss_realised: u128,
    /// Number of invoices currently funded and awaiting repayment.
    pub active_invoice_count: u32,
    /// Total supply of LP shares outstanding. Individual LP ownership of
    /// the pool is `lp_shares / total_shares`.
    pub total_shares: u128,
    /// Maximum utilization the pool will allow before rejecting new
    /// invoice funding, in basis points (see module docs).
    pub max_utilization_bps: u32,
}

/// Per-LP position snapshot returned by `get_lp_position`.
#[contracttype]
#[derive(Clone, Debug)]
pub struct LPPosition {
    /// LP share balance owned by this liquidity provider. Ownership of the
    /// pool is `shares / PoolStats::total_shares`.
    pub shares: u128,
    /// Current redemption value of `shares` in USDC stroops, computed as
    /// `shares * total_deposits / total_shares` at query time. Includes
    /// principal plus the LP's proportional share of undistributed yield.
    pub usdc_value: u128,
    /// Cumulative USDC yield (in stroops) realised by this LP across all
    /// prior withdrawals. Only updated on withdraw, when the redeemed
    /// amount exceeds the LP's tracked principal portion; unrealised yield
    /// still sitting in `usdc_value` is not counted here.
    pub yield_earned: u128,
    /// Number of successful deposits this LP has made into the pool.
    pub deposit_count: u32,
}

/// SEP-41 share allowance granted by an LP to a spender.
#[contracttype]
#[derive(Clone, Debug)]
pub struct ShareAllowance {
    /// Number of shares the spender may still move, stored as `i128` because
    /// SEP-41's `approve`/`allowance` interface is signed. Never negative.
    pub amount: i128,
    /// Last ledger sequence at which this grant is still live. Reads at or
    /// above this sequence see `amount`; later sequences see `0`.
    pub expiration_ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum DataKey {
    Admin,
    InvoiceContract,
    EscrowContract,
    FundingAsset,
    TotalShares,
    TotalDeposits,
    TotalFunded,
    TotalYieldDistributed,
    TotalLossRealised,
    ActiveInvoiceCount,
    LPShares(Address),
    LPDepositCount(Address),
    LPYieldEarned(Address),
    LPInitialDeposit(Address),
    FundedInvoice(BytesN<32>),
    MaxUtilizationBps,
    FeeBps,
    TreasuryAddress,
    // RegistryContract intentionally last to avoid changing enum discriminants
    // for already-deployed contract storage keys. New variants must keep
    // being appended after it, in the same spirit, rather than inserted
    // earlier.
    RegistryContract,
    /// Stored protocol fee basis points (defaults to 0 bps).
    ProtocolFeeBps,
    /// Admin-configured minimum initial deposit floor for this instance, set
    /// at `initialize` time. See `DEFAULT_MIN_INITIAL_DEPOSIT` for the
    /// fallback used by pre-migration instances.
    MinInitialDeposit,
    /// SEP-41 allowance from one LP to a spender, holding a `ShareAllowance`.
    /// Persistent (not instance) storage: an allowance is per-address-pair
    /// state like `LPShares`, not config that belongs on the instance
    /// footprint.
    Allowance(Address, Address),
    /// SEP-41 `name()` of this instance's share token, written at `initialize`.
    ShareName,
    /// SEP-41 `symbol()` of this instance's share token, written at `initialize`.
    ShareSymbol,
    /// SEP-41 `decimals()` of this instance's share token, written at
    /// `initialize`. Per-instance rather than a constant because
    /// `docs/SEP41_DESIGN.md` requires shares to carry the funding asset's
    /// decimals, and under the factory model each instance has its own.
    ShareDecimals,
}
