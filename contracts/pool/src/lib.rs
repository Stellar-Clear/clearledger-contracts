#![no_std]

use clearledger_pause::{require_not_paused, set_paused};
use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, BytesN, Env, IntoVal, String, Symbol,
    Vec,
};

mod constants;
mod errors;
mod events;
mod test;
mod types;

pub use constants::*;

pub use errors::*;
pub use types::*;

/// Default maximum utilization cap (in basis points) written at
/// `initialize()` time. 8500 bps = 85%. This is the single source of truth for
/// the default: `totals()`'s fallback reads the same constant, so the two call
/// sites can never silently desync if the default is ever changed.
pub const DEFAULT_MAX_UTILIZATION_BPS: u32 = 8500;

#[contract]
pub struct PoolContract;

#[derive(Clone, Copy)]
struct PoolTotals {
    shares: u128,
    deposits: u128,
    funded: u128,
    yield_distributed: u128,
    loss_realised: u128,
    active_invoices: u32,
    max_utilization_bps: u32,
}

#[contractimpl]
impl PoolContract {
    /// Initializes the pool contract with admin and external contract references.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The admin address for this contract.
    /// * `invoice_contract` - The invoice contract address.
    /// * `escrow_contract` - The escrow contract address.
    /// * `funding_asset` - The asset this pool instance funds invoices with.
    /// * `registry_contract` - The registry contract address, consulted by
    ///   `fund_invoice` to re-verify the issuer and buyer are still verified
    ///   before pool capital is committed.
    /// * `treasury` - The treasury address receiving protocol fee cuts (may equal admin initially).
    /// * `min_initial_deposit` - The minimum first deposit an empty pool will
    ///   accept, in `funding_asset` stroops. Callers funding an asset with
    ///   different decimals than the original USDC-only pool must scale this
    ///   accordingly rather than reuse `DEFAULT_MIN_INITIAL_DEPOSIT`.
    /// * `share_name` - SEP-41 `name()` of this pool's LP share token, e.g.
    ///   `"ClearLedger USDC Pool Shares"`. Must be non-empty.
    /// * `share_symbol` - SEP-41 `symbol()` of this pool's LP share token, e.g.
    ///   `"TT-USDC"`. Must be non-empty.
    /// * `share_decimals` - SEP-41 `decimals()` of this pool's LP share token.
    ///   Per `docs/SEP41_DESIGN.md` this must match `funding_asset`'s own
    ///   decimals, since shares are issued in the funding asset's base units.
    ///
    /// Share metadata is configured per instance, alongside `min_initial_deposit`
    /// and for the same reason: under the `pool_factory` model each pool funds a
    /// different asset, so a deploy for an asset with other than 7 decimals
    /// supplies its own name, symbol and precision. Fixing it at
    /// `initialize` also means a wallet never sees an LP's share balance
    /// silently re-labelled under a different symbol.
    ///
    /// Protocol fee storage (`DataKey::ProtocolFeeBps`) is explicitly initialized to 0 bps
    /// and `DataKey::TreasuryAddress` is initialized to the provided `treasury` address.
    ///
    /// # Auth
    /// Requires authorization from `admin`.
    ///
    /// # Wiring order
    /// `escrow_contract` must already be initialized before this call, since
    /// `initialize` cross-checks `escrow_contract.get_usdc_asset()` against
    /// its own `funding_asset` to catch a misconfigured deploy where escrow was
    /// wired up with a different token.
    ///
    /// # Panics
    /// * `AlreadyInitialized` if the contract has already been initialized.
    /// * `InvalidConfiguration` if core contract addresses collide, if
    ///   `treasury` equals the pool itself or any settlement contract address,
    ///   or if `share_name`/`share_symbol` is empty (a wallet that cannot render
    ///   the share token is treated as a misconfigured deploy).
    /// * `EscrowAssetMismatch` if `escrow_contract`'s configured USDC asset
    ///   does not match `funding_asset`.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// escrow_client.initialize(&admin, &pool, &invoice, &usdc); // escrow first
    /// client.initialize(&admin, &invoice, &escrow, &usdc, &registry, &admin,
    ///     &min_deposit, &share_name, &share_symbol, &share_decimals);
    /// ```
    // One-shot wiring for a pool instance: six contract/admin references, the
    // treasury, the deposit floor and the three SEP-41 share-metadata values.
    // Bundling the metadata into a struct would clear the argument-count lint
    // but would make the initializer's wiring less explicit at call sites.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        env: Env,
        admin: Address,
        invoice_contract: Address,
        escrow_contract: Address,
        funding_asset: Address,
        registry_contract: Address,
        treasury: Address,
        min_initial_deposit: u128,
        share_name: String,
        share_symbol: String,
        share_decimals: u32,
    ) {
        require_not_paused(&env);
        if Self::admin(&env).is_some() {
            panic_with_error!(&env, PoolError::AlreadyInitialized);
        }
        if admin == invoice_contract
            || admin == escrow_contract
            || admin == funding_asset
            || admin == registry_contract
            || invoice_contract == escrow_contract
            || invoice_contract == funding_asset
            || invoice_contract == registry_contract
            || escrow_contract == funding_asset
            || escrow_contract == registry_contract
            || funding_asset == registry_contract
        {
            panic_with_error!(&env, PoolError::InvalidConfiguration);
        }
        if share_name.is_empty() || share_symbol.is_empty() {
            panic_with_error!(&env, PoolError::InvalidConfiguration);
        }
        Self::assert_valid_treasury(
            &env,
            &treasury,
            &invoice_contract,
            &escrow_contract,
            &funding_asset,
            &registry_contract,
        );

        // Cross-check that the escrow contract being wired in was itself
        // initialized with the same funding_asset. A mismatch here would only
        // otherwise surface later as a failed token transfer inside
        // fund_invoice's escrow.lock call, since escrow.lock pulls funds
        // using escrow's own configured token client. This requires
        // escrow_contract to already be initialized at the time pool.initialize
        // is called.
        let args = Vec::new(&env);
        let escrow_funding_asset: Address =
            env.invoke_contract(&escrow_contract, &Symbol::new(&env, "get_usdc_asset"), args);
        if escrow_funding_asset != funding_asset {
            panic_with_error!(&env, PoolError::EscrowAssetMismatch);
        }

        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::InvoiceContract, &invoice_contract);
        env.storage()
            .instance()
            .set(&DataKey::EscrowContract, &escrow_contract);
        env.storage()
            .instance()
            .set(&DataKey::FundingAsset, &funding_asset);
        env.storage()
            .instance()
            .set(&DataKey::RegistryContract, &registry_contract);
        env.storage().instance().set(&DataKey::TotalShares, &0u128);
        env.storage()
            .instance()
            .set(&DataKey::TotalDeposits, &0u128);
        env.storage().instance().set(&DataKey::TotalFunded, &0u128);
        env.storage()
            .instance()
            .set(&DataKey::TotalYieldDistributed, &0u128);
        env.storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::MaxUtilizationBps, &DEFAULT_MAX_UTILIZATION_BPS);
        env.storage().instance().set(&DataKey::FeeBps, &0u32); // Default to 0% fee
        env.storage()
            .instance()
            .set(&DataKey::TotalLossRealised, &0u128);
        // Explicitly set DataKey::ProtocolFeeBps to 0 and DataKey::TreasuryAddress to treasury
        env.storage()
            .instance()
            .set(&DataKey::ProtocolFeeBps, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::TreasuryAddress, &treasury);
        env.storage()
            .instance()
            .set(&DataKey::MinInitialDeposit, &min_initial_deposit);
        env.storage()
            .instance()
            .set(&DataKey::ShareName, &share_name);
        env.storage()
            .instance()
            .set(&DataKey::ShareSymbol, &share_symbol);
        env.storage()
            .instance()
            .set(&DataKey::ShareDecimals, &share_decimals);
        Self::extend_instance_ttl(&env);

        events::pool_initialized(
            &env,
            &admin,
            &invoice_contract,
            &escrow_contract,
            &funding_asset,
        );
    }

    /// Returns the funding asset used by the pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * Panics if the contract has not been initialized (missing `FundingAsset`).
    ///
    /// # Returns
    /// * `Address` - The funding asset address.
    ///
    /// # Example
    /// ```ignore
    /// let asset = client.get_funding_asset();
    /// ```
    pub fn get_funding_asset(env: Env) -> Address {
        Self::funding_asset(&env)
    }

    /// Deprecated alias for [`Self::get_funding_asset`], kept so integrators
    /// built against the pre-factory USDC-only naming keep working. New
    /// callers should use `get_funding_asset` instead.
    pub fn get_usdc_asset(env: Env) -> Address {
        Self::funding_asset(&env)
    }

    /// Returns the decimal precision wallets use to render LP share balances
    /// (SEP-41 `decimals`).
    ///
    /// Shares are issued in the funding asset's base units, so
    /// `docs/SEP41_DESIGN.md` requires this to match the funding asset's own
    /// decimals; `initialize` therefore takes a `share_decimals` per instance.
    /// The `DEFAULT_SHARE_DECIMALS` fallback covers instances that predate it,
    /// which were all USDC pools.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Returns
    /// * `u32` - This instance's share decimals, or 7 if never configured.
    ///
    /// # Example
    /// ```ignore
    /// let decimals = client.decimals();
    /// ```
    pub fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::ShareDecimals)
            .unwrap_or(DEFAULT_SHARE_DECIMALS)
    }

    /// Returns this pool's LP share token name (SEP-41 `name`), as configured
    /// in `initialize`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `NotInitialized` if `initialize` has not stored the name yet.
    ///
    /// # Returns
    /// * `String` - The share token name.
    ///
    /// # Example
    /// ```ignore
    /// let name = client.name();
    /// ```
    pub fn name(env: Env) -> String {
        env.storage()
            .instance()
            .get(&DataKey::ShareName)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Returns this pool's LP share token symbol (SEP-41 `symbol`), as
    /// configured in `initialize`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `NotInitialized` if `initialize` has not stored the symbol yet.
    ///
    /// # Returns
    /// * `String` - The share token symbol.
    ///
    /// # Example
    /// ```ignore
    /// let symbol = client.symbol();
    /// ```
    pub fn symbol(env: Env) -> String {
        env.storage()
            .instance()
            .get(&DataKey::ShareSymbol)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Returns the admin address for the pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `PoolError::NotInitialized` if the contract has not been initialized
    ///   (missing `Admin`).
    ///
    /// # Returns
    /// * `Address` - The admin address.
    ///
    /// # Example
    /// ```ignore
    /// let admin = client.get_admin();
    /// ```
    pub fn get_admin(env: Env) -> Address {
        Self::admin(&env).unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Transfers pool admin ownership to `new_admin`.
    ///
    /// Uses the dual-authorization pattern shared with `RegistryContract` and
    /// `InvoiceContract`: both the current admin and the incoming `new_admin`
    /// must sign. This stops ownership from being handed to an address that has
    /// not consented (e.g. a mistyped address) and stops a compromised current
    /// admin from unilaterally installing a key it controls. Emits
    /// `ownership_transferred`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `new_admin` - The address that will become the pool admin.
    ///
    /// # Auth
    /// Requires authorization from both the current admin and `new_admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool has not been initialized.
    /// * `ContractPaused` (via `clearledger_pause::require_not_paused`) if the
    ///   pool is paused.
    ///
    /// # Example
    /// ```ignore
    /// client.transfer_ownership(&new_admin);
    /// ```
    pub fn transfer_ownership(env: Env, new_admin: Address) {
        require_not_paused(&env);
        let admin =
            Self::admin(&env).unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        admin.require_auth();
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::ownership_transferred(&env, &admin, &new_admin);
        Self::extend_instance_ttl(&env);
    }

    /// Engages the emergency circuit breaker.
    ///
    /// While paused every state-changing entry point reverts with
    /// `ContractPaused`, while read-only views (`get_stats`, `balance`,
    /// `get_lp_position`, ...) stay callable. Only the stored admin may pause,
    /// and [`Self::unpause`] is intentionally never guarded so a paused pool can
    /// always be resumed. Emits `paused`.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool has not been initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.pause();
    /// ```
    pub fn pause(env: Env) {
        let admin =
            Self::admin(&env).unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        admin.require_auth();
        set_paused(&env, true);
        events::paused(&env, &admin);
        Self::extend_instance_ttl(&env);
    }

    /// Disengages the emergency circuit breaker, restoring state-changing calls.
    ///
    /// Deliberately *not* guarded by `require_not_paused`: otherwise a paused
    /// pool could never be resumed. Emits `unpaused`.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool has not been initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.unpause();
    /// ```
    pub fn unpause(env: Env) {
        let admin =
            Self::admin(&env).unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        admin.require_auth();
        set_paused(&env, false);
        events::unpaused(&env, &admin);
        Self::extend_instance_ttl(&env);
    }

    /// Returns the protocol fee in basis points.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `PoolError::NotInitialized` if the contract has not been initialized
    ///   (missing `FeeBps`).
    ///
    /// # Returns
    /// * `u32` - The protocol fee in basis points.
    ///
    /// # Example
    /// ```ignore
    /// let fee_bps = client.get_fee_bps();
    /// ```
    pub fn get_fee_bps(env: Env) -> u32 {
        Self::fee_bps(&env).unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Returns the treasury address for protocol fee distribution.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `PoolError::NotInitialized` if the contract has not been initialized
    ///   (missing `TreasuryAddress`).
    ///
    /// # Returns
    /// * `Address` - The treasury address.
    ///
    /// # Example
    /// ```ignore
    /// let treasury = client.get_treasury_address();
    /// ```
    pub fn get_treasury_address(env: Env) -> Address {
        Self::treasury_address(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Returns the invoice contract address configured for the pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `PoolError::NotInitialized` if the contract has not been initialized
    ///   (missing `InvoiceContract`).
    ///
    /// # Returns
    /// * `Address` - The invoice contract address.
    ///
    /// # Example
    /// ```ignore
    /// let invoice = client.get_invoice_contract();
    /// ```
    pub fn get_invoice_contract(env: Env) -> Address {
        Self::invoice_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Returns the escrow contract address configured for the pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `PoolError::NotInitialized` if the contract has not been initialized
    ///   (missing `EscrowContract`).
    ///
    /// # Returns
    /// * `Address` - The escrow contract address.
    ///
    /// # Example
    /// ```ignore
    /// let escrow = client.get_escrow_contract();
    /// ```
    pub fn get_escrow_contract(env: Env) -> Address {
        Self::escrow_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
    }

    /// Deposits USDC from an LP and issues pool shares.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `lp` - The liquidity provider address.
    /// * `usdc_amount` - The amount of USDC to deposit.
    ///
    /// # Auth
    /// Requires self-authorization from `lp` (via `lp.require_auth()`).
    ///
    /// # Panics
    /// * `InvalidAmount` if `usdc_amount` is zero or if initial deposit is below this instance's configured minimum (see `initialize`'s `min_initial_deposit`).
    /// * `MinimumDeposit` if the deposit is too small to mint at least 1 share
    ///   at the current share price (prevents 0-share dust deposits).
    /// * `Overflow` if `usdc_amount * total_shares` would overflow `u128`
    ///   while computing the proportional share price.
    ///
    /// # Returns
    /// * `u128` - The number of shares issued.
    ///
    /// # Example
    /// ```ignore
    /// let shares = client.deposit(&lp, 10_000_000);
    /// ```
    pub fn deposit(env: Env, lp: Address, usdc_amount: u128) -> u128 {
        require_not_paused(&env);
        Self::require_initialized(&env);
        lp.require_auth();
        let shares_to_issue = Self::shares_for_deposit(&env, usdc_amount);
        let total_deposits = Self::totals(&env).deposits;

        let usdc_id = Self::funding_asset(&env);
        let usdc = token::Client::new(&env, &usdc_id);
        usdc.transfer(&lp, &env.current_contract_address(), &(usdc_amount as i128));

        Self::_mint(&env, &lp, shares_to_issue);
        env.storage()
            .instance()
            .set(&DataKey::TotalDeposits, &(total_deposits + usdc_amount));

        let lp_deposit_count_key = DataKey::LPDepositCount(lp.clone());
        let count: u32 = env
            .storage()
            .persistent()
            .get(&lp_deposit_count_key)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&lp_deposit_count_key, &(count + 1));
        env.storage()
            .persistent()
            .extend_ttl(&lp_deposit_count_key, TTL_THRESHOLD, TTL_EXTEND_TO);

        let lp_init_key = DataKey::LPInitialDeposit(lp.clone());
        let init_dep: u128 = env.storage().persistent().get(&lp_init_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&lp_init_key, &(init_dep + usdc_amount));
        env.storage()
            .persistent()
            .extend_ttl(&lp_init_key, TTL_THRESHOLD, TTL_EXTEND_TO);

        events::lp_deposited(&env, &lp, usdc_amount, shares_to_issue);
        shares_to_issue
    }

    /// Previews the number of pool shares a deposit would mint.
    ///
    /// Uses the same proportional calculation and rounding as `deposit`, and
    /// only reads pool state. No authorization is required.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `usdc_amount` - The amount of USDC to deposit.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `InvalidAmount` if `usdc_amount` is zero or is below the configured
    ///   minimum for an initial deposit.
    /// * `MinimumDeposit` if the amount rounds down to zero shares.
    /// * `Overflow` if `usdc_amount * total_shares` overflows `u128`.
    ///
    /// # Returns
    /// * `u128` - The number of shares `deposit` would issue.
    ///
    /// # Example
    /// ```ignore
    /// let shares = client.preview_deposit(10_000_000);
    /// ```
    pub fn preview_deposit(env: Env, usdc_amount: u128) -> u128 {
        Self::require_initialized(&env);
        Self::shares_for_deposit(&env, usdc_amount)
    }

    /// Previews the USDC a withdrawal of `shares` would return.
    ///
    /// Uses the same floor-rounded conversion as `withdraw`
    /// (`shares * total_deposits / total_shares`) and only reads pool state.
    /// The returned `WithdrawPreview` also reports the pool's currently
    /// available liquidity and whether it covers the quoted output, so a
    /// caller can surface an insufficient-liquidity condition without a
    /// reverted transaction.
    ///
    /// Unlike `withdraw`, this method neither requires nor checks an LP
    /// address: it quotes the pool's share price and liquidity, while the LP's
    /// own share balance remains available from `balance`/`get_lp_position`.
    /// It also does not check pause state, so callers should combine the quote
    /// with those checks before submitting `withdraw`. It is read-only — no
    /// authorization, storage writes, events, or TTL extensions occur.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `shares` - The number of shares to quote a redemption for.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `InvalidAmount` if `shares` is zero.
    /// * `NoShares` if no shares are outstanding.
    /// * `MinimumDeposit` if the computed USDC redemption rounds down to zero.
    /// * `Overflow` if `shares * total_deposits` would overflow `u128`.
    ///
    /// # Returns
    /// * `WithdrawPreview` - The quoted USDC output, available liquidity, and
    ///   whether that liquidity is sufficient.
    ///
    /// # Example
    /// ```ignore
    /// let preview = client.preview_withdraw(500);
    /// if preview.liquidity_ok {
    ///     // preview.usdc_out is what withdraw(&lp, 500) would return.
    /// }
    /// ```
    pub fn preview_withdraw(env: Env, shares: u128) -> WithdrawPreview {
        Self::require_initialized(&env);

        let totals = Self::totals(&env);
        let available_liquidity = totals.deposits - totals.funded;
        let usdc_out = Self::usdc_for_withdraw(&env, shares, totals.shares, totals.deposits);

        WithdrawPreview {
            usdc_out,
            available_liquidity,
            liquidity_ok: usdc_out <= available_liquidity,
        }
    }

    /// Withdraws shares from the pool and transfers USDC to the LP.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `lp` - The liquidity provider address.
    /// * `shares` - The number of shares to withdraw.
    ///
    /// # Auth
    /// Requires self-authorization from `lp` (via `lp.require_auth()`).
    ///
    /// # Panics
    /// * `InvalidAmount` if `shares` is zero.
    /// * `NoShares` if the LP has no shares.
    /// * `InsufficientShares` if the LP does not own enough shares.
    /// * `MinimumDeposit` if the computed USDC redemption rounds down to zero
    ///   (dust-guard: prevents burning shares for nothing when the share price
    ///   is very high relative to the number of shares redeemed).
    /// * `InsufficientLiquidity` if the pool lacks enough available USDC.
    /// * `Overflow` if `shares * total_deposits` (or `shares * lp_initial_deposit`)
    ///   would overflow `u128` while computing the redemption amount.
    ///
    /// # Notes
    /// Principal is tracked as one aggregate amount per LP, not as per-deposit
    /// lots. A withdrawal allocates that principal pro rata across the LP's
    /// shares, so its yield attribution can differ from FIFO or HIFO lot
    /// accounting after deposits at different share prices. On full withdrawal
    /// (remaining shares reach zero), `LPInitialDeposit` and `LPDepositCount`
    /// are removed so a subsequent deposit starts a fresh accounting cycle.
    ///
    /// # Returns
    /// * `u128` - The amount of USDC returned.
    ///
    /// # Example
    /// ```ignore
    /// let returned = client.withdraw(&lp, 500);
    /// ```
    pub fn withdraw(env: Env, lp: Address, shares: u128) -> u128 {
        require_not_paused(&env);
        Self::require_initialized(&env);
        lp.require_auth();
        if shares == 0 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }

        let lp_shares_key = DataKey::LPShares(lp.clone());
        let lp_shares: u128 = env
            .storage()
            .persistent()
            .get(&lp_shares_key)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NoShares));
        if shares > lp_shares {
            panic_with_error!(&env, PoolError::InsufficientShares);
        }

        let totals = Self::totals(&env);
        let total_shares = totals.shares;
        let total_deposits = totals.deposits;
        let total_funded = totals.funded;
        let available = total_deposits - total_funded;

        let usdc_to_return = Self::usdc_for_withdraw(&env, shares, total_shares, total_deposits);
        if usdc_to_return > available {
            panic_with_error!(&env, PoolError::InsufficientLiquidity);
        }

        let usdc_id = Self::funding_asset(&env);
        let usdc = token::Client::new(&env, &usdc_id);
        usdc.transfer(
            &env.current_contract_address(),
            &lp,
            &(usdc_to_return as i128),
        );

        let remaining_shares = Self::_burn(&env, &lp, shares);
        env.storage()
            .instance()
            .set(&DataKey::TotalDeposits, &(total_deposits - usdc_to_return));

        if remaining_shares == 0 {
            // Full withdrawal: reset LP-scoped storage to prevent stale state
            // on re-deposit. LPInitialDeposit is zeroed below via the
            // principal_portion calculation; LPDepositCount must be removed too.
            let dep_count_key = DataKey::LPDepositCount(lp.clone());
            env.storage().persistent().remove(&dep_count_key);
        }

        let init_dep_key = DataKey::LPInitialDeposit(lp.clone());
        let init_dep: u128 = env.storage().persistent().get(&init_dep_key).unwrap_or(0);
        let principal_scaled = shares
            .checked_mul(init_dep)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::Overflow));
        let principal_portion = principal_scaled / (lp_shares);
        let yield_earned = usdc_to_return.saturating_sub(principal_portion);

        let new_init_dep = init_dep.saturating_sub(principal_portion);
        if new_init_dep > 0 {
            env.storage().persistent().set(&init_dep_key, &new_init_dep);
            env.storage()
                .persistent()
                .extend_ttl(&init_dep_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        } else {
            env.storage().persistent().remove(&init_dep_key);
        }

        let yield_key = DataKey::LPYieldEarned(lp.clone());
        let prev_yield: u128 = env.storage().persistent().get(&yield_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&yield_key, &(prev_yield + yield_earned));
        env.storage()
            .persistent()
            .extend_ttl(&yield_key, TTL_THRESHOLD, TTL_EXTEND_TO);

        events::lp_withdrawn(&env, &lp, usdc_to_return, shares);
        usdc_to_return
    }

    /// Moves `amount` LP shares from `from` to `to` — the standard SEP-41
    /// `transfer` entry point.
    ///
    /// This is the function generic `soroban_sdk::token::Client` integrations
    /// call. It shares one balance-movement path with `transfer_from`
    /// (`Self::move_shares`), so authorization, balance checks, and the
    /// `shares_transferred` event behave identically on both routes. A
    /// previous attempt introduced two competing `transfer` definitions (the
    /// SEP-41 entry point and `transfer_shares` logic under the same name);
    /// this single entry point plus the shared `move_shares` helper is the
    /// duplicate-free resolution.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `from` - The address transferring shares (must authorize).
    /// * `to` - The address receiving shares.
    /// * `amount` - The number of shares to move.
    ///
    /// # Auth
    /// Requires authorization from `from` (via `from.require_auth()`).
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `InvalidAmount` if `amount` is zero or negative.
    /// * `NoShares` if `from` has no shares.
    /// * `InsufficientBalance` if `from` does not own enough shares.
    ///
    /// # Returns
    /// * `()` - No value is returned. Emits `shares_transferred`.
    ///
    /// # Example
    /// ```ignore
    /// let shares_token = soroban_sdk::token::Client::new(&env, &pool_id);
    /// shares_token.transfer(&lp, &recipient, &5_000_000_000);
    /// ```
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        require_not_paused(&env);
        Self::require_initialized(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }

        Self::move_shares(&env, &from, &to, amount as u128);
        events::shares_transferred(&env, &from, &to, amount);
        Self::extend_instance_ttl(&env);
    }

    /// Transfers LP shares from one address to another.
    ///
    /// # Non-standard
    /// This is a **legacy, non-standard** entry point kept for existing
    /// integrators that predate the SEP-41 share-token surface. It performs
    /// exactly the same checks and movement as the standard
    /// [`Self::transfer`] (same `from.require_auth()`, same `move_shares`
    /// path, same events) but takes `amount: i128` purely for historical
    /// call-signature compatibility. New integrations — especially generic
    /// `soroban_sdk::token::Client` consumers — must use [`Self::transfer`].
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `from` - The address transferring shares (must authorize).
    /// * `to` - The address receiving shares.
    /// * `amount` - The number of shares to transfer.
    ///
    /// # Auth
    /// Requires authorization from `from` (via `from.require_auth()`).
    ///
    /// # Panics
    /// * `InvalidAmount` if `amount` is zero.
    /// * `NoShares` if `from` has no shares.
    /// Transfers shares from one address to another.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `from` - The source address.
    /// * `to` - The destination address.
    /// * `amount` - The amount to transfer (can be negative for reverse accounting in specific contexts).
    ///
    /// # Auth
    /// Requires authorization from `from`.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `InvalidAmount` if `amount` is zero or negative.
    /// * `NoShares` if `from` has no shares.
    /// * `InsufficientBalance` if `from` does not own enough shares.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.transfer_shares(&from, &to, 100);
    /// ```
    ///
    /// Note: This function accepts i128 to allow for negative amounts in internal accounting,
    /// but negative amounts are rejected as invalid for standard transfers.
    pub fn transfer_shares(env: Env, from: Address, to: Address, amount: i128) {
        require_not_paused(&env);
        Self::require_initialized(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }

        Self::move_shares(&env, &from, &to, amount as u128);
        events::shares_transferred(&env, &from, &to, amount);
        Self::extend_instance_ttl(&env);
    }

    /// Grants `spender` permission to move up to `amount` of `from`'s shares
    /// until ledger sequence `expiration_ledger`.
    ///
    /// Approving again overwrites the previous grant rather than adding to it
    /// (SEP-41 semantics), and `amount = 0` clears it, which is the standard
    /// way to revoke a spend authorization.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `from` - The share owner granting the allowance (must authorize).
    /// * `spender` - The address permitted to move the shares.
    /// * `amount` - The maximum number of shares `spender` may move.
    /// * `expiration_ledger` - Last ledger sequence at which the grant is live.
    ///
    /// # Auth
    /// Requires authorization from `from` (via `from.require_auth()`).
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `InvalidAmount` if `amount` is negative.
    /// * `InvalidExpiration` if `amount` is non-zero and `expiration_ledger` is
    ///   at or before the current ledger sequence (such a grant could never be
    ///   spent, so approving with it is treated as a caller error).
    ///
    /// # Returns
    /// * `()` - No value is returned. Emits `allowance_approved`.
    ///
    /// # Example
    /// ```ignore
    /// // Let a staking contract move up to 5 shares through ledger 1_000.
    /// client.approve(&lp, &staking, &5, &1000);
    /// ```
    pub fn approve(
        env: Env,
        from: Address,
        spender: Address,
        amount: i128,
        expiration_ledger: u32,
    ) {
        require_not_paused(&env);
        Self::require_initialized(&env);
        from.require_auth();
        if amount < 0 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }
        if amount > 0 && expiration_ledger <= env.ledger().sequence() {
            panic_with_error!(&env, PoolError::InvalidExpiration);
        }

        let key = DataKey::Allowance(from.clone(), spender.clone());
        if amount == 0 {
            // Revoke: drop the entry entirely so its ledger rent is reclaimed.
            env.storage().persistent().remove(&key);
        } else {
            env.storage().persistent().set(
                &key,
                &ShareAllowance {
                    amount,
                    expiration_ledger,
                },
            );
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        }

        events::allowance_approved(&env, &from, &spender, amount, expiration_ledger);
        Self::extend_instance_ttl(&env);
    }

    /// Returns the number of `from`'s shares `spender` may still move.
    ///
    /// Reads `0` both when no grant exists and when the grant's
    /// `expiration_ledger` has passed, so callers never have to distinguish an
    /// unset allowance from a dead one.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `from` - The share owner.
    /// * `spender` - The address the grant was made to.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Returns
    /// * `i128` - The remaining allowance, or `0`.
    ///
    /// # Example
    /// ```ignore
    /// let remaining = client.allowance(&lp, &staking);
    /// ```
    pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
        match Self::live_allowance(&env, &from, &spender) {
            Some(record) => record.amount,
            None => 0,
        }
    }

    /// Moves `amount` of `from`'s shares to `to` using the allowance `from`
    /// granted to `spender`, then decrements that allowance by `amount`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `spender` - The address holding the grant (must authorize).
    /// * `from` - The share owner whose balance is debited.
    /// * `to` - The address receiving the shares.
    /// * `amount` - The number of shares to move.
    ///
    /// # Auth
    /// Requires authorization from `spender` (via `spender.require_auth()`) —
    /// `from` already authorized by calling `approve`.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `InvalidAmount` if `amount` is zero or negative.
    /// * `InsufficientAllowance` if the remaining (non-expired) allowance is
    ///   less than `amount`.
    /// * `NoShares` if `from` has no shares.
    /// * `InsufficientBalance` if `from` does not own enough shares.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.approve(&lp, &staking, &5, &1000);
    /// client.transfer_from(&staking, &lp, &treasury, &5);
    /// // allowance(lp, staking) is now 0
    /// ```
    pub fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        require_not_paused(&env);
        Self::require_initialized(&env);
        spender.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }

        let record = Self::live_allowance(&env, &from, &spender)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::InsufficientAllowance));
        if record.amount < amount {
            panic_with_error!(&env, PoolError::InsufficientAllowance);
        }

        let key = DataKey::Allowance(from.clone(), spender.clone());
        let remaining = record.amount - amount;
        if remaining > 0 {
            env.storage().persistent().set(
                &key,
                &ShareAllowance {
                    amount: remaining,
                    expiration_ledger: record.expiration_ledger,
                },
            );
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        } else {
            env.storage().persistent().remove(&key);
        }

        Self::move_shares(&env, &from, &to, amount as u128);
        events::shares_transferred(&env, &from, &to, amount);
        Self::extend_instance_ttl(&env);
    }

    /// Shared share-movement path for `transfer` and `transfer_from`: debits
    /// `from`'s `LPShares` balance and credits `to`'s.
    ///
    /// The grant bookkeeping and the two `require_auth` sites stay with the
    /// public entry points; this helper owns only balance checks and writes, so
    /// both paths enforce the same `NoShares`/`InsufficientBalance` rules.
    fn move_shares(env: &Env, from: &Address, to: &Address, amount: u128) {
        let from_shares_key = DataKey::LPShares(from.clone());
        let from_shares: u128 = env
            .storage()
            .persistent()
            .get(&from_shares_key)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::NoShares));

        if from_shares < amount {
            panic_with_error!(env, PoolError::InsufficientBalance);
        }

        // No-op if transferring to self
        if from == to {
            return;
        }

        // Decrement from sender
        let remaining_shares = from_shares - amount;
        if remaining_shares > 0 {
            env.storage()
                .persistent()
                .set(&from_shares_key, &remaining_shares);
            env.storage()
                .persistent()
                .extend_ttl(&from_shares_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        } else {
            env.storage().persistent().remove(&from_shares_key);
        }

        // Increment to recipient
        let to_shares_key = DataKey::LPShares(to.clone());
        let to_shares: u128 = env.storage().persistent().get(&to_shares_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&to_shares_key, &(to_shares + amount));
        env.storage()
            .persistent()
            .extend_ttl(&to_shares_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    /// Reads an allowance grant, returning `None` when it was never set or has
    /// expired. The expiry check lives here (rather than in the callers) so
    /// every read path agrees on what a dead grant looks like.
    fn live_allowance(env: &Env, from: &Address, spender: &Address) -> Option<ShareAllowance> {
        let record: ShareAllowance = env
            .storage()
            .persistent()
            .get(&DataKey::Allowance(from.clone(), spender.clone()))?;
        if record.expiration_ledger < env.ledger().sequence() {
            return None;
        }
        Some(record)
    }

    /// Funds a listed invoice by moving USDC through escrow and invoice contracts.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to fund.
    ///
    /// # Auth
    /// **Permissionless.** Any caller can trigger funding for an invoice, provided
    /// the invoice passes all on-chain eligibility checks:
    /// 1. Invoice status must be `Listed` (status 1)
    /// 2. Invoice funding asset must match the pool's asset (USDC)
    /// 3. Pool must have sufficient available liquidity
    /// 4. Funding would not cause pool utilization to exceed the `max_utilization_bps` cap
    ///
    /// See README §"Known Centralization Risks & Roadmap" for the longer-term
    /// governance design that will let LPs signal approval on funding decisions.
    ///
    /// # Registry re-verification
    /// The issuer and buyer's registry verification is re-checked here, in
    /// addition to the checks already performed by `invoice.create()` and
    /// `invoice.list_for_financing()`. This is the point where pool capital
    /// is actually committed, so a revocation that happened after listing
    /// must still block new funding. Once funding succeeds, verification is
    /// **not** re-checked again at any later step (`mark_shipped`,
    /// `confirm_delivery`, `repay`, `trigger_default`) — see
    /// `InvoiceContract::list_for_financing` for the rationale.
    ///
    /// # Panics
    /// * `AlreadyFunded` if a `FundedInvoice` entry already exists for this
    ///   invoice id. This check runs first, so a replay of an already-recorded
    ///   funding always surfaces `AlreadyFunded` rather than a status error.
    /// * `InvoiceNotListed` if the invoice is not in listed status.
    /// * `IssuerNotVerified` if the invoice issuer's registry verification has
    ///   since been revoked.
    /// * `BuyerNotVerified` if the invoice buyer's registry verification has
    ///   since been revoked.
    /// * `AssetMismatch` if the invoice funding asset does not match pool USDC.
    /// * `InvalidAmount` if the computed funded amount is zero.
    /// * `InsufficientLiquidity` if the pool does not have enough funds.
    /// * `UtilizationCapExceeded` if funding would push utilization above the cap.
    /// * `Overflow` if `face_value * (10000 - discount_bps)` or the resulting
    ///   utilization calculation overflows `u128`.
    ///
    /// # Returns
    /// * `bool` - `true` when the invoice is funded.
    ///
    /// # Example
    /// ```ignore
    /// client.fund_invoice(&invoice_id);
    /// ```
    pub fn fund_invoice(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        Self::require_initialized(&env);
        // `stop_on_capacity = false`: a single-invoice call always panics on a
        // capacity shortfall, matching the long-standing behavior.
        Self::fund_one(env, invoice_id, false)
    }

    /// Shared body of [`Self::fund_invoice`] and [`Self::batch_fund_invoice`].
    ///
    /// When `stop_on_capacity` is `true` (batch mode), a shortfall in available
    /// liquidity or a utilization-cap breach returns `false` instead of
    /// panicking so the caller can stop cleanly. All eligibility failures (not
    /// listed, already funded, unverified issuer/buyer, asset mismatch) panic in
    /// both modes — those are caller errors, not capacity.
    fn fund_one(env: Env, invoice_id: BytesN<32>, stop_on_capacity: bool) -> bool {
        let invoice_contract = Self::invoice_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));

        // The pool's own idempotency guard runs before any cross-contract read
        // (and before the invoice-status gate below) so a replay of an
        // already-recorded funding always surfaces the precise `AlreadyFunded`
        // error instead of a generic `InvoiceNotListed`. The funding entry is
        // the authoritative record that this invoice was already funded here.
        // Refs: issue #444.
        let funded_key = DataKey::FundedInvoice(invoice_id.clone());
        if env.storage().persistent().has(&funded_key) {
            panic_with_error!(&env, PoolError::AlreadyFunded);
        }

        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        let (invoice_status, face_value, discount_bps): (u32, u128, u32) = env.invoke_contract(
            &invoice_contract,
            &Symbol::new(&env, "get_funding_terms"),
            args,
        );
        if invoice_status != 1 {
            panic_with_error!(&env, PoolError::InvoiceNotListed);
        }

        let registry_id = Self::registry_contract(&env);
        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        let issuer: Address =
            env.invoke_contract(&invoice_contract, &Symbol::new(&env, "get_issuer"), args);
        let mut args = Vec::new(&env);
        args.push_back(issuer.into_val(&env));
        let issuer_verified: bool =
            env.invoke_contract(&registry_id, &Symbol::new(&env, "is_verified"), args);
        if !issuer_verified {
            panic_with_error!(&env, PoolError::IssuerNotVerified);
        }

        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        let buyer: Address =
            env.invoke_contract(&invoice_contract, &Symbol::new(&env, "get_buyer"), args);
        let mut args = Vec::new(&env);
        args.push_back(buyer.into_val(&env));
        let buyer_verified: bool =
            env.invoke_contract(&registry_id, &Symbol::new(&env, "is_verified"), args);
        if !buyer_verified {
            panic_with_error!(&env, PoolError::BuyerNotVerified);
        }

        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        let invoice_asset: Address = env.invoke_contract(
            &invoice_contract,
            &Symbol::new(&env, "get_funding_asset"),
            args,
        );
        let usdc_id = Self::funding_asset(&env);
        if invoice_asset != usdc_id {
            panic_with_error!(&env, PoolError::AssetMismatch);
        }

        // `face_value` is read from the invoice contract via a cross-contract
        // call and is not bounded by this pool, so the scaling multiplication
        // must be guarded just like the utilization check below (#585).
        let funded_amount = face_value
            .checked_mul(10000 - discount_bps as u128)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::Overflow))
            // Integer division floors; any fractional stroop is excluded from
            // the funded principal and remains part of the invoice discount.
            / 10000;
        if funded_amount == 0 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }

        let totals = Self::totals(&env);
        let total_deposits = totals.deposits;
        let total_funded = totals.funded;
        let available = total_deposits - total_funded;
        if funded_amount > available {
            if stop_on_capacity {
                return false;
            }
            panic_with_error!(&env, PoolError::InsufficientLiquidity);
        }

        let max_utilization_bps = totals.max_utilization_bps;
        let new_total_funded = total_funded + funded_amount;
        let utilization_after =
            Self::utilization_bps_or_panic(&env, new_total_funded, total_deposits);
        if utilization_after > max_utilization_bps {
            if stop_on_capacity {
                return false;
            }
            panic_with_error!(&env, PoolError::UtilizationCapExceeded);
        }

        // --- Checks-effects-interactions: commit pool state BEFORE any
        // cross-contract calls so a reentrant callback into this contract
        // always sees the updated TotalFunded / ActiveInvoiceCount /
        // FundedInvoice, preventing double-funding via stale state.
        env.storage()
            .instance()
            .set(&DataKey::TotalFunded, &(total_funded + funded_amount));
        let active_count = totals.active_invoices;
        env.storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &(active_count + 1));

        env.storage().persistent().set(&funded_key, &funded_amount);
        env.storage()
            .persistent()
            .extend_ttl(&funded_key, TTL_THRESHOLD, TTL_EXTEND_TO);

        // --- Interactions: cross-contract calls after pool state is committed.
        let escrow_contract = Self::escrow_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));

        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        args.push_back(funded_amount.into_val(&env));
        args.push_back(issuer.into_val(&env));
        let _: bool = env.invoke_contract(&escrow_contract, &Symbol::new(&env, "lock"), args);

        let pool_address = env.current_contract_address();
        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        args.push_back(pool_address.into_val(&env));
        args.push_back(usdc_id.into_val(&env));
        args.push_back(funded_amount.into_val(&env));
        let _: bool =
            env.invoke_contract(&invoice_contract, &Symbol::new(&env, "mark_funded"), args);

        events::invoice_funded_v1(&env, &invoice_id, funded_amount);
        events::invoice_funded(&env, &invoice_id, funded_amount);
        Self::extend_instance_ttl(&env);
        true
    }

    /// Funds several listed invoices in one call, applying `fund_invoice`'s
    /// checks to each entry and stopping when the pool runs out of capacity.
    ///
    /// Entries are processed in the given order. Each entry that passes every
    /// eligibility check in [`Self::fund_invoice`] is funded exactly as
    /// `fund_invoice` would, including the same cross-contract verification
    /// (listed status, registry verification of issuer and buyer, funding-asset
    /// match) and the same pool-side accounting and `escrow::lock` call.
    ///
    /// # Capacity handling
    ///
    /// When the next entry cannot be funded **because the pool lacks capacity**
    /// — insufficient available liquidity, or a resulting utilization rate above
    /// `max_utilization_bps` — funding **stops** and every remaining entry is
    /// left untouched. This deliberately never partially drains the pool: an
    /// entry that would breach the cap is not funded in part, and no entry after
    /// it is funded either.
    ///
    /// Entries that fail an *eligibility* check (not listed, already funded by
    /// this pool, unverified issuer/buyer, asset mismatch) instead revert the
    /// whole call with the same typed error `fund_invoice` would raise. The
    /// distinction matters: a capacity stop is a normal, expected outcome for
    /// an LP filling the pool, whereas an ineligible entry is a caller mistake
    /// that should be fixed rather than silently skipped.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_ids` - The invoices to fund, in priority order.
    ///
    /// # Auth
    /// None — funding is permissionless, exactly like [`Self::fund_invoice`].
    ///
    /// # Panics
    /// * `PoolError::BatchSizeExceeded` if `invoice_ids.len() > MAX_BATCH_SIZE`.
    /// * `PoolError::NotInitialized` if the pool is not initialized.
    /// * Any eligibility error [`Self::fund_invoice`] documents, for the first
    ///   entry that is ineligible.
    ///
    /// # Returns
    /// * `Vec<BytesN<32>>` - The invoice IDs actually funded, in the order they
    ///   were funded. Callers can diff this against the input to see which
    ///   entries were skipped by a capacity stop.
    ///
    /// # Example
    /// ```ignore
    /// let funded = client.batch_fund_invoice(&invoice_ids);
    /// ```
    pub fn batch_fund_invoice(env: Env, invoice_ids: Vec<BytesN<32>>) -> Vec<BytesN<32>> {
        require_not_paused(&env);
        Self::require_initialized(&env);
        if invoice_ids.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, PoolError::BatchSizeExceeded);
        }

        let mut funded: Vec<BytesN<32>> = Vec::new(&env);
        for invoice_id in invoice_ids.iter() {
            if !Self::fund_one(env.clone(), invoice_id.clone(), true) {
                // Capacity stop: leave this entry and every later one unfunded
                // rather than partially draining the pool.
                break;
            }
            funded.push_back(invoice_id.clone());
        }
        funded
    }

    /// Receives invoice repayment and updates pool liquidity metrics.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being repaid.
    /// * `amount` - The amount repaid.
    ///
    /// # Auth
    /// Requires authorization from the configured `invoice_contract`
    /// (via `invoice_contract.require_auth()`); only the invoice contract may
    /// invoke this entry point.
    ///
    /// # Panics
    /// * `InvoiceNotFound` if the invoice is not funded.
    /// * `InvalidAmount` if the repayment amount is less than the funded amount.
    /// * `ActiveCountUnderflow` if the active-invoice counter would underflow
    ///   (e.g. a mismatched repayment for an invoice that was never funded
    ///   through this pool).
    ///
    /// # Returns
    /// * `bool` - `true` when repayment is processed.
    ///
    /// # Example
    /// ```ignore
    /// client.receive_repayment(&invoice_id, 1_050);
    /// ```
    pub fn receive_repayment(env: Env, invoice_id: BytesN<32>, amount: u128) -> bool {
        require_not_paused(&env);
        let invoice_contract = Self::invoice_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        invoice_contract.require_auth();

        // Shared settlement path; `refund` is 0 so the whole surplus
        // (amount - funded_amount) is credited to LPs as yield.
        Self::settle_repayment(&env, &invoice_id, amount, 0);

        true
    }

    /// Receives invoice repayment with a partial refund to the buyer and updates
    /// pool liquidity metrics.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being repaid.
    /// * `amount` - The amount repaid.
    /// * `refund` - The amount to refund to the buyer.
    /// * `buyer` - The buyer receiving the refund.
    ///
    /// # Auth
    /// Requires authorization from the configured `invoice_contract`
    /// (via `invoice_contract.require_auth()`); only the invoice contract may
    /// invoke this entry point.
    ///
    /// # Trust boundary: the discount/refund split is invoice-computed
    /// `invoice.repay()` / `invoice.repay_early()` independently compute
    /// `earned_by_pool` / `refund_to_buyer` from the invoice's discount,
    /// elapsed time, and term, and pass the resulting `refund` here. Pool has
    /// no visibility into `funded_at`, `due_date`, or elapsed/term at all —
    /// it only bounds `refund` to `[0, amount - funded_amount]` (the maximum
    /// possible discount) via `InvalidAmount`. Pool does **not** independently
    /// verify that `refund` is proportional to time elapsed; any `refund`
    /// invoice passes within that bound is accepted unconditionally, and the
    /// remainder is credited to LPs as yield. Correctness of the time-based
    /// split is entirely `invoice_contract`'s responsibility.
    ///
    /// # Panics
    /// * `InvoiceNotFound` if the invoice is not funded.
    /// * `InvalidAmount` if the repayment amount is less than the funded amount,
    ///   or if the refund exceeds the maximum allowed.
    /// * `ActiveCountUnderflow` if the active-invoice counter would underflow
    ///   (e.g. a mismatched repayment for an invoice that was never funded
    ///   through this pool).
    ///
    /// # Returns
    /// * `bool` - `true` when repayment is processed.
    ///
    /// # Example
    /// ```ignore
    /// client.receive_repayment_with_refund(&invoice_id, 1_050, 50, &buyer);
    /// ```
    pub fn receive_repayment_with_refund(
        env: Env,
        invoice_id: BytesN<32>,
        amount: u128,
        refund: u128,
        buyer: Address,
    ) -> bool {
        require_not_paused(&env);
        let invoice_contract = Self::invoice_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        invoice_contract.require_auth();

        // Shared settlement path: `settle_repayment` owns the funded-entry
        // lookup, the `amount >= funded_amount` and refund-bound validation,
        // the totals update, the funded-entry removal, the event, and the
        // instance TTL bump. Only the refund's USDC transfer is specific to
        // this entry point and is layered on top here.
        Self::settle_repayment(&env, &invoice_id, amount, refund);

        // Transfer the buyer's refund out of the pool's USDC balance. Skipped
        // entirely when the refund is zero.
        let usdc_id = Self::funding_asset(&env);
        let usdc = token::Client::new(&env, &usdc_id);
        if refund > 0 {
            usdc.transfer(&env.current_contract_address(), &buyer, &(refund as i128));
        }

        true
    }

    /// Forwards a defaulted invoice to escrow default handling and updates
    /// the invoice status on the invoice contract.
    ///
    /// This function performs the following cross-contract sequence:
    /// 1. Calls `escrow.handle_default()` to release escrowed funds back
    ///    to the pool.
    /// 2. Calls `invoice.mark_defaulted()` to persist the `Defaulted` status
    ///    on the invoice record, update the status index, and emit the
    ///    `invoice_defaulted` event.
    /// 3. Updates the pool's local accounting and removes the funded invoice
    ///    entry. Only `TotalFunded`, `TotalLossRealised`, and
    ///    `ActiveInvoiceCount` change: `TotalDeposits` is intentionally left
    ///    untouched because escrow returns the principal to the pool, so LP
    ///    capital is preserved (issue #439).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The defaulted invoice.
    ///
    /// # Auth
    /// Requires authorization from the configured `invoice_contract`
    /// (via `invoice_contract.require_auth()`); only the invoice contract may
    /// invoke this entry point.
    ///
    /// # Panics
    /// * `InvoiceNotFound` if no funded invoice entry exists for `invoice_id`.
    /// * `EscrowDefaultNotReleased` if `escrow.handle_default()` returns `false`
    ///   (e.g. no lock record exists in escrow for this invoice, so no tokens
    ///   were actually transferred back to the pool). Without this check, pool
    ///   accounting would otherwise proceed to record a loss and free up
    ///   utilization as if funds had been recovered, even though escrow moved
    ///   nothing.
    /// * `ActiveCountUnderflow` if the active-invoice counter would underflow
    ///   (e.g. double-default of the same invoice).
    ///
    /// # Returns
    /// * `bool` - `true` when default handling completes.
    ///
    /// # Example
    /// ```ignore
    /// client.handle_default(&invoice_id);
    /// ```
    pub fn handle_default(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        let invoice_contract = Self::invoice_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        invoice_contract.require_auth();

        let funded_key = DataKey::FundedInvoice(invoice_id.clone());
        if !env.storage().persistent().has(&funded_key) {
            panic_with_error!(&env, PoolError::InvoiceNotFound);
        }
        let funded_amount: u128 = env
            .storage()
            .persistent()
            .get(&funded_key)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::InvoiceNotFound));

        let escrow_contract = Self::escrow_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        let pool_address = env.current_contract_address();
        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        args.push_back(pool_address.into_val(&env));
        let escrow_released: bool =
            env.invoke_contract(&escrow_contract, &Symbol::new(&env, "handle_default"), args);
        if !escrow_released {
            panic_with_error!(&env, PoolError::EscrowDefaultNotReleased);
        }

        // Escrow returns the full locked principal to the pool on default (see
        // `EscrowContract::handle_default`), so LP capital is not lost here:
        // only `TotalFunded` — capital currently out on funded invoices — is
        // decremented. `TotalDeposits` is intentionally left unchanged so
        // `available_liquidity = TotalDeposits - TotalFunded` rises by exactly
        // the recovered amount and LP share value is preserved. Previously this
        // also subtracted `funded_amount` from `TotalDeposits`, which
        // double-counted the loss and artificially deflated share value. Refs:
        // issue #439.
        let totals = Self::totals(&env);
        let total_funded = totals.funded;
        let total_loss_realised = totals.loss_realised;

        let new_total_funded = total_funded
            .checked_sub(funded_amount)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::Overflow));
        env.storage()
            .instance()
            .set(&DataKey::TotalFunded, &new_total_funded);
        env.storage().instance().set(
            &DataKey::TotalLossRealised,
            &(total_loss_realised + funded_amount),
        );

        let active_count = totals.active_invoices;
        let new_active_count = active_count
            .checked_sub(1)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::ActiveCountUnderflow));
        env.storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &new_active_count);

        // Persist the Defaulted status on the invoice contract (step 2 of the
        // documented cross-contract sequence). This runs after the active-count
        // underflow check so a mismatched default still surfaces
        // ActiveCountUnderflow (#17) rather than an invoice lookup error from
        // mark_defaulted. mark_defaulted is idempotent: when
        // invoice.trigger_default already transitioned the status to Defaulted
        // before invoking this pool entry point, the call is a no-op.
        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        let _: bool = env.invoke_contract(
            &invoice_contract,
            &Symbol::new(&env, "mark_defaulted"),
            args,
        );

        env.storage().persistent().remove(&funded_key);

        events::invoice_defaulted(&env, &invoice_id, funded_amount);
        Self::extend_instance_ttl(&env);
        true
    }

    /// Returns the LP share balance for a given address (SEP-41 interface).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `id` - The address to query the balance for.
    ///
    /// # Returns
    /// * `i128` - The share balance cast to `i128`.
    pub fn balance(env: Env, id: Address) -> i128 {
        let shares: u128 = env
            .storage()
            .persistent()
            .get(&DataKey::LPShares(id))
            .unwrap_or(0);
        shares as i128
    }

    /// Returns the total supply of LP shares (SEP-41 interface).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Returns
    /// * `i128` - The total supply of shares cast to `i128`.
    pub fn total_supply(env: Env) -> i128 {
        let total: u128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalShares)
            .unwrap_or(0);
        total as i128
    }

    /// Returns current pool statistics and utilization metrics.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool contract has not been initialized.
    /// * `Overflow` if scaling `total_funded` into basis points would overflow.
    ///
    /// # Returns
    /// * `PoolStats` - The current pool statistics.
    ///
    /// # Example
    /// ```ignore
    /// let stats = client.get_stats();
    /// ```
    pub fn get_stats(env: Env) -> PoolStats {
        if Self::admin(&env).is_none() {
            panic_with_error!(&env, PoolError::NotInitialized);
        }
        let totals = Self::totals(&env);
        let total_deposits = totals.deposits;
        let total_funded = totals.funded;
        let available = total_deposits - total_funded;
        let utilization = Self::utilization_bps_or_panic(&env, total_funded, total_deposits);

        PoolStats {
            total_deposits,
            total_funded,
            available_liquidity: available,
            utilization_rate_bps: utilization,
            total_yield_distributed: totals.yield_distributed,
            total_loss_realised: totals.loss_realised,
            active_invoice_count: totals.active_invoices,
            total_shares: totals.shares,
            max_utilization_bps: totals.max_utilization_bps,
        }
    }

    /// Returns the LP's position, including shares, value, yield, and deposits.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `lp` - The liquidity provider address.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `Overflow` if `lp_shares * total_deposits` would overflow `u128`
    ///   while computing the position's USDC value.
    ///
    /// All storage reads default to `0` when the LP has no recorded position,
    /// so an LP without a position simply reports zeros.
    ///
    /// # TTL maintenance
    /// `get_lp_position` is a read that keeps a live position alive: every
    /// surviving LP-scoped persistent entry it reads (`LPShares`,
    /// `LPYieldEarned`, `LPDepositCount`, and `LPInitialDeposit`) has its TTL
    /// extended using the same threshold/target as the deposit/withdraw write
    /// paths. This matters for deposit-and-hold LPs who never call
    /// `deposit`/`withdraw` again — without the read-triggered bump their
    /// entries would lapse and become archival-eligible even though the
    /// position is still economically live (#588). Entries that do not exist
    /// are skipped, so reads for an LP with no position perform no writes.
    ///
    /// # Returns
    /// * `LPPosition` - The LP position details.
    ///
    /// # Example
    /// ```ignore
    /// let position = client.get_lp_position(&lp);
    /// ```
    pub fn get_lp_position(env: Env, lp: Address) -> LPPosition {
        let lp_shares: u128 = env
            .storage()
            .persistent()
            .get(&DataKey::LPShares(lp.clone()))
            .unwrap_or(0);
        let totals = Self::totals(&env);
        let total_shares = totals.shares;
        let total_deposits = totals.deposits;

        let usdc_value = if total_shares > 0 && lp_shares > 0 {
            let scaled = lp_shares
                .checked_mul(total_deposits)
                .unwrap_or_else(|| panic_with_error!(&env, PoolError::Overflow));
            scaled / total_shares
        } else {
            0
        };

        let yield_earned: u128 = env
            .storage()
            .persistent()
            .get(&DataKey::LPYieldEarned(lp.clone()))
            .unwrap_or(0);
        let deposit_count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::LPDepositCount(lp.clone()))
            .unwrap_or(0);

        // Read-triggered TTL bump (#588): deposit/withdraw are the only write
        // paths that extend the LP-scoped persistent entries, so a
        // deposit-and-hold LP's entries would otherwise lapse and become
        // archival-eligible even though their position is still live. Refresh
        // every surviving LP entry here, same raw policy as the write path.
        // LPInitialDeposit has no field on `LPPosition`, but it is part of the
        // same deposit-time write bundle, so it is refreshed too.
        //
        // The `has()` guard keeps reads for an LP with no recorded position
        // (or a fully withdrawn one) from calling `extend_ttl` on keys that do
        // not exist, which would host-panic. Entries whose TTL is already at or
        // beyond the extend target are no-ops on chain, so this is cheap.
        for key in [
            DataKey::LPShares(lp.clone()),
            DataKey::LPYieldEarned(lp.clone()),
            DataKey::LPDepositCount(lp.clone()),
            DataKey::LPInitialDeposit(lp.clone()),
        ] {
            if env.storage().persistent().has(&key) {
                env.storage()
                    .persistent()
                    .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
            }
        }

        LPPosition {
            shares: lp_shares,
            usdc_value,
            yield_earned,
            deposit_count,
        }
    }

    /// Returns the pool utilization rate as basis points.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `Overflow` if scaling `total_funded` into basis points would overflow.
    ///
    /// # Returns
    /// * `u32` - The utilization rate in basis points, or `0` when
    ///   `total_deposits` is zero.
    ///
    /// # Example
    /// ```ignore
    /// let utilization = client.get_utilization_rate();
    /// ```
    pub fn get_utilization_rate(env: Env) -> u32 {
        let totals = Self::totals(&env);
        Self::utilization_bps_or_panic(&env, totals.funded, totals.deposits)
    }

    /// Suggests a `discount_bps` for an issuer to consider when calling
    /// `invoice::list_for_financing`, interpolated linearly from the pool's
    /// current utilization.
    ///
    /// The curve runs from [`SUGGESTED_DISCOUNT_FLOOR_BPS`] at 0% utilization
    /// (an idle pool prices in almost no risk discount) up to
    /// [`SUGGESTED_DISCOUNT_CEILING_BPS`] at 100% utilization (a fully
    /// deployed pool should only fund invoices that pay close to the invoice
    /// contract's hard 5000 bps discount cap). At utilization `u` the
    /// suggestion is `floor + (ceiling - floor) * u / 10_000`.
    ///
    /// This is advisory only: it is a pure read of the pool's current
    /// `TotalFunded / TotalDeposits` ratio and does not change how
    /// `fund_invoice` prices an invoice — the issuer still chooses
    /// `discount_bps` at listing time and the pool keeps funding whatever it
    /// was told. It exists so frontends and issuers can query an on-chain
    /// utilization signal instead of hardcoding rates (issue #719).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `PoolError::Overflow` if `TotalFunded * 10_000` overflows `u128`
    ///   while the utilization rate is computed (unreachable for realistic
    ///   supply values).
    ///
    /// # Returns
    /// * `u32` - The suggested discount in basis points, within
    ///   `[SUGGESTED_DISCOUNT_FLOOR_BPS, SUGGESTED_DISCOUNT_CEILING_BPS]`.
    ///   An empty (or uninitialized) pool reports 0% utilization and therefore
    ///   returns the floor.
    ///
    /// # Example
    /// ```ignore
    /// let suggested_bps = client.get_suggested_discount_bps();
    /// ```
    pub fn get_suggested_discount_bps(env: Env) -> u32 {
        let utilization_bps = Self::get_utilization_rate(env.clone());
        SUGGESTED_DISCOUNT_FLOOR_BPS
            + (SUGGESTED_DISCOUNT_CEILING_BPS - SUGGESTED_DISCOUNT_FLOOR_BPS) * utilization_bps
                / 10_000
    }

    /// Returns the pool's active funded amount for an invoice, if any.
    ///
    /// A pure read of the `DataKey::FundedInvoice(invoice_id)` persistent
    /// entry that `fund_invoice` writes and the repayment and default paths
    /// remove, so the result mirrors the invoice's funding lifecycle exactly:
    /// * `None` before the invoice has ever been funded through this pool.
    /// * `Some(amount)` while the pool holds an active funding for the
    ///   invoice, where `amount` is the USDC amount committed at funding
    ///   (`face_value * (10000 - discount_bps) / 10000`).
    /// * `None` again after repayment (`receive_repayment` /
    ///   `receive_repayment_with_refund`) or default (`handle_default`)
    ///   removes the entry.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query the active funding for.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Never panics; this is a read-only storage lookup. No state is
    /// written and no TTL is extended, so it can be called freely by
    /// indexers and off-chain callers.
    ///
    /// # Returns
    /// * `Option<u128>` - `Some(amount)` while the invoice is actively
    ///   funded by this pool, `None` before funding and after
    ///   repayment/default.
    ///
    /// # Example
    /// ```ignore
    /// let funded = client.get_funded_amount(&invoice_id);
    /// ```
    pub fn get_funded_amount(env: Env, invoice_id: BytesN<32>) -> Option<u128> {
        env.storage()
            .persistent()
            .get(&DataKey::FundedInvoice(invoice_id))
    }

    /// Updates the pool's maximum utilization cap.
    ///
    /// The cap bounds the utilization (in basis points) that `fund_invoice`
    /// may drive the pool to: funding is rejected with
    /// `UtilizationCapExceeded` when the post-funding utilization would
    /// exceed it. The current cap is reported in
    /// `PoolStats::max_utilization_bps`.
    ///
    /// Emits a `max_utilization_updated` event carrying the old and new cap
    /// values so off-chain indexers can observe risk-parameter changes
    /// without polling `get_stats()`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The admin address for this contract.
    /// * `new_cap_bps` - The new utilization cap, in basis points
    ///   (`10_000` = 100%).
    ///
    /// # Auth
    /// Requires authorization from `admin` (via `admin.require_auth()`).
    ///
    /// # Panics
    /// * `InvalidConfiguration` if `new_cap_bps` is zero. A zero cap would
    ///   make every positive funding attempt fail and is therefore not a
    ///   valid operating configuration.
    /// * `InvalidAmount` if `new_cap_bps` exceeds `10_000`.
    ///
    /// # Returns
    /// * `bool` - `true` when the cap is updated.
    ///
    /// # Example
    /// ```ignore
    /// client.set_max_utilization(&admin, &9000);
    /// ```
    pub fn set_max_utilization(env: Env, admin: Address, new_cap_bps: u32) -> bool {
        require_not_paused(&env);
        admin.require_auth();
        if new_cap_bps == 0 {
            panic_with_error!(&env, PoolError::InvalidConfiguration);
        }
        if new_cap_bps > 10000 {
            panic_with_error!(&env, PoolError::InvalidAmount);
        }
        let old_cap_bps = Self::totals(&env).max_utilization_bps;
        env.storage()
            .instance()
            .set(&DataKey::MaxUtilizationBps, &new_cap_bps);
        events::max_utilization_updated(&env, old_cap_bps, new_cap_bps);
        Self::extend_instance_ttl(&env);
        true
    }

    /// Sets the protocol fee in basis points and the treasury address.
    ///
    /// Requires authorization from the contract admin. Updates both
    /// `DataKey::ProtocolFeeBps` and `DataKey::TreasuryAddress` in contract storage.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `fee_bps` - The new protocol fee in basis points (max `2000` = 20%).
    /// * `treasury` - The address receiving protocol cuts.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the pool is not initialized.
    /// * `FeeTooHigh` if `fee_bps` exceeds `MAX_PROTOCOL_FEE_BPS` (2000 bps).
    /// * `InvalidConfiguration` if `treasury` is the pool, invoice, escrow,
    ///   registry, or funding-asset address.
    ///
    /// # Returns
    /// * `bool` - `true` when the fee is updated.
    pub fn set_protocol_fee(env: Env, fee_bps: u32, treasury: Address) -> bool {
        require_not_paused(&env);
        let admin =
            Self::admin(&env).unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        admin.require_auth();
        if fee_bps > MAX_PROTOCOL_FEE_BPS {
            panic_with_error!(&env, PoolError::FeeTooHigh);
        }
        let invoice_contract = Self::invoice_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        let escrow_contract = Self::escrow_contract(&env)
            .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized));
        Self::assert_valid_treasury(
            &env,
            &treasury,
            &invoice_contract,
            &escrow_contract,
            &Self::funding_asset(&env),
            &Self::registry_contract(&env),
        );
        let old_fee_bps = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0u32);
        // Explicitly update both DataKey::ProtocolFeeBps and DataKey::TreasuryAddress
        env.storage()
            .instance()
            .set(&DataKey::ProtocolFeeBps, &fee_bps);
        // Explicitly store updated treasury address
        env.storage()
            .instance()
            .set(&DataKey::TreasuryAddress, &treasury);
        events::protocol_fee_updated(&env, old_fee_bps, fee_bps, &treasury);
        Self::extend_instance_ttl(&env);
        true
    }

    /// Returns the current protocol fee in basis points.
    pub fn get_protocol_fee_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0u32)
    }

    /// Returns the configured treasury address.
    pub fn get_treasury(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::TreasuryAddress)
            .unwrap_or_else(|| {
                Self::admin(&env)
                    .unwrap_or_else(|| panic_with_error!(&env, PoolError::NotInitialized))
            })
    }

    fn utilization_bps_or_panic(env: &Env, total_funded: u128, total_deposits: u128) -> u32 {
        if total_deposits == 0 {
            return 0;
        }

        let scaled_funded = total_funded
            .checked_mul(10_000)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::Overflow));

        scaled_funded.checked_div(total_deposits).unwrap_or(0) as u32
    }

    fn require_initialized(env: &Env) {
        if !env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(env, PoolError::NotInitialized);
        }
    }

    /// Prevents protocol fees from being routed to the pool itself or to one
    /// of the contracts that participate in its settlement wiring.
    fn assert_valid_treasury(
        env: &Env,
        treasury: &Address,
        invoice_contract: &Address,
        escrow_contract: &Address,
        funding_asset: &Address,
        registry_contract: &Address,
    ) {
        if treasury == &env.current_contract_address()
            || treasury == invoice_contract
            || treasury == escrow_contract
            || treasury == funding_asset
            || treasury == registry_contract
        {
            panic_with_error!(env, PoolError::InvalidConfiguration);
        }
    }

    fn admin(env: &Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
    }

    fn invoice_contract(env: &Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::InvoiceContract)
    }

    fn escrow_contract(env: &Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::EscrowContract)
    }

    fn funding_asset(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::FundingAsset)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::NotInitialized))
    }

    fn min_initial_deposit(env: &Env) -> u128 {
        env.storage()
            .instance()
            .get(&DataKey::MinInitialDeposit)
            .unwrap_or(DEFAULT_MIN_INITIAL_DEPOSIT)
    }

    fn shares_for_deposit(env: &Env, usdc_amount: u128) -> u128 {
        if usdc_amount == 0 || usdc_amount > i128::MAX as u128 {
            panic_with_error!(env, PoolError::InvalidAmount);
        }

        let totals = Self::totals(env);
        if (totals.shares == 0 || totals.deposits == 0)
            && usdc_amount < Self::min_initial_deposit(env)
        {
            panic_with_error!(env, PoolError::InvalidAmount);
        }

        let shares_to_issue = if totals.shares == 0 || totals.deposits == 0 {
            usdc_amount
        } else {
            let scaled = usdc_amount
                .checked_mul(totals.shares)
                .unwrap_or_else(|| panic_with_error!(env, PoolError::Overflow));
            scaled / totals.deposits
        };

        if shares_to_issue == 0 {
            panic_with_error!(env, PoolError::MinimumDeposit);
        }

        shares_to_issue
    }

    /// Converts LP shares to the USDC `withdraw` would return.
    ///
    /// This is the single source of truth for withdrawal rounding: integer
    /// division floors the result, and the zero-output dust guard matches the
    /// deposit-side one. Both `withdraw` and `preview_withdraw` call this, so a
    /// quote can never drift from the amount the write path pays out.
    fn usdc_for_withdraw(
        env: &Env,
        shares: u128,
        total_shares: u128,
        total_deposits: u128,
    ) -> u128 {
        if shares == 0 {
            panic_with_error!(env, PoolError::InvalidAmount);
        }
        if total_shares == 0 {
            panic_with_error!(env, PoolError::NoShares);
        }

        let scaled = shares
            .checked_mul(total_deposits)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::Overflow));
        let usdc_to_return = scaled / total_shares;
        if usdc_to_return == 0 {
            panic_with_error!(env, PoolError::MinimumDeposit);
        }

        usdc_to_return
    }

    fn registry_contract(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::RegistryContract)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::NotInitialized))
    }

    fn fee_bps(env: &Env) -> Option<u32> {
        env.storage().instance().get(&DataKey::FeeBps)
    }

    fn treasury_address(env: &Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::TreasuryAddress)
    }

    fn totals(env: &Env) -> PoolTotals {
        PoolTotals {
            shares: env
                .storage()
                .instance()
                .get(&DataKey::TotalShares)
                .unwrap_or(0),
            deposits: env
                .storage()
                .instance()
                .get(&DataKey::TotalDeposits)
                .unwrap_or(0),
            funded: env
                .storage()
                .instance()
                .get(&DataKey::TotalFunded)
                .unwrap_or(0),
            yield_distributed: env
                .storage()
                .instance()
                .get(&DataKey::TotalYieldDistributed)
                .unwrap_or(0),
            loss_realised: env
                .storage()
                .instance()
                .get(&DataKey::TotalLossRealised)
                .unwrap_or(0),
            active_invoices: env
                .storage()
                .instance()
                .get(&DataKey::ActiveInvoiceCount)
                .unwrap_or(0),
            max_utilization_bps: env
                .storage()
                .instance()
                .get(&DataKey::MaxUtilizationBps)
                .unwrap_or(DEFAULT_MAX_UTILIZATION_BPS),
        }
    }

    /// Shared settlement path for invoice repayments, called by both
    /// `receive_repayment` and `receive_repayment_with_refund`.
    ///
    /// Owns every bookkeeping step the two entry points have in common, so a
    /// future fix to the settlement logic only needs to be applied here:
    /// 1. Looks up the `FundedInvoice` entry, panicking with `InvoiceNotFound`
    ///    if the invoice was never funded.
    /// 2. Panics with `InvalidAmount` if `amount` does not cover the funded
    ///    amount, or if `refund` would exceed the repayment surplus
    ///    (`amount - funded_amount`). The refund bound is enforced here rather
    ///    than only in the refund entry point so the helper itself guarantees
    ///    the LP yield slice (`amount - funded_amount - refund`) is never
    ///    negative; `receive_repayment` calls this with `refund = 0`, for which
    ///    the bound is trivially satisfied.
    /// 3. Credits `yield_amount` to `TotalDeposits` / `TotalYieldDistributed`,
    ///    removes the funded principal from `TotalFunded`, and decrements
    ///    `ActiveInvoiceCount` (panicking with `ActiveCountUnderflow` if it
    ///    would go negative).
    /// 4. Removes the funded entry, emits `repayment_received`, and extends
    ///    the instance TTL.
    ///
    /// Refund-specific logic (the USDC transfer back to the buyer) is layered
    /// on top in `receive_repayment_with_refund`.
    fn settle_repayment(env: &Env, invoice_id: &BytesN<32>, amount: u128, refund: u128) {
        let funded_key = DataKey::FundedInvoice(invoice_id.clone());
        let funded_amount: u128 = env
            .storage()
            .persistent()
            .get(&funded_key)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::InvoiceNotFound));
        if amount < funded_amount {
            panic_with_error!(env, PoolError::InvalidAmount);
        }
        if refund > amount - funded_amount {
            panic_with_error!(env, PoolError::InvalidAmount);
        }

        let yield_amount = amount - funded_amount - refund;

        // Calculate protocol fee split. `ProtocolFeeBps` is the live slot
        // written by `set_protocol_fee`; the legacy `DataKey::FeeBps` slot is
        // always zero and is no longer consulted here.
        let fee_bps = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0u32);
        let protocol_cut = if fee_bps > 0 {
            yield_amount
                .checked_mul(fee_bps as u128)
                .unwrap_or_else(|| panic_with_error!(env, PoolError::Overflow))
                // Floor the treasury share; the fractional remainder stays
                // with LPs so the split conserves every whole stroop.
                / 10_000
        } else {
            0
        };
        let lp_yield = yield_amount - protocol_cut;

        let totals = Self::totals(env);
        let total_deposits = totals.deposits;
        let total_funded = totals.funded;
        let total_yield = totals.yield_distributed;

        if protocol_cut > 0 {
            if let Some(treasury) = env
                .storage()
                .instance()
                .get::<_, Address>(&DataKey::TreasuryAddress)
            {
                let usdc_id = Self::funding_asset(env);
                let usdc = token::Client::new(env, &usdc_id);
                usdc.transfer(
                    &env.current_contract_address(),
                    &treasury,
                    &(protocol_cut as i128),
                );
            }
        }

        let new_total_funded = total_funded
            .checked_sub(funded_amount)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::Overflow));

        // Add LP yield (not full yield) to TotalDeposits and TotalYieldDistributed
        env.storage()
            .instance()
            .set(&DataKey::TotalDeposits, &(total_deposits + lp_yield));
        env.storage()
            .instance()
            .set(&DataKey::TotalYieldDistributed, &(total_yield + lp_yield));
        env.storage()
            .instance()
            .set(&DataKey::TotalFunded, &new_total_funded);

        let active_count = totals.active_invoices;
        let new_active_count = active_count
            .checked_sub(1)
            .unwrap_or_else(|| panic_with_error!(env, PoolError::ActiveCountUnderflow));
        env.storage()
            .instance()
            .set(&DataKey::ActiveInvoiceCount, &new_active_count);

        env.storage().persistent().remove(&funded_key);

        // Update event to include protocol cut information
        events::repayment_received(env, invoice_id, amount, lp_yield, protocol_cut);
        Self::extend_instance_ttl(env);
    }

    fn extend_instance_ttl(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    /// Internal helper to mint LP shares (scoped for SEP-41 share issuance).
    fn _mint(env: &Env, to: &Address, amount: u128) {
        let total_shares = Self::totals(env).shares;
        env.storage()
            .instance()
            .set(&DataKey::TotalShares, &(total_shares + amount));

        let lp_shares_key = DataKey::LPShares(to.clone());
        let lp_shares: u128 = env.storage().persistent().get(&lp_shares_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&lp_shares_key, &(lp_shares + amount));
        env.storage()
            .persistent()
            .extend_ttl(&lp_shares_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    /// Internal helper to burn LP shares (scoped for SEP-41 share redemption).
    fn _burn(env: &Env, from: &Address, amount: u128) -> u128 {
        let total_shares = Self::totals(env).shares;
        env.storage()
            .instance()
            .set(&DataKey::TotalShares, &(total_shares - amount));

        let lp_shares_key = DataKey::LPShares(from.clone());
        let lp_shares: u128 = env.storage().persistent().get(&lp_shares_key).unwrap_or(0);
        let remaining_shares = lp_shares - amount;
        env.storage()
            .persistent()
            .set(&lp_shares_key, &remaining_shares);
        env.storage()
            .persistent()
            .extend_ttl(&lp_shares_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        remaining_shares
    }
}
