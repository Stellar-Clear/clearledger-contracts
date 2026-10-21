#![no_std]

use clearledger_pause::{require_not_paused, set_paused};
use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Vec};

mod constants;
mod errors;
mod events;
mod test;
mod types;

pub use constants::*;
pub use errors::*;
pub use types::*;

const DEFAULT_MIN_LOCK_SECONDS: u64 = 60;

/// Maximum number of events a single [`EscrowContract::get_history_page`] call
/// may return.
///
/// The cap keeps every history read within a predictable Soroban resource
/// budget regardless of how many events an invoice has accumulated. Requests
/// with `limit == 0` or `limit > MAX_HISTORY_LIMIT` panic with
/// [`EscrowError::InvalidLimit`].
pub const MAX_HISTORY_LIMIT: u32 = 50;

/// Maximum number of [`EscrowEvent`] entries retained in contract storage per
/// invoice. New events push out the oldest entries once this cap is reached;
/// older history remains available from the emitted contract events.
pub const MAX_HISTORY_EVENTS: u32 = 32;

/// Minimum retention an admin may request from [`EscrowContract::compact_history`].
/// At least one event is kept so the invoice's most recent state transition is
/// always readable on-chain.
pub const MIN_HISTORY_EVENTS: u32 = 1;

/// Maximum number of entries a single [`EscrowContract::compact_history`] call
/// removes. This bounds the CPU/memory cost of the call for legacy histories
/// that predate automatic compaction; callers repeat the call until it returns
/// `0`.
pub const MAX_COMPACT_PER_CALL: u32 = 32;

#[contract]
pub struct EscrowContract;

#[contractimpl]
impl EscrowContract {
    /// Initializes the escrow contract and stores required contract references.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The admin address for this contract.
    /// * `pool_contract` - The pool contract address.
    /// * `usdc_asset` - The USDC asset address.
    ///
    /// # Auth
    /// Requires authorization from `admin`.
    ///
    /// # Panics
    /// * `AlreadyInitialized` if the contract has already been initialized.
    /// * `InvalidContractAddress` if `pool_contract` aliases `admin` or this
    ///   escrow contract.
    /// * `InvalidAsset` if `usdc_asset` aliases `admin`, `pool_contract`, or
    ///   this escrow contract.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.initialize(&admin, &pool, &usdc);
    /// ```
    pub fn initialize(env: Env, admin: Address, pool_contract: Address, usdc_asset: Address) {
        require_not_paused(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, EscrowError::AlreadyInitialized);
        }
        if pool_contract == admin || pool_contract == env.current_contract_address() {
            panic_with_error!(&env, EscrowError::InvalidContractAddress);
        }
        if usdc_asset == admin
            || usdc_asset == pool_contract
            || usdc_asset == env.current_contract_address()
        {
            panic_with_error!(&env, EscrowError::InvalidAsset);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::PoolContract, &pool_contract);
        env.storage()
            .instance()
            .set(&DataKey::UsdcAsset, &usdc_asset);
        Self::extend_instance_ttl(&env);
    }

    /// Get a token client for the USDC asset stored in the contract.
    fn usdc_client(env: &Env) -> token::Client<'_> {
        let usdc_id: Address = env
            .storage()
            .instance()
            .get(&DataKey::UsdcAsset)
            .unwrap_or_else(|| panic_with_error!(env, EscrowError::NotInitialized));
        token::Client::new(env, &usdc_id)
    }

    /// Returns the USDC asset this escrow contract was initialized with.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// None. This is a read-only view.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    ///
    /// # Returns
    /// * `Address` - The USDC asset address.
    ///
    /// # Example
    /// ```ignore
    /// let asset = client.get_usdc_asset();
    /// ```
    pub fn get_usdc_asset(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::UsdcAsset)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotInitialized))
    }

    /// Returns the admin address this escrow contract was initialized with.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// None. This is a read-only view.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    ///
    /// # Returns
    /// * `Address` - The admin address.
    ///
    /// # Example
    /// ```ignore
    /// let admin = client.get_admin();
    /// ```
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotInitialized))
    }

    /// Replaces this contract's Wasm with an installed Wasm using the stored admin.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotInitialized));
        admin.require_auth();

        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        Self::extend_instance_ttl(&env);
        events::upgraded(&env, &new_wasm_hash);
    }

    /// Returns the pool contract address this escrow contract was initialized with.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// None. This is a read-only view.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    ///
    /// # Returns
    /// * `Address` - The pool contract address.
    ///
    /// # Example
    /// ```ignore
    /// let pool = client.get_pool_contract();
    /// ```
    pub fn get_pool_contract(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotInitialized))
    }

    /// Locks USDC in escrow against a funded invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice ID being locked.
    /// * `amount` - The amount to lock. Must be in the inclusive range
    ///   `1..=i128::MAX`, since the USDC token transfer takes an `i128`.
    /// * `issuer` - The invoice issuer address authorized to receive released
    ///   funds. Recorded as the sole source of truth for `release_to_issuer`.
    ///
    /// # Auth
    /// Requires authorization from the configured pool contract.
    ///
    /// # Ordering
    /// Amount validation and the duplicate-lock check both run before the USDC
    /// transfer. A rejected lock therefore moves no tokens, writes no record,
    /// and appends no history. In particular, a duplicate lock never overwrites
    /// the original record's `issuer` or `amount`.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    /// * `InvalidAmount` if the amount is zero or greater than `i128::MAX`.
    /// * `AlreadyLocked` if the invoice is already locked.
    ///
    /// # Returns
    /// * `bool` - `true` when the funds are locked.
    ///
    /// # Example
    /// ```ignore
    /// client.lock(&invoice_id, &amount, &issuer);
    /// ```
    pub fn lock(env: Env, invoice_id: BytesN<32>, amount: u128, issuer: Address) -> bool {
        require_not_paused(&env);
        let pool = Self::require_pool_auth(&env);

        if amount == 0 || amount > i128::MAX as u128 {
            panic_with_error!(&env, EscrowError::InvalidAmount);
        }

        let key = DataKey::Locked(invoice_id.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, EscrowError::AlreadyLocked);
        }

        let usdc = Self::usdc_client(&env);
        usdc.transfer(&pool, &env.current_contract_address(), &(amount as i128));

        let record = EscrowRecord {
            invoice_id: invoice_id.clone(),
            amount,
            locked_at: env.ledger().timestamp(),
            issuer: issuer.clone(),
        };
        env.storage().persistent().set(&key, &record);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::append_history(&env, &invoice_id, EscrowAction::Locked, amount, None);
        Self::extend_instance_ttl(&env);
        events::funds_locked(&env, &invoice_id, &issuer, amount);

        true
    }

    /// Releases escrowed funds to the issuer.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice whose escrow is released.
    /// * `issuer` - Caller-supplied issuer address. The parameter is retained
    ///   for ABI compatibility; it must equal the issuer recorded by `lock`.
    ///
    /// # Auth
    /// Requires authorization from the configured pool contract.
    ///
    /// # Trust boundary
    /// The issuer stored in the [`EscrowRecord`] at lock time is the sole
    /// source of truth. A caller-supplied `issuer` is accepted only when it
    /// equals that stored address, and the token transfer and emitted event
    /// always use the stored address. A mismatch reverts with
    /// `InvalidRecipient` before any token transfer or storage mutation.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    /// * `NotFound` if no escrow record exists for the invoice.
    /// * `InvalidRecipient` if `issuer` does not match the stored issuer address.
    /// * `InvalidRecipient` if the stored issuer is the escrow or pool contract address.
    /// * `InvalidAmount` if the stored amount exceeds `i128::MAX`.
    ///
    /// # Returns
    /// * `bool` - `true` when funds are released.
    ///
    /// # Example
    /// ```ignore
    /// client.release_to_issuer(&invoice_id, &issuer);
    /// ```
    pub fn release_to_issuer(env: Env, invoice_id: BytesN<32>, issuer: Address) -> bool {
        require_not_paused(&env);
        let pool = Self::require_pool_auth(&env);

        let key = DataKey::Locked(invoice_id.clone());
        let record: EscrowRecord = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotFound));

        if issuer != record.issuer {
            panic_with_error!(&env, EscrowError::InvalidRecipient);
        }

        let stored_issuer = record.issuer.clone();
        if stored_issuer == env.current_contract_address() || stored_issuer == pool {
            panic_with_error!(&env, EscrowError::InvalidRecipient);
        }

        let usdc = Self::usdc_client(&env);
        if record.amount > i128::MAX as u128 {
            panic_with_error!(&env, EscrowError::InvalidAmount);
        }
        usdc.transfer(
            &env.current_contract_address(),
            &stored_issuer,
            &(record.amount as i128),
        );

        Self::append_history(
            &env,
            &invoice_id,
            EscrowAction::ReleasedToIssuer,
            record.amount,
            None,
        );
        env.storage().persistent().remove(&key);
        Self::extend_instance_ttl(&env);
        events::released_to_issuer(&env, &invoice_id, &stored_issuer, record.amount);
        true
    }

    /// Releases escrowed funds back to the pool as repayment.
    ///
    /// Called by the invoice contract during buyer repayment flows:
    /// `buyer → invoice.repay → escrow.release_to_pool → pool`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice whose escrow is being released.
    /// * `repayment_amount` - The amount transferred to the pool (may exceed the
    ///   originally locked amount when the buyer repays the full face value including yield).
    ///
    /// # Auth
    /// Requires authorization from the configured pool contract.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    /// * `NotFound` if no escrow record exists for the invoice.
    /// * `InvalidAmount` if `repayment_amount` is zero.
    ///
    /// # Returns
    /// * `bool` - `true` when funds are returned.
    ///
    /// # Example
    /// ```ignore
    /// client.release_to_pool(&invoice_id, &repayment_amount);
    /// ```
    pub fn release_to_pool(env: Env, invoice_id: BytesN<32>, repayment_amount: u128) -> bool {
        require_not_paused(&env);
        let pool = Self::require_pool_auth(&env);

        if repayment_amount == 0 || repayment_amount > i128::MAX as u128 {
            panic_with_error!(&env, EscrowError::InvalidAmount);
        }

        let key = DataKey::Locked(invoice_id.clone());
        let mut record: EscrowRecord = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotFound));

        let usdc = Self::usdc_client(&env);
        usdc.transfer(
            &env.current_contract_address(),
            &pool,
            &(repayment_amount as i128),
        );

        Self::append_history(
            &env,
            &invoice_id,
            EscrowAction::ReleasedToPool,
            repayment_amount,
            None,
        );
        if repayment_amount < record.amount {
            record.amount -= repayment_amount;
            env.storage().persistent().set(&key, &record);
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        } else {
            // Buyer repayment may include yield above the original lock.
            // Preserve the existing terminal-settlement behavior in that case.
            env.storage().persistent().remove(&key);
        }
        Self::extend_instance_ttl(&env);
        events::released_to_pool(&env, &invoice_id, &pool, repayment_amount);
        true
    }

    /// Handles an escrow default by returning the locked funds to the pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice with a defaulted escrow lock.
    /// * `caller` - The address calling this function (admin or pool contract).
    ///
    /// # Auth
    /// Requires authorization from `caller`, which must equal either the stored
    /// admin address or the configured pool contract.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized and a lock record exists for the invoice.
    /// * `NotAuthorized` if `caller` is neither the admin nor the pool contract.
    /// * `NotAuthorized` if the record has not been locked long enough to satisfy the grace period.
    ///
    /// # Coupling with `invoice.trigger_default`
    /// This grace period (`DEFAULT_MIN_LOCK_SECONDS`, measured from
    /// [`EscrowRecord::locked_at`]) is independent of, and not known to,
    /// `invoice.trigger_default`'s own due-date gate (`now >= due_date`).
    /// `invoice.trigger_default` sets the invoice to `Defaulted` and then calls
    /// this function transitively via `pool.handle_default`; if an invoice's
    /// `due_date` is reached less than `DEFAULT_MIN_LOCK_SECONDS` after it was
    /// funded (i.e. after `locked_at`), this call panics with `NotAuthorized`
    /// and the whole transaction (including the invoice's status change)
    /// reverts. There is currently no mechanism for `invoice` to read or
    /// respect this window ahead of time; callers of very-short-duration
    /// invoices should expect `trigger_default` to revert until `locked_at +
    /// DEFAULT_MIN_LOCK_SECONDS` has elapsed. See
    /// `contracts/invoice/src/test.rs`'s
    /// `test_trigger_default_reverts_when_escrow_grace_period_not_elapsed` for
    /// a pinned repro of this behavior.
    ///
    /// # Returns
    /// * `bool` - `true` if default handling completed, `false` if no lock exists.
    ///
    /// # Example
    /// ```ignore
    /// let result = client.handle_default(&invoice_id, &caller);
    /// ```
    pub fn handle_default(env: Env, invoice_id: BytesN<32>, caller: Address) -> bool {
        require_not_paused(&env);
        let key = DataKey::Locked(invoice_id.clone());
        let Some(record) = env.storage().persistent().get::<_, EscrowRecord>(&key) else {
            return false;
        };
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotInitialized));
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap_or_else(|| panic_with_error!(&env, EscrowError::NotInitialized));

        caller.require_auth();
        if caller != admin && caller != pool {
            panic_with_error!(&env, EscrowError::NotAuthorized);
        }

        let now = env.ledger().timestamp();
        if now - record.locked_at < DEFAULT_MIN_LOCK_SECONDS {
            panic_with_error!(&env, EscrowError::NotAuthorized);
        }

        let usdc = Self::usdc_client(&env);
        if record.amount > i128::MAX as u128 {
            panic_with_error!(&env, EscrowError::InvalidAmount);
        }
        usdc.transfer(
            &env.current_contract_address(),
            &pool,
            &(record.amount as i128),
        );

        Self::append_history(
            &env,
            &invoice_id,
            EscrowAction::DefaultHandled,
            record.amount,
            Some(caller.clone()),
        );
        env.storage().persistent().remove(&key);
        Self::extend_instance_ttl(&env);
        events::default_resolved(&env, &invoice_id, &pool, &caller, record.amount);
        true
    }

    /// Engages the emergency circuit breaker.
    ///
    /// While paused every state-changing entry point (`lock`,
    /// `release_to_issuer`, `release_to_pool`, `handle_default`) reverts with
    /// `ContractPaused`, so no funds can move in or out of escrow while the
    /// read-only views (`get_locked`, `get_locked_at`, `get_history`,
    /// `get_usdc_asset`, `get_admin`, `get_pool_contract`) stay callable. Only
    /// the stored admin may pause, and [`Self::unpause`] is intentionally never
    /// guarded so paused escrow can always be resumed. Emits `paused`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `EscrowError::NotInitialized` if the contract has not been
    ///   initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.pause();
    /// ```
    pub fn pause(env: Env) {
        let admin = Self::require_admin(&env);
        admin.require_auth();
        set_paused(&env, true);
        events::paused(&env, &admin);
        Self::extend_instance_ttl(&env);
    }

    /// Disengages the emergency circuit breaker, restoring fund movement.
    ///
    /// Deliberately *not* guarded by `require_not_paused`: otherwise paused
    /// escrow could never be resumed. Emits `unpaused`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `EscrowError::NotInitialized` if the contract has not been
    ///   initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.unpause();
    /// ```
    pub fn unpause(env: Env) {
        let admin = Self::require_admin(&env);
        admin.require_auth();
        set_paused(&env, false);
        events::unpaused(&env, &admin);
        Self::extend_instance_ttl(&env);
    }

    /// Returns the amount of USDC currently locked against an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice ID to query.
    ///
    /// # Auth
    /// None. This is a read-only view, callable while the contract is paused.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `u128` - The locked amount, or `0` if no record exists.
    ///
    /// # Example
    /// ```ignore
    /// let locked = client.get_locked(&invoice_id);
    /// ```
    pub fn get_locked(env: Env, invoice_id: BytesN<32>) -> u128 {
        env.storage()
            .persistent()
            .get::<_, EscrowRecord>(&DataKey::Locked(invoice_id))
            .map(|r| r.amount)
            .unwrap_or(0)
    }

    /// Returns the timestamp when the escrow was locked for an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// None. This is a read-only view.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `u64` - The locked-at timestamp, or 0 if no escrow record exists.
    ///
    /// # Example
    /// ```ignore
    /// let locked_at = client.get_locked_at(&invoice_id);
    /// ```
    pub fn get_locked_at(env: Env, invoice_id: BytesN<32>) -> u64 {
        env.storage()
            .persistent()
            .get::<_, EscrowRecord>(&DataKey::Locked(invoice_id))
            .map(|r| r.locked_at)
            .unwrap_or(0)
    }

    /// Returns the retained history of escrow events for an invoice.
    ///
    /// At most [`MAX_HISTORY_EVENTS`] events are retained per invoice. Older
    /// entries may be evicted automatically or removed by the admin through
    /// [`Self::compact_history`]; full history remains available from ledger
    /// events to indexers. New integrations should use [`Self::get_history_page`]
    /// to bound the amount of returned data and storage hydrated.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// None. This is a read-only view.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Vec<EscrowEvent>` - The retained history for the invoice, oldest
    ///   first, or an empty vector if none exists.
    ///
    /// # Example
    /// ```ignore
    /// let history = client.get_history(&invoice_id);
    /// ```
    pub fn get_history(env: Env, invoice_id: BytesN<32>) -> Vec<EscrowEvent> {
        if !env
            .storage()
            .persistent()
            .has(&DataKey::HistoryCount(invoice_id.clone()))
        {
            if let Some(legacy) = env
                .storage()
                .persistent()
                .get::<_, Vec<EscrowEvent>>(&DataKey::History(invoice_id.clone()))
            {
                return legacy;
            }
        }

        let count = Self::history_count(&env, &invoice_id);
        let start = Self::history_start(&env, &invoice_id).min(count);
        let mut history = Vec::new(&env);
        let mut index = start;
        while index < count {
            if let Some(event) = Self::history_event(&env, &invoice_id, index) {
                history.push_back(event);
            }
            index += 1;
        }
        history
    }

    /// Returns one bounded page of escrow history for an invoice.
    ///
    /// `cursor` is the zero-based append index of the first event to return;
    /// pass `next_cursor` to continue a walk. A cursor older than the retained
    /// history advances to the first retained append index. Appends and pruning
    /// never renumber surviving events, so pages do not duplicate or skip
    /// retained entries when partial releases, re-locks, or compaction occur.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    /// * `cursor` - Zero-based append index. Must be `<=` the current event
    ///   count; a value below the retention start begins at the oldest retained
    ///   event, while `cursor == count` returns an empty final page.
    /// * `limit` - Maximum events to return. Must be in
    ///   `1..=MAX_HISTORY_LIMIT`.
    ///
    /// # Auth
    /// None. This is a read-only view.
    ///
    /// # Panics
    /// * [`EscrowError::InvalidLimit`] if `limit == 0` or
    ///   `limit > MAX_HISTORY_LIMIT`.
    /// * [`EscrowError::InvalidCursor`] if `cursor` is greater than the number
    ///   of recorded events.
    ///
    /// # Returns
    /// * [`HistoryPage`] - Up to `limit` events plus the cursor for the next
    ///   page, or `None` when this is the final page.
    ///
    /// # Example
    /// ```ignore
    /// let page = client.get_history_page(&invoice_id, &0, &25);
    /// if let Some(cursor) = page.next_cursor {
    ///     let next = client.get_history_page(&invoice_id, &cursor, &25);
    /// }
    /// ```
    pub fn get_history_page(
        env: Env,
        invoice_id: BytesN<32>,
        cursor: u32,
        limit: u32,
    ) -> HistoryPage {
        if limit == 0 || limit > MAX_HISTORY_LIMIT {
            panic_with_error!(&env, EscrowError::InvalidLimit);
        }

        let count = Self::history_count(&env, &invoice_id);
        if cursor > count {
            panic_with_error!(&env, EscrowError::InvalidCursor);
        }

        // Retention may have pruned entries older than this cursor. Advance a
        // stale cursor to the first retained append index without shifting the
        // indexes of any surviving event.
        let start = Self::history_start(&env, &invoice_id).min(count);
        let cursor = cursor.max(start);
        let end = cursor.saturating_add(limit).min(count);
        let mut items = Vec::new(&env);
        let mut index = cursor;
        while index < end {
            if let Some(event) = Self::history_event(&env, &invoice_id, index) {
                items.push_back(event);
            }
            index += 1;
        }

        let next_cursor = if end < count { Some(end) } else { None };
        HistoryPage { items, next_cursor }
    }

    fn history_count(env: &Env, invoice_id: &BytesN<32>) -> u32 {
        let key = DataKey::HistoryCount(invoice_id.clone());
        if let Some(count) = env.storage().persistent().get::<_, u32>(&key) {
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
            return count;
        }
        env.storage()
            .persistent()
            .get::<_, Vec<EscrowEvent>>(&DataKey::History(invoice_id.clone()))
            .map(|history| history.len())
            .unwrap_or(0)
    }

    fn history_start(env: &Env, invoice_id: &BytesN<32>) -> u32 {
        let key = DataKey::HistoryStart(invoice_id.clone());
        if let Some(start) = env.storage().persistent().get::<_, u32>(&key) {
            env.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
            start
        } else {
            0
        }
    }

    fn history_event(env: &Env, invoice_id: &BytesN<32>, index: u32) -> Option<EscrowEvent> {
        let key = DataKey::HistoryEvent(invoice_id.clone(), index);
        if let Some(event) = env.storage().persistent().get::<_, EscrowEvent>(&key) {
            env.storage()
                .persistent()
                .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
            return Some(event);
        }
        env.storage()
            .persistent()
            .get::<_, Vec<EscrowEvent>>(&DataKey::History(invoice_id.clone()))
            .and_then(|history| history.get(index))
    }

    /// Prunes the oldest stored history events for an invoice down to `keep`.
    ///
    /// This is the manual half of the history compaction policy. Appends are
    /// automatically capped at [`MAX_HISTORY_EVENTS`]; this entry point lets
    /// the admin tighten retention further and drain histories written before
    /// the cap existed. Removal is bounded to [`MAX_COMPACT_PER_CALL`] entries
    /// per call, so an oversized legacy history may require repeated calls
    /// until the return value is `0`.
    ///
    /// The newest `keep` events are retained in their original order. Pruned
    /// events are not erased from the ledger's event stream: indexers that
    /// ingested `funds_locked`, `released_to_issuer`, `released_to_pool` and
    /// `default_resolved` still have the complete audit trail. Emits
    /// `history_compacted(invoice_id, retained, removed)` when anything is
    /// removed.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice whose history should be compacted.
    /// * `keep` - Number of most-recent events to retain. Must be in
    ///   [`MIN_HISTORY_EVENTS`]`..=`[`MAX_HISTORY_EVENTS`].
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`. Compaction is
    /// destructive for on-chain reads, so it is deliberately admin-gated
    /// rather than permissionless; a permissionless caller could make audit
    /// data inaccessible to clients before they had a chance to sync.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    /// * `InvalidRetention` if `keep` is outside the supported range.
    /// * `NotFound` if no history exists for the invoice.
    ///
    /// # Returns
    /// * `u32` - Number of events removed from contract storage.
    ///
    /// # Example
    /// ```ignore
    /// let removed = client.compact_history(&invoice_id, &8);
    /// ```
    pub fn compact_history(env: Env, invoice_id: BytesN<32>, keep: u32) -> u32 {
        if !(MIN_HISTORY_EVENTS..=MAX_HISTORY_EVENTS).contains(&keep) {
            panic_with_error!(&env, EscrowError::InvalidRetention);
        }

        let admin = Self::require_admin(&env);
        admin.require_auth();

        let had_legacy = env.storage().persistent().has(&DataKey::History(invoice_id.clone()));
        let count_key = DataKey::HistoryCount(invoice_id.clone());
        let count = match env.storage().persistent().get::<_, u32>(&count_key) {
            Some(count) => count,
            None if had_legacy => Self::migrate_legacy_history(&env, &invoice_id),
            None => panic_with_error!(&env, EscrowError::NotFound),
        };
        if count == 0 && !had_legacy {
            panic_with_error!(&env, EscrowError::NotFound);
        }

        let start_key = DataKey::HistoryStart(invoice_id.clone());
        let mut start = env.storage().persistent().get::<_, u32>(&start_key).unwrap_or(0);
        let retained = count.saturating_sub(start);
        let to_remove = retained.saturating_sub(keep).min(MAX_COMPACT_PER_CALL);
        for index in start..start.saturating_add(to_remove) {
            env.storage().persistent().remove(&DataKey::HistoryEvent(invoice_id.clone(), index));
        }
        start = start.saturating_add(to_remove);
        if to_remove > 0 {
            env.storage().persistent().set(&start_key, &start);
            env.storage().persistent().extend_ttl(&start_key, TTL_THRESHOLD, TTL_EXTEND_TO);
            env.storage().persistent().extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
            events::history_compacted(&env, &invoice_id, count.saturating_sub(start), to_remove);
        }
        Self::extend_instance_ttl(&env);
        to_remove
    }

    fn append_history(
        env: &Env,
        invoice_id: &BytesN<32>,
        action: EscrowAction,
        amount: u128,
        caller: Option<Address>,
    ) {
        let count_key = DataKey::HistoryCount(invoice_id.clone());
        let mut count = match env.storage().persistent().get::<_, u32>(&count_key) {
            Some(count) => count,
            None => Self::migrate_legacy_history(env, invoice_id),
        };
        let start_key = DataKey::HistoryStart(invoice_id.clone());
        let mut start = env.storage().persistent().get::<_, u32>(&start_key).unwrap_or(0);

        let event_key = DataKey::HistoryEvent(invoice_id.clone(), count);
        env.storage().persistent().set(
            &event_key,
            &EscrowEvent {
                invoice_id: invoice_id.clone(),
                action,
                amount,
                timestamp: env.ledger().timestamp(),
                caller,
            },
        );
        env.storage().persistent().extend_ttl(&event_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        count = count.saturating_add(1);

        let mut removed = 0u32;
        while count.saturating_sub(start) > MAX_HISTORY_EVENTS {
            env.storage().persistent().remove(&DataKey::HistoryEvent(invoice_id.clone(), start));
            start = start.saturating_add(1);
            removed = removed.saturating_add(1);
        }
        env.storage().persistent().set(&count_key, &count);
        env.storage().persistent().extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        if start > 0 {
            env.storage().persistent().set(&start_key, &start);
            env.storage().persistent().extend_ttl(&start_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        }
        if removed > 0 {
            events::history_compacted(env, invoice_id, count.saturating_sub(start), removed);
        }
    }

    fn migrate_legacy_history(env: &Env, invoice_id: &BytesN<32>) -> u32 {
        let legacy_key = DataKey::History(invoice_id.clone());
        let Some(legacy) = env.storage().persistent().get::<_, Vec<EscrowEvent>>(&legacy_key) else {
            return 0;
        };
        let mut index = 0;
        while index < legacy.len() {
            if let Some(event) = legacy.get(index) {
                let event_key = DataKey::HistoryEvent(invoice_id.clone(), index);
                env.storage().persistent().set(&event_key, &event);
                env.storage().persistent().extend_ttl(&event_key, TTL_THRESHOLD, TTL_EXTEND_TO);
            }
            index += 1;
        }
        let count = legacy.len();
        env.storage().persistent().set(&DataKey::HistoryCount(invoice_id.clone()), &count);
        env.storage().persistent().extend_ttl(&DataKey::HistoryCount(invoice_id.clone()), TTL_THRESHOLD, TTL_EXTEND_TO);
        env.storage().persistent().remove(&legacy_key);
        count
    }

    fn extend_instance_ttl(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    fn require_pool_auth(env: &Env) -> Address {
        let pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap_or_else(|| panic_with_error!(env, EscrowError::NotInitialized));
        pool.require_auth();
        pool
    }

    fn require_admin(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(env, EscrowError::NotInitialized))
    }
}
