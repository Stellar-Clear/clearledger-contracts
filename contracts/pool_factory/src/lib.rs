#![no_std]

use soroban_sdk::{
    contract, contractimpl, panic_with_error, vec, xdr::ToXdr, Address, BytesN, Env, IntoVal,
    String, Symbol, Vec,
};

mod constants;
mod errors;
mod events;
mod types;

#[cfg(test)]
mod test;

pub use constants::*;
pub use errors::*;
pub use types::*;

/// Function name of the pool contract's initializer, invoked on every instance
/// this factory deploys.
const POOL_INITIALIZE: &str = "initialize";
/// Function name of the invoice contract's registry getter, used by
/// `register_asset` to resolve the registry a new pool instance must verify
/// issuer/buyer profiles against.
const INVOICE_GET_REGISTRY: &str = "get_registry_contract";

#[contract]
pub struct PoolFactoryContract;

#[contractimpl]
impl PoolFactoryContract {
    /// Initializes the pool factory with an admin address.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The admin address for this contract.
    ///
    /// # Auth
    /// Requires authorization from `admin`.
    ///
    /// # Panics
    /// * `AlreadyInitialized` if the contract has already been initialized.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.initialize(&admin);
    /// ```
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, PoolFactoryError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::AssetCount, &0u32);
        events::contract_initialized(&env, &admin);
    }

    /// Transfers factory admin ownership to `new_admin`.
    ///
    /// The factory admin is the trust anchor for the pools it manages: it is
    /// the only address allowed to register assets and is copied into every
    /// newly deployed pool as that pool's admin and treasury. Moving it is
    /// therefore deliberately a two-party operation.
    ///
    /// # Auth
    /// Requires authorization from both the current admin (read from storage)
    /// and `new_admin`. Requiring the incoming admin to sign prevents ownership
    /// from being forced onto an address that never consented (for example a
    /// typo), while requiring the current admin prevents anyone else from
    /// seizing the factory. Neither signature alone is sufficient. After the
    /// call succeeds every admin-gated entry point (`register_asset`,
    /// `register_existing_pool`, and a subsequent `transfer_ownership`) checks
    /// the *new* admin, so the old key has no residual authority.
    ///
    /// # Panics
    /// * `NotInitialized` if the factory has not been initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.transfer_ownership(&new_admin);
    /// ```
    pub fn transfer_ownership(env: Env, new_admin: Address) {
        let admin = Self::admin(&env);
        admin.require_auth();
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::ownership_transferred(&env, &admin, &new_admin);
    }

    /// Deploys a new pool instance for an asset and records it as that asset's
    /// pool.
    ///
    /// This is the factory's core function: it deploys a fresh instance of the
    /// pool Wasm identified by `pool_wasm_hash`, initializes it for `asset`,
    /// and records the asset -> pool mapping so `get_pool_for_asset` (and the
    /// invoice contract, the frontend, and any other integrator) can find the
    /// instance later.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset the new pool instance custodies. Passed to the
    ///   pool as its `funding_asset`, the single asset that instance supports.
    ///   For native XLM this must be the network's Stellar Asset Contract
    ///   address (Testnet
    ///   `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`), never
    ///   the string `native` or an issuer account. See `docs/NATIVE_XLM.md`.
    /// * `pool_wasm_hash` - Hash of the uploaded pool Wasm to deploy. Must
    ///   already be present in the ledger.
    /// * `invoice_contract` - The invoice contract the new pool funds invoices
    ///   from. Must be initialized.
    /// * `escrow_contract` - The escrow contract the new pool locks and settles
    ///   invoice funds through. Must already be initialized with the same
    ///   `asset`, since `pool::initialize` cross-checks the escrow's configured
    ///   asset against its own.
    ///
    /// # Auth
    /// Requires authorization from the admin stored at `initialize`. The
    /// deployment needs no authorization of its own: the new instance's
    /// deployer is this factory, which the caller has already authorized by
    /// invoking `register_asset`.
    ///
    /// # Instance identity
    /// The instance is deployed with this factory as the deploying contract and
    /// a salt derived from `asset`, so an asset's pool address is deterministic
    /// and independent of the deployment order. Combined with the
    /// already-registered guard, a second `register_asset` call for the same
    /// asset can never orphan the first instance.
    ///
    /// # Registry wiring
    /// The new pool's registry contract is not a parameter: it is read back from
    /// `invoice_contract` (`get_registry_contract`). The pool re-verifies
    /// issuer/buyer profiles in `fund_invoice` against the same registry the
    /// invoice contract verified them against, so deriving it keeps the two
    /// contracts from disagreeing and removes a whole class of misconfiguration
    /// (a pool pointed at a registry that is not the one its invoices were
    /// checked against would reject invoices the invoice contract had already
    /// accepted). The factory's admin becomes the new pool's admin and its
    /// initial treasury, which `pool::initialize` explicitly allows.
    ///
    /// # Panics
    /// * `NotInitialized` if the factory has not been initialized.
    /// * `AssetAlreadyRegistered` if the asset already has a pool. Register an
    ///   asset once only; there is no replace semantics.
    /// * `InvoiceNotInitialized` if `invoice_contract` does not report a
    ///   registry contract.
    /// * `InvalidAsset` if `asset` aliases the factory admin, the factory
    ///   itself, or another wired address.
    /// * `InvalidContractAddress` if `invoice_contract` or `escrow_contract`
    ///   aliases another configured role. Both checks run before deployment,
    ///   so an invalid registration never orphans a deployed instance.
    /// * `PoolError::InvalidContractAddress` / `PoolError::InvalidAsset` if the
    ///   pool rejects the resulting address set (for example if the factory
    ///   admin is also one of the invoice/escrow/registry addresses).
    /// * `PoolError::EscrowAssetMismatch` if `escrow_contract` was initialized
    ///   with a different asset than `asset`.
    ///
    /// # Returns
    /// * `Address` - The address of the newly deployed pool instance.
    ///
    /// # Example
    /// ```ignore
    /// client.register_asset(&asset, &pool_wasm_hash, &invoice, &escrow);
    /// ```
    pub fn register_asset(
        env: Env,
        asset: Address,
        pool_wasm_hash: BytesN<32>,
        invoice_contract: Address,
        escrow_contract: Address,
    ) -> Address {
        let admin = Self::admin(&env);
        admin.require_auth();
        Self::assert_unregistered(&env, &asset);
        Self::assert_valid_registration_addresses(
            &env,
            &admin,
            &asset,
            &invoice_contract,
            &escrow_contract,
        );

        // Deploy the instance with this factory as the deploying contract. The
        // deployer address plus the salt fix the instance address, so the
        // address is known to the caller before the transaction is even built.
        let pool_address = env
            .deployer()
            .with_current_contract(Self::asset_salt(&env, &asset))
            .deploy(pool_wasm_hash);

        Self::initialize_pool(
            &env,
            &admin,
            &asset,
            &pool_address,
            &invoice_contract,
            &escrow_contract,
        );

        // Recorded last, so a lookup can never observe a pool that has been
        // deployed but not yet initialized.
        Self::record_pool(&env, &asset, &pool_address);
        events::pool_instance_created(&env, &asset, &pool_address);
        pool_address
    }

    /// Registers an existing pool contract for a given asset.
    ///
    /// This function is used for migration scenarios where a pool contract
    /// already exists and needs to be registered under the factory without
    /// deploying a new instance.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset address to register. For native XLM, pass the
    ///   network's Stellar Asset Contract address; see `register_asset`.
    /// * `pool_address` - The existing pool contract address.
    ///
    /// # Auth
    /// Requires authorization from the admin.
    ///
    /// # Panics
    /// * `NotInitialized` if the factory has not been initialized.
    /// * `AssetAlreadyRegistered` if the asset is already registered.
    /// * `InvalidAsset` if `asset` aliases the factory admin, the factory
    ///   itself, or `pool_address`.
    /// * `InvalidContractAddress` if `pool_address` aliases the factory admin
    ///   or the factory itself.
    /// * `InvalidPool` if `pool_address` does not expose a pool initialized
    ///   for `asset`.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.register_existing_pool(&usdc, &existing_pool);
    /// ```
    pub fn register_existing_pool(env: Env, asset: Address, pool_address: Address) {
        let admin = Self::admin(&env);
        admin.require_auth();
        Self::assert_unregistered(&env, &asset);
        if pool_address == admin || pool_address == env.current_contract_address() {
            panic_with_error!(&env, PoolFactoryError::InvalidContractAddress);
        }
        if asset == admin
            || asset == env.current_contract_address()
            || asset == pool_address
        {
            panic_with_error!(&env, PoolFactoryError::InvalidAsset);
        }

        let pool_funding_asset = env.try_invoke_contract::<Address, soroban_sdk::Error>(
            &pool_address,
            &Symbol::new(&env, "get_funding_asset"),
            Vec::new(&env),
        );
        if !matches!(pool_funding_asset, Ok(Ok(ref funding_asset)) if funding_asset == &asset) {
            panic_with_error!(&env, PoolFactoryError::InvalidPool);
        }

        Self::record_pool(&env, &asset, &pool_address);
        events::existing_pool_registered(&env, &asset, &pool_address);
    }

    /// Returns the pool instance the factory manages for an asset.
    ///
    /// This is the read-only lookup that callers (the invoice contract, the
    /// frontend, other integrators) use to find an asset's pool without
    /// re-deploying it or guessing addresses. It is the one lookup that covers
    /// both registration paths, since `register_asset` and
    /// `register_existing_pool` write the same key.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The asset to look up.
    ///
    /// # Auth
    /// No authorization is required (read-only view).
    ///
    /// # Panics
    /// Does not panic: an unregistered asset simply has no pool.
    ///
    /// # Returns
    /// * `Option<Address>` - The asset's pool instance, or `None` if the asset
    ///   has not been registered.
    ///
    /// # Example
    /// ```ignore
    /// let pool = client.get_pool_for_asset(&usdc);
    /// ```
    pub fn get_pool_for_asset(env: Env, asset: Address) -> Option<Address> {
        env.storage().instance().get(&DataKey::PoolForAsset(asset))
    }

    /// Returns aggregated statistics across all registered pool instances.
    ///
    /// **Deprecated for new integrations:** this view is unbounded. It walks
    /// every registered asset and makes one cross-contract call per asset in a
    /// single invocation, so its resource use grows with the registry and can
    /// exceed the Soroban per-call budget. Use
    /// [`Self::get_aggregate_stats_page`] instead and walk the result with a
    /// cursor. This function is retained for ABI compatibility and returns the
    /// same registration order the page API exposes.
    ///
    /// Iterates through all registered assets, queries each pool instance's
    /// `get_stats`, and returns a vector pairing each responsive pool address
    /// with its current `PoolStats`. Pools whose call fails are skipped so one
    /// unavailable or invalid pool cannot block stats for all other assets.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required (read-only view).
    ///
    /// # Returns
    /// * `Vec<(Address, PoolStats)>` - A list of (Pool Address, PoolStats) tuples.
    ///
    /// # Cost
    /// Resource consumption scales linearly with the number of registered assets,
    /// as a cross-contract call is made for each instance.
    pub fn get_aggregate_stats(env: Env) -> Vec<(Address, PoolStats)> {
        let assets = Self::list_assets(env.clone());
        let mut aggregate = Vec::new(&env);
        for asset in assets {
            if let Some(pool_address) = Self::get_pool_for_asset(env.clone(), asset) {
                let result = env.try_invoke_contract::<PoolStats, soroban_sdk::Error>(
                    &pool_address,
                    &Symbol::new(&env, "get_stats"),
                    Vec::new(&env),
                );
                if let Ok(Ok(stats)) = result {
                    aggregate.push_back((pool_address, stats));
                }
            }
        }
        aggregate
    }

    /// Returns a bounded page of aggregate statistics across registered pools.
    ///
    /// This is the preferred aggregate-stats read. It walks at most `limit`
    /// entries of the asset index, queries each mapped pool instance's
    /// `get_stats`, and returns the responsive pools' stats together with a
    /// cursor for the next page. Pools whose call fails are skipped, exactly as
    /// in [`Self::get_aggregate_stats`], so one unavailable or invalid pool
    /// cannot block the rest of the page.
    ///
    /// # Iteration semantics
    /// The asset index is append-only. Both [`Self::register_asset`] and
    /// [`Self::register_existing_pool`] append the newly registered asset at
    /// index `AssetCount`, then increment `AssetCount`, so registration order is
    /// stable: an asset's cursor never changes and is never reused.
    ///
    /// Start at `cursor = 0` and keep passing the returned `next_cursor` until
    /// it is `None`. A `next_cursor` of `Some(next)` means at least one more
    /// asset exists at index `next`; it is also the number of asset-index
    /// entries already walked. A registration that lands between two page reads
    /// is appended after the last cursor already returned, so it appears at the
    /// end of the walk and cannot shift, duplicate, or skip an item already
    /// read. Calling with `cursor` equal to the current asset count returns an
    /// empty final page (`next_cursor == None`).
    ///
    /// The cursor tracks *asset-index entries*, not returned items. A page that
    /// skips unresponsive pools can therefore contain fewer than `limit` items
    /// while still advancing `next_cursor` by the full number of entries
    /// walked (capped at the current asset count). This mirrors the unbounded
    /// view's skip-on-failure behavior without ever retrying an entry.
    /// Each invocation reads at most
    /// [`MAX_AGGREGATE_STATS_PAGE_LIMIT`] index entries and makes at most that
    /// many cross-contract calls.
    ///
    /// # ABI
    /// `get_aggregate_stats_page(cursor: u32, limit: u32) -> AggregateStatsPage`,
    /// where [`AggregateStatsPage`] is
    /// `{ items: Vec<(Address, PoolStats)>, next_cursor: Option<u32> }`. Both
    /// arguments and the result are encoded as Soroban contract types;
    /// `next_cursor` is a 32-bit asset index, not an opaque token.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `cursor` - Zero-based index of the first asset to walk. Must be `<=`
    ///   the current asset count; `cursor == count` returns an empty page.
    /// * `limit` - Maximum number of asset-index entries to walk. Must be in
    ///   `1..=MAX_AGGREGATE_STATS_PAGE_LIMIT`.
    ///
    /// # Auth
    /// No authorization is required (read-only view).
    ///
    /// # Panics
    /// * `InvalidAggregateStatsLimit` if `limit == 0` or
    ///   `limit > MAX_AGGREGATE_STATS_PAGE_LIMIT`.
    /// * `InvalidAggregateStatsCursor` if `cursor` is greater than the current
    ///   asset count.
    ///
    /// # Returns
    /// * [`AggregateStatsPage`] - Up to `limit` (pool address, stats) pairs in
    ///   registration order plus the cursor for the next page, or `None` when
    ///   this is the final page.
    ///
    /// # Example
    /// ```ignore
    /// let mut cursor = 0;
    /// loop {
    ///     let page = client.get_aggregate_stats_page(&cursor, &50);
    ///     for (pool, stats) in page.items.iter() {
    ///         // use pool and stats
    ///     }
    ///     match page.next_cursor {
    ///         Some(next) => cursor = next,
    ///         None => break,
    ///     }
    /// }
    /// ```
    pub fn get_aggregate_stats_page(env: Env, cursor: u32, limit: u32) -> AggregateStatsPage {
        if limit == 0 || limit > MAX_AGGREGATE_STATS_PAGE_LIMIT {
            panic_with_error!(&env, PoolFactoryError::InvalidAggregateStatsLimit);
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::AssetCount)
            .unwrap_or(0);
        if cursor > count {
            panic_with_error!(&env, PoolFactoryError::InvalidAggregateStatsCursor);
        }

        let end = cursor.saturating_add(limit).min(count);
        let mut items = Vec::new(&env);
        let mut index = cursor;
        while index < end {
            let asset: Option<Address> = env.storage().instance().get(&DataKey::AssetIndex(index));
            if let Some(asset) = asset {
                if let Some(pool_address) = Self::get_pool_for_asset(env.clone(), asset) {
                    let result = env.try_invoke_contract::<PoolStats, soroban_sdk::Error>(
                        &pool_address,
                        &Symbol::new(&env, "get_stats"),
                        Vec::new(&env),
                    );
                    if let Ok(Ok(stats)) = result {
                        items.push_back((pool_address, stats));
                    }
                }
            }
            index += 1;
        }

        let next_cursor = if end < count { Some(end) } else { None };
        AggregateStatsPage { items, next_cursor }
    }

    /// Lists a bounded page of registered assets.
    ///
    /// This is the preferred asset-list read. It replaces [`Self::list_assets`]
    /// for integrations: the asset index is stored in instance storage, so the
    /// unbounded view's cost grows with every registration and will eventually
    /// exceed the Soroban per-call budget.
    ///
    /// # Iteration semantics
    /// The asset index is append-only. Both `register_asset` and
    /// `register_existing_pool` append the newly registered asset at index
    /// `AssetCount`, then increment `AssetCount`, so registration order is
    /// stable: an asset's cursor never changes and is never reused.
    ///
    /// Start at `cursor = 0` and keep passing the returned `next_cursor` until
    /// it is `None`. A `next_cursor` of `Some(next)` means at least one more
    /// asset exists at index `next`; it is also the number of items already
    /// walked. A registration that lands between two page reads is appended
    /// after the last cursor already returned, so it appears at the end of the
    /// walk and cannot shift, duplicate, or skip an item already read. Calling
    /// with `cursor` equal to the current asset count returns an empty final
    /// page (`next_cursor == None`). Each invocation reads at most
    /// [`MAX_ASSET_PAGE_LIMIT`] index entries.
    ///
    /// # ABI
    /// `get_assets_page(cursor: u32, limit: u32) -> AssetPage`, where
    /// [`AssetPage`] is `{ items: Vec<Address>, next_cursor: Option<u32> }`.
    /// Both arguments and the result are encoded as Soroban contract types;
    /// `next_cursor` is a 32-bit index, not an opaque token.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `cursor` - Zero-based index of the first asset to return. Must be
    ///   `<=` the current asset count; `cursor == count` returns an empty page.
    /// * `limit` - Maximum number of assets to return. Must be in
    ///   `1..=MAX_ASSET_PAGE_LIMIT`.
    ///
    /// # Auth
    /// No authorization is required (read-only view).
    ///
    /// # Panics
    /// * `InvalidLimit` if `limit == 0` or
    ///   `limit > MAX_ASSET_PAGE_LIMIT`.
    /// * `InvalidCursor` if `cursor` is greater than the current asset count.
    ///
    /// # Returns
    /// * [`AssetPage`] - Up to `limit` assets in registration order plus the
    ///   cursor for the next page, or `None` when this is the final page.
    ///
    /// # Example
    /// ```ignore
    /// let mut cursor = 0;
    /// loop {
    ///     let page = client.get_assets_page(&cursor, &50);
    ///     for asset in page.items.iter() {
    ///         // use asset
    ///     }
    ///     match page.next_cursor {
    ///         Some(next) => cursor = next,
    ///         None => break,
    ///     }
    /// }
    /// ```
    pub fn get_assets_page(env: Env, cursor: u32, limit: u32) -> AssetPage {
        if limit == 0 || limit > MAX_ASSET_PAGE_LIMIT {
            panic_with_error!(&env, PoolFactoryError::InvalidLimit);
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::AssetCount)
            .unwrap_or(0);
        if cursor > count {
            panic_with_error!(&env, PoolFactoryError::InvalidCursor);
        }

        let end = cursor.saturating_add(limit).min(count);
        let mut items = Vec::new(&env);
        let mut index = cursor;
        while index < end {
            if let Some(asset) = env.storage().instance().get(&DataKey::AssetIndex(index)) {
                items.push_back(asset);
            }
            index += 1;
        }

        let next_cursor = if end < count { Some(end) } else { None };
        AssetPage { items, next_cursor }
    }

    /// Lists all registered assets.
    ///
    /// **Deprecated for new integrations:** this view is unbounded, so its
    /// cost grows with the number of registered assets and can exceed the
    /// Soroban per-call budget. Use [`Self::get_assets_page`] instead and walk
    /// the list with a cursor. This function is retained for ABI compatibility
    /// and returns the same registration order the page API exposes.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required (read-only view).
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Vec<Address>` - A vector of all registered asset addresses.
    ///
    /// # Example
    /// ```ignore
    /// let assets = client.list_assets();
    /// ```
    pub fn list_assets(env: Env) -> Vec<Address> {
        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::AssetCount)
            .unwrap_or(0);

        let mut assets = Vec::new(&env);
        for i in 0..count {
            if let Some(asset) = env.storage().instance().get(&DataKey::AssetIndex(i)) {
                assets.push_back(asset);
            }
        }
        assets
    }

    /// Returns the stored admin, panicking if the factory is not initialized.
    fn admin(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(env, PoolFactoryError::NotInitialized))
    }

    /// Rejects registration address sets that alias a token id with a
    /// contract/admin role, or one contract role with another. Validated
    /// before deploy/storage so a rejected call leaves no orphaned instance.
    fn assert_valid_registration_addresses(
        env: &Env,
        admin: &Address,
        asset: &Address,
        invoice_contract: &Address,
        escrow_contract: &Address,
    ) {
        if asset == admin
            || asset == &env.current_contract_address()
            || asset == invoice_contract
            || asset == escrow_contract
        {
            panic_with_error!(env, PoolFactoryError::InvalidAsset);
        }
        if invoice_contract == admin
            || invoice_contract == &env.current_contract_address()
            || escrow_contract == admin
            || escrow_contract == &env.current_contract_address()
            || invoice_contract == escrow_contract
        {
            panic_with_error!(env, PoolFactoryError::InvalidContractAddress);
        }
    }

    /// Panics if `asset` already has a pool registered.
    ///
    /// Guards both registration paths so neither can silently repoint an
    /// asset at a different pool and orphan the instance (and any LP position)
    /// behind the previous mapping.
    fn assert_unregistered(env: &Env, asset: &Address) {
        if env
            .storage()
            .instance()
            .has(&DataKey::PoolForAsset(asset.clone()))
        {
            panic_with_error!(env, PoolFactoryError::AssetAlreadyRegistered);
        }
    }

    /// Records `pool_address` as `asset`'s pool and appends `asset` to the
    /// enumeration index used by `list_assets` and
    /// `get_aggregate_stats_page`.
    ///
    /// The append order is what makes the aggregate-stats page API's cursors
    /// stable: an existing index entry is never moved or rewritten.
    fn record_pool(env: &Env, asset: &Address, pool_address: &Address) {
        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::AssetCount)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::AssetCount, &(count + 1));
        env.storage()
            .instance()
            .set(&DataKey::AssetIndex(count), asset);
        env.storage()
            .instance()
            .set(&DataKey::PoolForAsset(asset.clone()), pool_address);
    }

    /// Derives the per-asset deploy salt.
    ///
    /// Hashing the asset's XDR encoding keeps the salt deterministic (so the
    /// pool address for an asset is stable and predictable off-chain) while
    /// staying distinct for every asset, including account-addressed assets
    /// that do not fit in a 32-byte contract address.
    fn asset_salt(env: &Env, asset: &Address) -> BytesN<32> {
        let digest = env.crypto().sha256(&asset.clone().to_xdr(env));
        BytesN::from_array(env, &digest.to_array())
    }

    /// Initializes a freshly deployed pool instance.
    ///
    /// Args are passed positionally to `pool::initialize`, whose signature is
    /// `(admin, invoice_contract, escrow_contract, funding_asset,
    /// registry_contract, treasury, min_initial_deposit, share_name,
    /// share_symbol, share_decimals)`.
    ///
    /// The three share-metadata args are the factory-wide defaults documented
    /// on `DEFAULT_SHARE_NAME`: `register_asset` is not given per-asset names,
    /// so there is nothing more accurate to pass here.
    fn initialize_pool(
        env: &Env,
        admin: &Address,
        asset: &Address,
        pool_address: &Address,
        invoice_contract: &Address,
        escrow_contract: &Address,
    ) {
        let registry_contract: Option<Address> = env.invoke_contract(
            invoice_contract,
            &Symbol::new(env, INVOICE_GET_REGISTRY),
            Vec::new(env),
        );
        let registry_contract = registry_contract
            .unwrap_or_else(|| panic_with_error!(env, PoolFactoryError::InvoiceNotInitialized));

        let args = vec![
            env,
            admin.to_val(),
            invoice_contract.to_val(),
            escrow_contract.to_val(),
            asset.to_val(),
            registry_contract.to_val(),
            admin.to_val(),
            DEFAULT_MIN_INITIAL_DEPOSIT.into_val(env),
            String::from_str(env, DEFAULT_SHARE_NAME).to_val(),
            String::from_str(env, DEFAULT_SHARE_SYMBOL).to_val(),
            DEFAULT_SHARE_DECIMALS.into_val(env),
        ];
        env.invoke_contract::<()>(pool_address, &Symbol::new(env, POOL_INITIALIZE), args);
    }
}
