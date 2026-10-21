#![no_std]

use clearledger_pause::{require_not_paused, set_paused};
use soroban_sdk::{
    contract, contractimpl, panic_with_error, token,
    xdr::{FromXdr, ToXdr},
    Address, Bytes, BytesN, Env, IntoVal, Map, String, Symbol, Vec,
};

mod constants;
mod errors;
mod events;
mod test;
mod types;

pub use constants::*;
pub use errors::*;
pub use types::*;

/// Upper bound on `Invoice::face_value`, in USDC stroops.
///
/// Chosen so that `face_value * 10_000` (the scaling factor used by
/// downstream discount/utilization math in the pool contract, e.g.
/// `face_value * (10_000 - discount_bps) / 10_000`) can never overflow
/// `u128`, preventing arithmetic overflow in the pool when it consumes
/// this value.
pub const MAX_FACE_VALUE: u128 = u128::MAX / 10_000;

/// Maximum allowed invoice lifetime in seconds.
///
/// This caps `due_date` to `now + MAX_INVOICE_LIFETIME_SECONDS` to reject
/// far-future due dates that are effectively garbage data (e.g., centuries
/// in the future). The value is ~10 years (10 * 365 * 24 * 60 * 60).
pub const MAX_INVOICE_LIFETIME_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;

/// Fixed 32-byte domain separator that Underwrite agents must include in
/// the [`AttestationPayload`] they sign. Binds a signature to this
/// contract's attestation scheme specifically, so it can't be replayed
/// against an unrelated contract or message format.
pub const ATTESTATION_DOMAIN_SEPARATOR: [u8; 32] = *b"ClearLedger.InvoiceAttest.v1____";

/// Maximum number of invoices a single paginated `get_by_*` query may return.
///
/// Pagination exists to keep each read within the Soroban per-transaction
/// CPU/memory budget (issue #71): without a hard cap a caller could pass a
/// huge `page_size` and hydrate the entire index in one call, defeating the
/// point. `50` mirrors the workspace's existing batch ceiling
/// (`batch_register_issuers` caps at 50 entries per call).
///
/// Requests with `page_size > MAX_PAGE_SIZE` panic with
/// [`InvoiceError::InvalidPageSize`].
pub const MAX_PAGE_SIZE: u32 = 50;

/// Maximum number of entries accepted by `batch_create` and
/// `batch_list_for_financing`.
///
/// Mirrors `RegistryContract::batch_register_issuers`'s 50-entry cap. Each
/// `batch_create` entry performs registry verification cross-contract calls,
/// so an unbounded `Vec` would let one transaction exceed the per-call budget.
/// Requests over the cap panic with [`InvoiceError::BatchSizeExceeded`].
pub const MAX_BATCH_SIZE: u32 = 50;

/// Default listing expiry window in seconds (7 days), used when the admin has
/// not called `set_expiry_window`.
pub const DEFAULT_EXPIRY_WINDOW: u64 = 7 * 24 * 60 * 60;

#[contract]
pub struct InvoiceContract;

#[contractimpl]
impl InvoiceContract {
    fn save_invoice(env: &Env, inv_key: DataKey, invoice: &Invoice) {
        env.storage().persistent().set(&inv_key, invoice);
        env.storage()
            .persistent()
            .extend_ttl(&inv_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    fn expiry_window(env: &Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::ExpiryWindow)
            .unwrap_or(DEFAULT_EXPIRY_WINDOW)
    }

    fn listing_expiry_timestamp(env: &Env, listed_at: u64) -> u64 {
        Self::checked_listing_expiry_timestamp(env, listed_at)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::MathOverflow))
    }

    fn checked_listing_expiry_timestamp(env: &Env, listed_at: u64) -> Option<u64> {
        listed_at.checked_add(Self::expiry_window(env))
    }

    /// Initializes the invoice contract with admin and registry references.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `admin` - The admin address for this contract.
    /// * `registry_contract` - The deployed registry contract address.
    ///
    /// # Auth
    /// Requires authorization from `admin`.
    ///
    /// # Panics
    /// * `InvoiceError::AlreadyInitialized` if the contract has already been initialized.
    /// * `InvoiceError::InvalidConfiguration` if the registry address equals
    ///   the admin or this invoice contract.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.initialize(&admin, &registry_address);
    /// ```
    pub fn initialize(env: Env, admin: Address, registry_contract: Address) {
        require_not_paused(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, InvoiceError::AlreadyInitialized);
        }
        admin.require_auth();
        Self::assert_valid_wiring_address(&env, &registry_contract, &admin, None, Vec::new(&env));
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::RegistryContract, &registry_contract);
        env.storage().instance().set(&DataKey::Counter, &0u64);
        Self::extend_instance_ttl(&env);
        events::contract_initialized(&env, &admin, &registry_contract);
    }

    /// Returns the stored admin address, or `None` if not initialized.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Option<Address>` - The stored admin address if initialized, or `None`.
    ///
    /// # Example
    /// ```ignore
    /// let admin = client.get_admin();
    /// ```
    pub fn get_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Admin)
    }

    /// Replaces this contract's Wasm with an installed Wasm using the stored admin.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();

        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        Self::extend_instance_ttl(&env);
        events::upgraded(&env, &new_wasm_hash);
    }

    /// Returns the stored registry contract address, or `None` if not initialized.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Option<Address>` - The registry contract address if initialized, or `None`.
    ///
    /// # Example
    /// ```ignore
    /// let registry = client.get_registry_contract();
    /// ```
    pub fn get_registry_contract(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::RegistryContract)
    }

    /// Sets the pool contract address used by this invoice contract.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `pool_contract` - The pool contract address.
    ///
    /// # Auth
    /// Requires authorization from the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the admin is not initialized.
    /// * `InvoiceError::InvalidConfiguration` if the pool address aliases the
    ///   admin, this invoice contract, registry, escrow, or agent registry.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.set_pool_contract(&pool_address);
    /// ```
    pub fn set_pool_contract(env: Env, pool_contract: Address) {
        require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        let registry: Option<Address> = env.storage().instance().get(&DataKey::RegistryContract);
        let mut other_wired_contracts = Vec::new(&env);
        if let Some(escrow) = env.storage().instance().get(&DataKey::EscrowContract) {
            other_wired_contracts.push_back(escrow);
        }
        if let Some(agent_registry) = env
            .storage()
            .instance()
            .get(&DataKey::AgentRegistryContract)
        {
            other_wired_contracts.push_back(agent_registry);
        }
        Self::assert_valid_wiring_address(
            &env,
            &pool_contract,
            &admin,
            registry,
            other_wired_contracts,
        );
        let old_pool: Option<Address> = env.storage().instance().get(&DataKey::PoolContract);
        env.storage()
            .instance()
            .set(&DataKey::PoolContract, &pool_contract);
        if let Some(old) = old_pool {
            events::pool_contract_updated(&env, &old, &pool_contract);
        } else {
            events::pool_contract_updated(&env, &pool_contract, &pool_contract);
            Self::extend_instance_ttl(&env);
        }
    }

    /// Returns the stored pool contract address, or `None` if not configured.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Option<Address>` - The pool contract address if configured, or `None`.
    ///
    /// # Example
    /// ```ignore
    /// let pool = client.get_pool_contract();
    /// ```
    pub fn get_pool_contract(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::PoolContract)
    }

    /// Sets the agent-registry contract address used by `submit_attestation`
    /// to look up an attesting agent's signing key.
    ///
    /// The agent-registry contract is deployed from the separate
    /// `underwrite-contract` repo; its address is passed in here once it's
    /// available (see `AGENT_REGISTRY_CONTRACT` in `.env.example`).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `agent_registry_contract` - The deployed agent-registry contract address.
    ///
    /// # Auth
    /// Requires authorization from the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the admin is not initialized.
    /// * `InvoiceError::InvalidConfiguration` if the agent-registry address
    ///   aliases the admin, this invoice contract, registry, pool, or escrow.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.set_agent_registry_contract(&agent_registry_address);
    /// ```
    pub fn set_agent_registry_contract(env: Env, agent_registry_contract: Address) {
        require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        let registry: Option<Address> = env.storage().instance().get(&DataKey::RegistryContract);
        let mut other_wired_contracts = Vec::new(&env);
        if let Some(pool) = env.storage().instance().get(&DataKey::PoolContract) {
            other_wired_contracts.push_back(pool);
        }
        if let Some(escrow) = env.storage().instance().get(&DataKey::EscrowContract) {
            other_wired_contracts.push_back(escrow);
        }
        Self::assert_valid_wiring_address(
            &env,
            &agent_registry_contract,
            &admin,
            registry,
            other_wired_contracts,
        );
        let old: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::AgentRegistryContract);
        env.storage()
            .instance()
            .set(&DataKey::AgentRegistryContract, &agent_registry_contract);
        if let Some(old) = old {
            events::agent_registry_contract_updated(&env, &old, &agent_registry_contract);
        } else {
            events::agent_registry_contract_updated(
                &env,
                &agent_registry_contract,
                &agent_registry_contract,
            );
            Self::extend_instance_ttl(&env);
        }
    }

    /// Returns the stored agent-registry contract address, or `None` if not configured.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Option<Address>` - The agent-registry contract address if configured, or `None`.
    ///
    /// # Example
    /// ```ignore
    /// let agent_registry = client.get_agent_registry_contract();
    /// ```
    pub fn get_agent_registry_contract(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::AgentRegistryContract)
    }

    /// Sets the escrow contract address used by this invoice contract.
    ///
    /// The escrow address is required so that `repay` and `repay_early` can
    /// route buyer funds through escrow (buyer → escrow → pool) instead of
    /// transferring directly to the pool, which would bypass escrow security.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `escrow_contract` - The escrow contract address.
    ///
    /// # Auth
    /// Requires authorization from the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the admin is not initialized.
    /// * `InvoiceError::InvalidConfiguration` if the escrow address aliases
    ///   the admin, this invoice contract, registry, pool, or agent registry.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    pub fn set_escrow_contract(env: Env, escrow_contract: Address) {
        require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();

        let registry: Option<Address> = env.storage().instance().get(&DataKey::RegistryContract);
        let mut other_wired_contracts = Vec::new(&env);
        if let Some(pool) = env.storage().instance().get(&DataKey::PoolContract) {
            other_wired_contracts.push_back(pool);
        }
        if let Some(agent_registry) = env
            .storage()
            .instance()
            .get(&DataKey::AgentRegistryContract)
        {
            other_wired_contracts.push_back(agent_registry);
        }
        Self::assert_valid_wiring_address(
            &env,
            &escrow_contract,
            &admin,
            registry,
            other_wired_contracts,
        );

        let old_escrow: Option<Address> = env.storage().instance().get(&DataKey::EscrowContract);
        env.storage()
            .instance()
            .set(&DataKey::EscrowContract, &escrow_contract);
        Self::extend_instance_ttl(&env);

        if let Some(old) = old_escrow {
            events::escrow_contract_updated(&env, &old, &escrow_contract);
        } else {
            events::escrow_contract_updated(&env, &escrow_contract, &escrow_contract);
        }
    }

    /// Returns the stored escrow contract address, or `None` if not configured.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Option<Address>` - The escrow contract address if configured, or `None`.
    ///
    /// # Example
    /// ```ignore
    /// let escrow = client.get_escrow_contract();
    /// ```
    pub fn get_escrow_contract(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::EscrowContract)
    }

    /// Adds an asset to the list of supported funding assets.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The address of the asset to support.
    ///
    /// # Auth
    /// Requires authorization from the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the admin cannot be found.
    /// Returns the attestation for a given invoice, if one exists.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Option<Attestation>` - The attestation if one exists, or `None`.
    ///
    /// # Example
    /// ```ignore
    /// let attestation = client.get_attestation(&invoice_id);
    /// ```
    pub fn get_attestation(env: Env, invoice_id: BytesN<32>) -> Option<Attestation> {
        env.storage()
            .persistent()
            .get(&DataKey::Attestation(invoice_id))
    }

    /// Adds an asset to the allow-list of funding assets that `create`
    /// accepts. Adding an already-supported asset is a no-op.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The token contract address to allow-list.
    ///
    /// # Auth
    /// Requires authorization from the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the admin has not been initialized.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Pool Factory Integration
    /// Note that under the `pool_factory` model, a supported asset listed here
    /// should also be registered via `pool_factory::register_asset` (or
    /// `register_existing_pool`). If an invoice is listed in an asset that has
    /// no corresponding pool instance, `fund_invoice` will not be reachable
    /// end-to-end for that invoice.
    ///
    /// # Example
    /// ```ignore
    /// client.add_supported_asset(&usdc);
    /// ```
    pub fn add_supported_asset(env: Env, asset: Address) {
        require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();

        let key = DataKey::SupportedAsset(asset.clone());
        if env.storage().persistent().has(&key) {
            return;
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::SupportedAssetCount)
            .unwrap_or(0);
        let next_count = count
            .checked_add(1)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::CounterOverflow));
        env.storage()
            .instance()
            .set(&DataKey::SupportedAssetCount, &next_count);
        env.storage().persistent().set(&key, &true);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::extend_instance_ttl(&env);
        events::supported_asset_added(&env, &asset);
    }

    /// Removes an asset from the allow-list of funding assets.
    ///
    /// Removing an asset that is not currently supported is a no-op. Once
    /// removed, new invoices can no longer be created with it, though any
    /// already-funded invoices using it continue unaffected.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The token contract address to remove from the allow-list.
    ///
    /// # Auth
    /// Requires authorization from the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the admin has not been initialized.
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.remove_supported_asset(&usdc);
    /// ```
    pub fn remove_supported_asset(env: Env, asset: Address) {
        require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();

        let key = DataKey::SupportedAsset(asset.clone());
        if !env.storage().persistent().has(&key) {
            return;
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&DataKey::SupportedAssetCount)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::SupportedAssetCount, &(count - 1));
        env.storage().persistent().remove(&key);
        Self::extend_instance_ttl(&env);
        events::supported_asset_removed(&env, &asset);
    }

    /// Checks whether an asset is currently on the funding allow-list.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `asset` - The token contract address to check.
    ///
    /// # Auth
    /// None — this is a public read.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `bool` - `true` if the asset is allow-listed, `false` otherwise.
    ///
    /// # Example
    /// ```ignore
    /// let supported = client.is_supported_asset(&usdc);
    /// ```
    pub fn is_supported_asset(env: Env, asset: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::SupportedAsset(asset))
    }

    /// Returns the number of assets currently on the funding allow-list.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// None — this is a public read.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `u32` - The count of supported assets. `0` if none have ever been
    ///   added.
    ///
    /// # Example
    /// ```ignore
    /// let count = client.get_supported_asset_count();
    /// ```
    pub fn get_supported_asset_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::SupportedAssetCount)
            .unwrap_or(0)
    }

    /// Creates a new invoice with the given issuer, buyer, and terms.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `issuer` - The issuer address creating the invoice.
    /// * `buyer` - The buyer address receiving the invoice.
    /// * `face_value` - The full invoice value.
    /// * `due_date` - The invoice due date timestamp.
    /// * `funding_asset` - The asset to be used for financing.
    ///
    /// # Auth
    /// Requires authorization from `issuer`.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the contract has not been initialized.
    /// * `InvoiceError::IssuerNotVerified` if the issuer is not verified in the registry.
    /// * `InvoiceError::BuyerNotVerified` if the buyer is not verified in the registry.
    /// * `InvoiceError::InvalidFaceValue` if `face_value` is zero.
    /// * `InvoiceError::InvalidAmount` if `face_value` exceeds [`MAX_FACE_VALUE`].
    /// * `InvoiceError::InvalidDueDate` if `due_date` is not strictly in the
    ///   future. Requires `due_date > now`; the boundary comparator is `<=`,
    ///   so `due_date == now` is rejected. Pinning tests:
    ///   `test_create_fails_when_due_date_equals_now` and
    ///   `test_create_succeeds_when_due_date_one_second_in_future`.
    /// * `InvoiceError::InvalidDueDate` if `due_date` exceeds
    ///   `now + MAX_INVOICE_LIFETIME_SECONDS` (~10 years).
    /// * `InvoiceError::CounterOverflow` if the internal invoice counter overflows.
    /// * `InvoiceError::InvalidParticipants` if `issuer` and `buyer` are the same address.
    ///
    /// # Returns
    /// * `BytesN<32>` - The generated invoice ID.
    ///
    /// # Example
    /// ```ignore
    /// let invoice_id = client.create(&issuer, &buyer, 1_000, 1_000_000, &asset);
    /// ```
    pub fn create(
        env: Env,
        issuer: Address,
        buyer: Address,
        face_value: u128,
        due_date: u64,
        funding_asset: Address,
    ) -> BytesN<32> {
        require_not_paused(&env);
        issuer.require_auth();
        Self::create_inner(&env, issuer, buyer, face_value, due_date, funding_asset)
    }

    /// Auth-free body of [`Self::create`], shared with [`Self::batch_create`].
    ///
    /// The caller owns `require_not_paused` and the issuer's `require_auth()`.
    /// The split matters for the batch path: Soroban rejects a repeated
    /// `require_auth()` for the same address inside one invocation frame with
    /// `Error(Auth, ExistingValue)`, so a batch cannot re-enter the
    /// auth-performing `create` once per entry.
    fn create_inner(
        env: &Env,
        issuer: Address,
        buyer: Address,
        face_value: u128,
        due_date: u64,
        funding_asset: Address,
    ) -> BytesN<32> {
        if issuer == buyer {
            panic_with_error!(env, InvoiceError::InvalidParticipants);
        }

        let registry_id: Address = env
            .storage()
            .instance()
            .get(&DataKey::RegistryContract)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));

        require_verified(env, &registry_id, &issuer, InvoiceError::IssuerNotVerified);
        require_verified(env, &registry_id, &buyer, InvoiceError::BuyerNotVerified);

        if !env
            .storage()
            .persistent()
            .has(&DataKey::SupportedAsset(funding_asset.clone()))
        {
            panic_with_error!(env, InvoiceError::UnsupportedAsset);
        }

        if face_value == 0 {
            panic_with_error!(env, InvoiceError::InvalidFaceValue);
        }
        if face_value > MAX_FACE_VALUE {
            panic_with_error!(env, InvoiceError::InvalidAmount);
        }
        let now = env.ledger().timestamp();
        if due_date <= now {
            panic_with_error!(env, InvoiceError::InvalidDueDate);
        }
        let max_due_date = now
            .checked_add(MAX_INVOICE_LIFETIME_SECONDS)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::MathOverflow));
        if due_date > max_due_date {
            panic_with_error!(env, InvoiceError::InvalidDueDate);
        }

        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::Counter)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
        let next_counter = counter
            .checked_add(1)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::CounterOverflow));
        env.storage()
            .instance()
            .set(&DataKey::Counter, &next_counter);

        let mut hash_input = Bytes::new(env);
        let issuer_xdr = issuer.clone().to_xdr(env);
        let buyer_xdr = buyer.clone().to_xdr(env);

        // Safely append all XDR bytes without assuming a fixed length
        for b in issuer_xdr.iter() {
            hash_input.push_back(b);
        }
        for b in buyer_xdr.iter() {
            hash_input.push_back(b);
        }
        for b in face_value.to_be_bytes() {
            hash_input.push_back(b);
        }
        for b in due_date.to_be_bytes() {
            hash_input.push_back(b);
        }
        for b in counter.to_be_bytes() {
            hash_input.push_back(b);
        }
        {
            let asset_xdr = funding_asset.clone().to_xdr(env);
            for b in asset_xdr.iter() {
                hash_input.push_back(b);
            }
        }
        let invoice_id: BytesN<32> = env.crypto().sha256(&hash_input).into();

        let invoice = Invoice {
            id: invoice_id.clone(),
            issuer: issuer.clone(),
            buyer: buyer.clone(),
            face_value,
            discount_bps: 0,
            funded_amount: 0,
            due_date,
            status: InvoiceStatus::Created,
            created_at: now,
            listed_at: None,
            funded_at: None,
            shipped_at: None,
            issuer_confirmed: false,
            buyer_confirmed: false,
            repaid_at: None,
            funding_asset: funding_asset.clone(),
            funding_pool: None,
            repaid_amount: 0,
            remaining_balance: face_value,
        };

        let inv_key = DataKey::Invoice(invoice_id.clone());
        Self::save_invoice(env, inv_key, &invoice);

        self::extend_issuer_index(env, &issuer, &invoice_id);
        self::extend_buyer_index(env, &buyer, &invoice_id);
        self::extend_status_index(env, InvoiceStatus::Created, &invoice_id);
        increment_status_count(env, InvoiceStatus::Created);
        Self::extend_instance_ttl(env);

        events::invoice_created(
            env,
            &invoice_id,
            &invoice.issuer,
            &invoice.buyer,
            face_value,
            &funding_asset,
        );
        invoice_id
    }

    /// Creates several invoices in one call, applying `create`'s validation to
    /// every entry.
    ///
    /// Each entry is `(buyer, face_value, due_date, funding_asset)`. Every
    /// entry is validated and persisted with the exact same checks as
    /// [`Self::create`] — verified issuer and buyer, distinct participants,
    /// non-zero face value within [`MAX_FACE_VALUE`], a due date strictly in the
    /// future and no more than [`MAX_INVOICE_LIFETIME_SECONDS`] ahead, and a
    /// supported funding asset — by delegating to `create_inner` per entry, so
    /// the two entry points cannot drift apart.
    ///
    /// # Partial-success behavior (atomic)
    ///
    /// `batch_create` is **all-or-nothing**: the first entry that fails
    /// validation reverts the entire call, including invoices already created
    /// by earlier entries in the same batch. This is deliberate and differs
    /// from `RegistryContract::batch_register_issuers`, which pre-validates
    /// only *metadata* and then skips *duplicates*. The reason is that invoice
    /// creation consumes the shared `Counter`: a partial success would burn
    /// counter values and persist a prefix of the batch, leaving the caller
    /// unable to tell which invoices exist without re-reading every invoice it
    /// submitted. A reverted batch leaves no invoices and no counter movement,
    /// so the caller can fix the bad entry and resubmit the whole batch.
    ///
    /// Callers needing tolerant behavior should pre-validate with the views
    /// (`is_supported_asset`, `get_buyer`, `get_status`) or call
    /// [`Self::create`] per invoice. [`Self::batch_list_for_financing`] is the
    /// tolerant counterpart for the listing step.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `issuer` - The issuer creating every invoice in the batch.
    /// * `entries` - `(buyer, face_value, due_date, funding_asset)` per invoice.
    ///
    /// # Auth
    /// Requires authorization from `issuer` **once for the entire batch**, not
    /// once per entry: Soroban rejects a repeated `require_auth()` for the same
    /// address inside one invocation frame with `Error(Auth, ExistingValue)`,
    /// so a single signature covers every invoice created by this call.
    ///
    /// # Panics
    /// * `InvoiceError::BatchSizeExceeded` if `entries.len() > MAX_BATCH_SIZE`.
    /// * Every panic documented for [`Self::create`], for whichever entry
    ///   fails first.
    ///
    /// # Returns
    /// * `Vec<BytesN<32>>` - The generated invoice IDs, in entry order.
    ///
    /// # Example
    /// ```ignore
    /// client.batch_create(&issuer, &vec![&env, (buyer, 1_000, due, &asset)]);
    /// ```
    pub fn batch_create(
        env: Env,
        issuer: Address,
        entries: Vec<(Address, u128, u64, Address)>,
    ) -> Vec<BytesN<32>> {
        require_not_paused(&env);
        if entries.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, InvoiceError::BatchSizeExceeded);
        }
        // Auth once for the whole batch; see `create_inner`.
        issuer.require_auth();

        let mut created: Vec<BytesN<32>> = Vec::new(&env);
        for (buyer, face_value, due_date, funding_asset) in entries.iter() {
            // Delegating to `create_inner` keeps validation identical by
            // construction.
            let invoice_id = Self::create_inner(
                &env,
                issuer.clone(),
                buyer.clone(),
                face_value,
                due_date,
                funding_asset.clone(),
            );
            created.push_back(invoice_id);
        }

        events::batch_invoices_created(&env, created.len(), 0);
        Self::extend_instance_ttl(&env);
        created
    }

    /// Lists a created invoice for financing with a discount.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to list.
    /// * `discount_bps` - The discount rate in basis points.
    ///
    /// # Auth
    /// Requires authorization from the invoice's issuer.
    ///
    /// # Registry re-verification
    /// Registry verification is re-checked here in addition to `create()`.
    /// This is the last point before pool capital can be committed to the
    /// invoice, so a revocation that happened after `create()` but before
    /// listing must still block it. Verification is deliberately **not**
    /// re-checked at any later lifecycle step (`mark_shipped`,
    /// `confirm_delivery`, `repay`, `repay_early`, `trigger_default`):
    /// once an invoice is funded, pool liquidity is already committed and
    /// repayment terms are already fixed, so a later revocation does not
    /// retroactively unwind or default an in-flight invoice. See
    /// `PoolContract::fund_invoice` for the other re-check point.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice does not exist.
    /// * `InvoiceError::VerificationRequired` if no Underwrite agent attestation has
    ///   been submitted for this invoice (see `submit_attestation`).
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not `Created`.
    /// * `InvoiceError::IssuerNotVerified` if the issuer's registry verification
    ///   has since been revoked.
    /// * `InvoiceError::BuyerNotVerified` if the buyer's registry verification
    ///   has since been revoked.
    /// * `InvoiceError::DiscountTooHigh` if `discount_bps` is greater than 5000.
    ///
    /// # Returns
    /// * `bool` - `true` when listing succeeds.
    ///
    /// # Example
    /// ```ignore
    /// client.list_for_financing(&invoice_id, 250);
    /// ```
    pub fn list_for_financing(env: Env, invoice_id: BytesN<32>, discount_bps: u32) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Attestation(invoice_id.clone()))
        {
            panic_with_error!(&env, InvoiceError::VerificationRequired);
        }
        invoice.issuer.require_auth();
        if invoice.status != InvoiceStatus::Created {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        let registry_id: Address = env
            .storage()
            .instance()
            .get(&DataKey::RegistryContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        require_verified(
            &env,
            &registry_id,
            &invoice.issuer,
            InvoiceError::IssuerNotVerified,
        );
        require_verified(
            &env,
            &registry_id,
            &invoice.buyer,
            InvoiceError::BuyerNotVerified,
        );
        if discount_bps > 5000 {
            panic_with_error!(&env, InvoiceError::DiscountTooHigh);
        }
        let listed_at = env.ledger().timestamp();
        let expires_at = Self::listing_expiry_timestamp(&env, listed_at);
        invoice.status = InvoiceStatus::Listed;
        invoice.discount_bps = discount_bps;
        invoice.listed_at = Some(listed_at);
        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);

        move_status_index(
            &env,
            &invoice_id,
            InvoiceStatus::Created,
            InvoiceStatus::Listed,
        );
        events::invoice_listed(&env, &invoice_id, discount_bps, expires_at);
        true
    }

    /// Lists several created invoices for financing in one call, applying
    /// `list_for_financing`'s checks to each entry.
    ///
    /// Each entry is `(invoice_id, discount_bps)`. An entry is listed when it
    /// has a submitted attestation, is in `Created` status, has both its issuer
    /// and buyer still verified in the registry, and has
    /// `discount_bps <= 5000` — the same checks [`Self::list_for_financing`]
    /// applies.
    ///
    /// # Failure behavior (per-entry tolerant)
    ///
    /// Unlike [`Self::batch_create`], this entry point is **tolerant**: an entry
    /// that fails any check is collected into the returned failed-list and the
    /// batch continues with the next entry. The call is only rejected by errors
    /// that make the batch itself meaningless: exceeding [`MAX_BATCH_SIZE`], a
    /// missing registry reference, or a missing issuer authorization.
    ///
    /// Tolerant behavior is right here because the interesting failures are
    /// per-invoice state an issuer listing a backlog cannot avoid (already
    /// listed, revoked verification, missing attestation). Reverting the whole
    /// batch would force the caller to bisect by trial and error, and would also
    /// discard the successful listings that already emitted events.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `entries` - `(invoice_id, discount_bps)` per invoice.
    ///
    /// # Auth
    /// Requires authorization from each listed invoice's own issuer, but only
    /// **once per distinct issuer** for the whole batch. Soroban rejects a
    /// repeated `require_auth()` for the same address inside one invocation
    /// frame, so a batch cannot re-require per entry; an issuer listing several
    /// of their own invoices signs once and covers them all.
    ///
    /// # Panics
    /// * `InvoiceError::BatchSizeExceeded` if `entries.len() > MAX_BATCH_SIZE`.
    /// * `InvoiceError::NotFound` if the contract is uninitialized or the
    ///   registry reference is missing.
    /// * An auth failure if a listed invoice's issuer has not authorized this
    ///   call.
    ///
    /// # Returns
    /// * `Vec<BytesN<32>>` - The invoice IDs that failed validation, in entry
    ///   order. An empty result means every entry was listed.
    ///
    /// # Example
    /// ```ignore
    /// let failed = client.batch_list_for_financing(
    ///     &vec![&env, (invoice_id, 250)],
    /// );
    /// ```
    pub fn batch_list_for_financing(env: Env, entries: Vec<(BytesN<32>, u32)>) -> Vec<BytesN<32>> {
        require_not_paused(&env);
        if entries.len() > MAX_BATCH_SIZE {
            panic_with_error!(&env, InvoiceError::BatchSizeExceeded);
        }

        let registry_id: Address = env
            .storage()
            .instance()
            .get(&DataKey::RegistryContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        let mut failed: Vec<BytesN<32>> = Vec::new(&env);
        let mut listed: u32 = 0;
        let mut authorized: Vec<Address> = Vec::new(&env);
        for (invoice_id, discount_bps) in entries.iter() {
            match Self::list_entry(
                &env,
                &registry_id,
                &invoice_id,
                discount_bps,
                &mut authorized,
            ) {
                true => listed += 1,
                false => failed.push_back(invoice_id.clone()),
            }
        }

        events::batch_invoices_listed(&env, listed, failed.len());
        Self::extend_instance_ttl(&env);
        failed
    }

    /// Per-entry body of [`Self::batch_list_for_financing`].
    ///
    /// Returns `true` when the invoice was listed and `false` when it failed a
    /// validation check. Every failure is a *return*, never a panic, so one bad
    /// entry cannot abort the batch.
    ///
    /// `authorized` accumulates the issuers whose signature was already
    /// required in this frame, because a second `require_auth()` for the same
    /// address is rejected by Soroban with `Error(Auth, ExistingValue)`.
    fn list_entry(
        env: &Env,
        registry_id: &Address,
        invoice_id: &BytesN<32>,
        discount_bps: u32,
        authorized: &mut Vec<Address>,
    ) -> bool {
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let Some(mut invoice) = env.storage().persistent().get::<_, Invoice>(&inv_key) else {
            return false;
        };
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Attestation(invoice_id.clone()))
        {
            return false;
        }
        if !authorized.contains(&invoice.issuer) {
            invoice.issuer.require_auth();
            authorized.push_back(invoice.issuer.clone());
        }
        if invoice.status != InvoiceStatus::Created {
            return false;
        }
        if !Self::is_verified_in(env, registry_id, &invoice.issuer)
            || !Self::is_verified_in(env, registry_id, &invoice.buyer)
        {
            return false;
        }
        if discount_bps > 5000 {
            return false;
        }
        let listed_at = env.ledger().timestamp();
        let Some(expires_at) = Self::checked_listing_expiry_timestamp(env, listed_at) else {
            return false;
        };
        invoice.status = InvoiceStatus::Listed;
        invoice.discount_bps = discount_bps;
        invoice.listed_at = Some(listed_at);
        Self::save_invoice(env, inv_key, &invoice);

        move_status_index(
            env,
            invoice_id,
            InvoiceStatus::Created,
            InvoiceStatus::Listed,
        );
        events::invoice_listed(env, invoice_id, discount_bps, expires_at);
        true
    }

    /// Non-panicking registry verification probe, used by the tolerant
    /// per-entry listing path where a revoked verification must be reported as a
    /// failed entry rather than revert the whole batch.
    fn is_verified_in(env: &Env, registry_id: &Address, address: &Address) -> bool {
        let mut args = Vec::new(env);
        args.push_back(address.clone().into_val(env));
        let verified: bool =
            env.invoke_contract(registry_id, &Symbol::new(env, "is_verified"), args);
        verified
    }

    /// Cancels an invoice while it is still in `Created` status.
    pub fn cancel(env: Env, invoice_id: BytesN<32>) -> bool {
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        invoice.issuer.require_auth();
        if invoice.status != InvoiceStatus::Created {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        invoice.status = InvoiceStatus::Cancelled;
        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);
        move_status_index(
            &env,
            &invoice_id,
            InvoiceStatus::Created,
            InvoiceStatus::Cancelled,
        );
        events::invoice_cancelled(&env, &invoice_id);
        true
    }

    /// Submits a signed risk attestation from a registered Underwrite agent
    /// against an invoice, unlocking it for `list_for_financing`.
    ///
    /// This is the hook by which ClearLedger calls into Underwrite's
    /// agent-registry contract (a separate product, built in the
    /// `underwrite-contract` repo) — this contract only verifies the
    /// signature and checks the signer against that registry; it does not
    /// implement any agent logic itself.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being attested.
    /// * `payload` - XDR-encoded [`AttestationPayload`]: `domain_separator`,
    ///   `invoice_id`, `risk_score`, `evidence_hash`, `agent_id`, `nonce`.
    ///   This is exactly the byte string the agent signed.
    /// * `signature` - A 65-byte recoverable secp256k1 signature over
    ///   `keccak256(payload)`: 64 bytes of `r || s` followed by a 1-byte
    ///   recovery id.
    ///
    /// # Auth
    /// None from the caller — submission is deliberately permissionless.
    /// Trust comes entirely from the signature recovering to an active
    /// agent's registered pubkey, not from who relays the transaction.
    ///
    /// # Replay / domain binding
    /// The signature is over the exact XDR `payload`, so any change to
    /// `domain_separator`, `invoice_id`, `agent_id`, or `nonce` invalidates
    /// it. The fixed [`ATTESTATION_DOMAIN_SEPARATOR`] separates this scheme
    /// from other contracts/protocols, and the one-attestation-per-invoice
    /// storage guard is the on-chain replay guard. There is deliberately no
    /// separate nonce ledger: distinct nonces do not permit a second
    /// attestation for the same invoice.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice does not exist, or if the
    ///   agent-registry contract has not been configured via
    ///   `set_agent_registry_contract`.
    /// * `InvoiceError::InvalidAmount` if `payload` fails to decode as an
    ///   `AttestationPayload`, if its `domain_separator` doesn't match this
    ///   contract's, or if its `invoice_id` doesn't match the `invoice_id`
    ///   argument.
    /// * `InvoiceError::UntrustedSigner` if the recovered signer is not an
    ///   active agent in the agent-registry, or its registered pubkey
    ///   doesn't match the recovered key.
    /// * `InvoiceError::AlreadyAttested` if an attestation already exists
    ///   for this invoice (replay guard — one attestation per invoice).
    ///
    /// # Returns
    /// * `()` - No value is returned.
    ///
    /// # Example
    /// ```ignore
    /// client.submit_attestation(&invoice_id, &payload, &signature);
    /// ```
    pub fn submit_attestation(
        env: Env,
        invoice_id: BytesN<32>,
        payload: Bytes,
        signature: BytesN<65>,
    ) {
        require_not_paused(&env);
        // NO require_auth on the caller — submission is permissionless by
        // design. Security comes entirely from the signature check below,
        // not from who calls this.
        Self::get_invoice(&env, invoice_id.clone());

        let attestation_key = DataKey::Attestation(invoice_id.clone());
        if env.storage().persistent().has(&attestation_key) {
            panic_with_error!(&env, InvoiceError::AlreadyAttested);
        }

        let decoded: AttestationPayload = AttestationPayload::from_xdr(&env, &payload)
            .unwrap_or_else(|_| panic_with_error!(&env, InvoiceError::InvalidAmount));
        if decoded.domain_separator != BytesN::from_array(&env, &ATTESTATION_DOMAIN_SEPARATOR) {
            panic_with_error!(&env, InvoiceError::InvalidAmount);
        }
        if decoded.invoice_id != invoice_id {
            panic_with_error!(&env, InvoiceError::InvalidAmount);
        }

        // Split the 65-byte recoverable signature into its 64-byte r||s
        // component and 1-byte recovery id for `secp256k1_recover`.
        let sig_bytes = signature.to_array();
        let mut rs = [0u8; 64];
        rs.copy_from_slice(&sig_bytes[0..64]);
        let recovery_id = sig_bytes[64] as u32;
        let sig_rs = BytesN::from_array(&env, &rs);

        let hash = env.crypto().keccak256(&payload);
        let recovered_pubkey = env.crypto().secp256k1_recover(&hash, &sig_rs, recovery_id);

        let registry: Address = env
            .storage()
            .instance()
            .get(&DataKey::AgentRegistryContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        let mut args = Vec::new(&env);
        args.push_back(decoded.agent_id.clone().into_val(&env));
        let agent: Option<Agent> =
            env.invoke_contract(&registry, &Symbol::new(&env, "get_agent"), args);
        match agent {
            Some(agent) if agent.active && agent.pubkey == recovered_pubkey => {}
            _ => panic_with_error!(&env, InvoiceError::UntrustedSigner),
        }

        let attestation = Attestation {
            agent_id: decoded.agent_id.clone(),
            risk_score: decoded.risk_score,
            evidence_hash: decoded.evidence_hash,
            submitted_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&attestation_key, &attestation);
        env.storage()
            .persistent()
            .extend_ttl(&attestation_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::extend_instance_ttl(&env);

        events::attestation_submitted(&env, &invoice_id, &decoded.agent_id, decoded.risk_score);
    }

    /// Marks a listed invoice as funded by a pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being funded.
    /// * `pool_address` - The pool address authorizing funding.
    /// * `asset_address` - The asset used to fund the invoice.
    /// * `funded_amount` - The amount funded.
    ///
    /// # Auth
    /// Requires authorization from `pool_address`.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice cannot be found, or if the
    ///   pool contract has not been configured via `set_pool_contract`.
    /// * `InvoiceError::NotAuthorized` if `pool_address` does not match the
    ///   pool contract configured via `set_pool_contract`. The configured
    ///   pool is the only address allowed to fund — without this check any
    ///   address that can self-authorize could hijack an invoice's
    ///   `funding_pool` (issue #553).
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not `Listed`.
    /// * `InvoiceError::UnsupportedAsset` if the asset does not match the invoice funding asset.
    /// * `InvoiceError::InvalidAmount` if `funded_amount` is zero or exceeds
    ///   the invoice's `face_value` (issue #554).
    ///
    /// # Returns
    /// * `bool` - `true` when funding is recorded.
    ///
    /// # Example
    /// ```ignore
    /// client.mark_funded(&invoice_id, &pool, &asset, 950);
    /// ```
    pub fn mark_funded(
        env: Env,
        invoice_id: BytesN<32>,
        pool_address: Address,
        asset_address: Address,
        funded_amount: u128,
    ) -> bool {
        require_not_paused(&env);
        let configured_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        if pool_address != configured_pool {
            panic_with_error!(&env, InvoiceError::NotAuthorized);
        }
        pool_address.require_auth();
        let configured_pool: Address = env
            .storage()
            .instance()
            .get(&DataKey::PoolContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        if pool_address != configured_pool {
            panic_with_error!(&env, InvoiceError::NotAuthorized);
        }

        if funded_amount == 0 {
            panic_with_error!(&env, InvoiceError::InvalidAmount);
        }

        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        if invoice.status != InvoiceStatus::Listed {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }
        if funded_amount > invoice.face_value {
            panic_with_error!(&env, InvoiceError::InvalidAmount);
        }
        if asset_address != invoice.funding_asset {
            panic_with_error!(&env, InvoiceError::UnsupportedAsset);
        }
        if funded_amount > invoice.face_value {
            panic_with_error!(&env, InvoiceError::InvalidAmount);
        }

        invoice.status = InvoiceStatus::Funded;
        invoice.funded_amount = funded_amount;
        invoice.funded_at = Some(env.ledger().timestamp());
        invoice.funding_pool = Some(pool_address);
        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);

        move_status_index(
            &env,
            &invoice_id,
            InvoiceStatus::Listed,
            InvoiceStatus::Funded,
        );
        events::invoice_funded_v1(&env, &invoice_id, funded_amount);
        events::invoice_funded(&env, &invoice_id, funded_amount);
        true
    }

    /// Marks a funded invoice as shipped.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to mark as shipped.
    ///
    /// # Auth
    /// Requires authorization from the invoice's issuer.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not `Funded`.
    ///
    /// # Returns
    /// * `bool` - `true` when shipment is recorded.
    ///
    /// # Example
    /// ```ignore
    /// client.mark_shipped(&invoice_id);
    /// ```
    pub fn mark_shipped(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        invoice.issuer.require_auth();
        if invoice.status != InvoiceStatus::Funded {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }
        invoice.status = InvoiceStatus::Active;
        invoice.shipped_at = Some(env.ledger().timestamp());
        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);

        move_status_index(
            &env,
            &invoice_id,
            InvoiceStatus::Funded,
            InvoiceStatus::Active,
        );
        events::invoice_shipped(&env, &invoice_id);
        true
    }

    /// Confirms delivery for an active invoice by issuer or buyer.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being confirmed.
    /// * `confirmer` - The address confirming delivery.
    ///
    /// # Auth
    /// Requires authorization from `confirmer`.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not `Active`.
    /// * `InvoiceError::NotAuthorized` if the confirmer is neither issuer nor buyer.
    /// * `InvoiceError::AlreadyConfirmed` if the confirmer already confirmed.
    ///
    /// # Returns
    /// * `bool` - `true` when confirmation is processed.
    ///
    /// # Events
    /// All events are published after the invoice record is persisted and its
    /// TTL extended, so any event observer that reacts to an event is
    /// guaranteed to see the fully-updated invoice if it reads storage in
    /// response. When both parties have confirmed, `both_confirmed` is
    /// published first, followed by `delivery_confirmed`; otherwise only
    /// `delivery_confirmed` is published.
    ///
    /// # Example
    /// ```ignore
    /// client.confirm_delivery(&invoice_id, &buyer);
    /// ```
    pub fn confirm_delivery(env: Env, invoice_id: BytesN<32>, confirmer: Address) -> bool {
        require_not_paused(&env);
        confirmer.require_auth();

        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        if invoice.status != InvoiceStatus::Active {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }
        if confirmer != invoice.issuer && confirmer != invoice.buyer {
            panic_with_error!(&env, InvoiceError::NotAuthorized);
        }

        if confirmer == invoice.issuer {
            if invoice.issuer_confirmed {
                panic_with_error!(&env, InvoiceError::AlreadyConfirmed);
            }
            invoice.issuer_confirmed = true;
        }
        if confirmer == invoice.buyer {
            if invoice.buyer_confirmed {
                panic_with_error!(&env, InvoiceError::AlreadyConfirmed);
            }
            invoice.buyer_confirmed = true;
        }

        let both_confirmed = invoice.issuer_confirmed && invoice.buyer_confirmed;
        if both_confirmed {
            invoice.status = InvoiceStatus::Confirmed;
            move_status_index(
                &env,
                &invoice_id,
                InvoiceStatus::Active,
                InvoiceStatus::Confirmed,
            );
        }

        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);

        // Emit events only after all state (invoice record, status index,
        // TTLs) has been persisted, so event ordering never depends on which
        // branch was taken above.
        if both_confirmed {
            events::both_confirmed(&env, &invoice_id);
        }
        events::delivery_confirmed(&env, &invoice_id, &confirmer);
        true
    }

    /// Repays a confirmed invoice, transferring funds to the pool.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being repaid.
    ///
    /// # Auth
    /// Requires authorization from the invoice's buyer.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice cannot be found, or if the invoice has no
    ///   recorded funding pool or funding timestamp.
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not `Funded`, `Active`, or `Confirmed`.
    /// * `InvoiceError::CrossContractCallFailed` if token transfer, escrow, or pool
    ///   repayment accounting fails.
    ///
    /// # Returns
    /// * `bool` - `true` when repayment is completed.
    ///
    /// # Example
    /// ```ignore
    /// client.repay(&invoice_id);
    /// ```
    /// Repays an invoice in partial installments or in full.
    ///
    /// Transfers `amount` from the buyer to escrow. If the cumulative repayment
    /// is less than `face_value`, the invoice remains in its current status,
    /// updating `repaid_amount` and `remaining_balance` and emitting `partial_repayment_received`.
    /// Once cumulative repayment equals `face_value`, the escrow releases the funds
    /// to the pool, the pool's repayment accounting is updated, the invoice transitions
    /// to `Repaid`, and `invoice_repaid` is emitted.
    pub fn repay_partial(env: Env, invoice_id: BytesN<32>, amount: u128) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        invoice.buyer.require_auth();
        if invoice.status != InvoiceStatus::Funded
            && invoice.status != InvoiceStatus::Active
            && invoice.status != InvoiceStatus::Confirmed
        {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        if amount == 0 {
            panic_with_error!(&env, InvoiceError::InvalidAmount);
        }

        if amount > invoice.remaining_balance {
            panic_with_error!(&env, InvoiceError::RepaymentExceedsBalance);
        }

        let escrow: Address = env
            .storage()
            .instance()
            .get(&DataKey::EscrowContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        let token = token::Client::new(&env, &invoice.funding_asset);
        // Step 1: buyer transfers amount into escrow
        if !matches!(
            token.try_transfer(&invoice.buyer, &escrow, &(amount as i128)),
            Ok(Ok(()))
        ) {
            panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
        }

        let new_repaid_amount = invoice
            .repaid_amount
            .checked_add(amount)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::MathOverflow));
        let new_remaining = invoice
            .remaining_balance
            .checked_sub(amount)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::MathOverflow));

        if new_remaining > 0 {
            let mut updated = invoice;
            updated.repaid_amount = new_repaid_amount;
            updated.remaining_balance = new_remaining;
            Self::save_invoice(&env, inv_key, &updated);
            Self::extend_instance_ttl(&env);

            events::partial_repayment_received(&env, &invoice_id, amount, new_remaining);
            true
        } else {
            let pool: Address = invoice
                .funding_pool
                .clone()
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            let face_value = invoice.face_value;
            let funded_amount = invoice.funded_amount;

            let now = env.ledger().timestamp();
            let funded_at = invoice
                .funded_at
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            let discount = face_value.saturating_sub(funded_amount);
            let term = invoice.due_date.saturating_sub(funded_at);
            let elapsed = now.saturating_sub(funded_at);
            // Floor proportional discount accrual; the buyer receives the
            // remainder so earned amount plus refund equals the full discount.
            let earned_by_pool = if term == 0 {
                discount
            } else {
                discount
                    .checked_mul(elapsed as u128)
                    .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::MathOverflow))
                    / (term as u128)
            };
            let refund_to_buyer = discount.saturating_sub(earned_by_pool);

            // Step 2: escrow releases full face_value back to pool
            let mut escrow_args = Vec::new(&env);
            escrow_args.push_back(invoice_id.clone().into_val(&env));
            escrow_args.push_back(face_value.into_val(&env));
            let escrow_released: bool =
                env.invoke_contract(&escrow, &Symbol::new(&env, "release_to_pool"), escrow_args);
            if !escrow_released {
                panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
            }

            // Step 3: notify pool to update its internal accounting
            let mut args = Vec::new(&env);
            args.push_back(invoice_id.clone().into_val(&env));
            args.push_back(face_value.into_val(&env));
            args.push_back(refund_to_buyer.into_val(&env));
            args.push_back(invoice.buyer.into_val(&env));
            let repayment_recorded: bool = env.invoke_contract(
                &pool,
                &Symbol::new(&env, "receive_repayment_with_refund"),
                args,
            );
            if !repayment_recorded {
                panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
            }

            let prev_status = invoice.status;
            let mut updated = invoice;
            updated.status = InvoiceStatus::Repaid;
            updated.repaid_at = Some(now);
            updated.repaid_amount = new_repaid_amount;
            updated.remaining_balance = 0;
            Self::save_invoice(&env, inv_key, &updated);
            Self::extend_instance_ttl(&env);

            move_status_index(&env, &invoice_id, prev_status, InvoiceStatus::Repaid);
            events::invoice_repaid(&env, &invoice_id, updated.face_value);
            true
        }
    }

    pub fn repay(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        Self::repay_partial(env, invoice_id, invoice.remaining_balance)
    }

    /// Repays a confirmed invoice before its due date, returning the
    /// pro-rated discount to the buyer.
    ///
    /// Early repayment settles the invoice in full (via escrow → pool, the
    /// same fund route as [`Self::repay`]) before maturity. Instead of the
    /// buyer owning the full unbilled discount, the pool only earns the
    /// discount for the portion of the term that has already elapsed; the
    /// remaining discount is refunded back to the buyer. This function must
    /// be called before `due_date` — once the invoice is due, use
    /// [`Self::repay`] instead.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice being repaid early.
    ///
    /// # Auth
    /// Requires authorization from the invoice's buyer.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice cannot be found, or if the
    ///   invoice has no recorded funding pool or funding timestamp.
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not
    ///   `Confirmed`, or if `now >= due_date` (an early repayment must happen
    ///   strictly before the due date).
    /// * `InvoiceError::CrossContractCallFailed` if token transfer, escrow, or pool
    ///   repayment accounting fails.
    ///
    /// # Returns
    /// * `bool` - `true` when early repayment is completed.
    ///
    /// # Example
    /// ```ignore
    /// client.repay_early(&invoice_id);
    /// ```
    pub fn repay_early(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        invoice.buyer.require_auth();
        let _prev_status = invoice.status;
        if invoice.status != InvoiceStatus::Confirmed {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        let pool: Address = invoice
            .funding_pool
            .clone()
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        let face_value = invoice.face_value;
        let funded_amount = invoice.funded_amount;
        let discount = face_value.saturating_sub(funded_amount);

        let funded_at = invoice
            .funded_at
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        let now = env.ledger().timestamp();
        if now >= invoice.due_date {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        let _prev_status = invoice.status;
        let term = invoice.due_date.saturating_sub(funded_at);
        let elapsed = now.saturating_sub(funded_at);

        // Floor proportional discount accrual; the buyer receives the
        // remainder so earned amount plus refund equals the full discount.
        let earned_by_pool = if term == 0 {
            discount
        } else {
            discount
                .checked_mul(elapsed as u128)
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::MathOverflow))
                / (term as u128)
        };
        let refund_to_buyer = discount.saturating_sub(earned_by_pool);

        let buyer = invoice.buyer.clone();
        let funding_asset = invoice.funding_asset.clone();

        // Route repayment through escrow so escrow remains the secure
        // intermediary for all fund movements (fixes issue #59).
        // Flow: buyer → escrow → pool (via escrow::release_to_pool)
        let escrow: Address = env
            .storage()
            .instance()
            .get(&DataKey::EscrowContract)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        let token = token::Client::new(&env, &funding_asset);
        // Step 1: buyer transfers face_value into escrow
        if !matches!(
            token.try_transfer(&buyer, &escrow, &(face_value as i128)),
            Ok(Ok(()))
        ) {
            panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
        }

        // Step 2: escrow releases face_value back to pool
        let mut escrow_args = Vec::new(&env);
        escrow_args.push_back(invoice_id.clone().into_val(&env));
        escrow_args.push_back(face_value.into_val(&env));
        let escrow_released: bool =
            env.invoke_contract(&escrow, &Symbol::new(&env, "release_to_pool"), escrow_args);
        if !escrow_released {
            panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
        }

        // Step 3: notify pool to update its internal accounting
        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        args.push_back(face_value.into_val(&env));
        args.push_back(refund_to_buyer.into_val(&env));
        args.push_back(buyer.into_val(&env));
        let repayment_recorded: bool = env.invoke_contract(
            &pool,
            &Symbol::new(&env, "receive_repayment_with_refund"),
            args,
        );
        if !repayment_recorded {
            panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
        }

        let mut updated = invoice;
        updated.status = InvoiceStatus::Repaid;
        updated.repaid_at = Some(now);
        updated.repaid_amount = face_value;
        updated.remaining_balance = 0;
        Self::save_invoice(&env, inv_key, &updated);
        Self::extend_instance_ttl(&env);

        move_status_index(
            &env,
            &invoice_id,
            InvoiceStatus::Confirmed,
            InvoiceStatus::Repaid,
        );
        events::invoice_repaid(&env, &invoice_id, updated.face_value);
        true
    }

    /// Triggers default on a past-due invoice.
    ///
    /// Default is permitted once `now >= due_date` — the due date has been
    /// reached or passed. This is consistent with the `create` check that
    /// rejects `due_date <= now` (due dates must be in the future).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to default.
    ///
    /// # Auth
    /// No authorization is required. Anyone may trigger a default once the due
    /// date has passed. This removes the single-point-of-failure risk of an
    /// admin-only gate and ensures LP loss recognition is timely.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice or funding pool cannot be found.
    /// * `InvoiceError::InvalidStatusTransition` if invoice is not `Funded`, `Active`, or `Confirmed`.
    /// * `InvoiceError::DueDateNotPassed` if `now < due_date` — the due date
    ///   has not yet been reached.
    /// * `InvoiceError::CrossContractCallFailed` if the pool returns `false` from
    ///   `handle_default`.
    ///
    /// # Returns
    /// * `bool` - `true` when default processing succeeds.
    ///
    /// # Example
    /// ```ignore
    /// client.trigger_default(&invoice_id);
    /// ```
    ///
    /// # Coupling with escrow's minimum lock window
    /// This function's due-date gate (`now >= due_date`) is independent of,
    /// and has no awareness of, the escrow contract's own
    /// `DEFAULT_MIN_LOCK_SECONDS` grace period (60s from the escrow lock
    /// timestamp, roughly `funded_at`). If `due_date` is reached less than
    /// that window after the invoice was funded, the downstream default call
    /// may panic from the escrow constraint. The whole transaction reverts, so
    /// the invoice remains unchanged. The call transitively invokes
    /// `escrow.handle_default()` (via `pool.handle_default`), which may panic
    /// with `EscrowError::NotAuthorized`. The whole transaction reverts, so
    /// there is no persistent state inconsistency, but the caller sees a
    /// revert originating from a constraint this contract does not itself
    /// enforce or expose. See
    /// `test_trigger_default_reverts_when_escrow_grace_period_not_elapsed`
    /// for a pinned repro.
    ///
    pub fn trigger_default(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        let valid_transition = invoice.status == InvoiceStatus::Funded
            || invoice.status == InvoiceStatus::Active
            || invoice.status == InvoiceStatus::Confirmed;
        if !valid_transition {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }
        if env.ledger().timestamp() < invoice.due_date {
            panic_with_error!(&env, InvoiceError::DueDateNotPassed);
        }

        let prev_status = invoice.status;

        let pool: Address = invoice
            .funding_pool
            .clone()
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        let mut args = Vec::new(&env);
        args.push_back(invoice_id.clone().into_val(&env));
        let default_handled: bool =
            env.invoke_contract(&pool, &Symbol::new(&env, "handle_default"), args);
        if !default_handled {
            panic_with_error!(&env, InvoiceError::CrossContractCallFailed);
        }

        let current: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        if current.status == prev_status {
            invoice.status = InvoiceStatus::Defaulted;
            Self::save_invoice(&env, inv_key, &invoice);
            Self::extend_instance_ttl(&env);
            move_status_index(&env, &invoice_id, prev_status, InvoiceStatus::Defaulted);
            events::invoice_defaulted(&env, &invoice_id);
        }
        true
    }

    /// Marks an invoice as defaulted, called by the pool contract during
    /// default processing.
    ///
    /// This function is invoked as a cross-contract call from
    /// `pool.handle_default()` and is responsible for persisting the
    /// `Defaulted` status on the invoice record, updating the status index,
    /// and emitting the `invoice_defaulted` event.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to mark as defaulted.
    ///
    /// # Auth
    /// Requires authorization from the invoice's funding pool contract
    /// (retrieved from the invoice record's `funding_pool` field).
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice cannot be found or has no
    ///   recorded funding pool.
    /// * `InvoiceError::InvalidStatusTransition` if the invoice status is not
    ///   `Funded`, `Active`, or `Confirmed` (or already `Defaulted`, which is
    ///   accepted as a no-op for idempotency).
    ///
    /// # Returns
    /// * `bool` - `true` when the invoice is marked as defaulted.
    ///
    /// # Example
    /// ```ignore
    /// client.mark_defaulted(&invoice_id);
    /// ```
    pub fn mark_defaulted(env: Env, invoice_id: BytesN<32>) -> bool {
        require_not_paused(&env);
        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        let pool: Address = invoice
            .funding_pool
            .clone()
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        pool.require_auth();

        // Allow transition from Funded, Active, or Confirmed to Defaulted.
        // If already Defaulted, treat as a no-op for idempotency (e.g., when
        // trigger_default already performed the transition before calling
        // pool.handle_default).
        let valid_transition = invoice.status == InvoiceStatus::Funded
            || invoice.status == InvoiceStatus::Active
            || invoice.status == InvoiceStatus::Confirmed;
        if !valid_transition {
            if invoice.status == InvoiceStatus::Defaulted {
                return true;
            }
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        let prev_status = invoice.status;
        invoice.status = InvoiceStatus::Defaulted;
        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);

        move_status_index(&env, &invoice_id, prev_status, InvoiceStatus::Defaulted);
        events::invoice_defaulted(&env, &invoice_id);
        true
    }

    pub fn set_expiry_window(env: Env, window: u64) {
        require_not_paused(&env);
        if window > 31_536_000u64 {
            panic_with_error!(&env, InvoiceError::InvalidExpiryWindow);
        }
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::ExpiryWindow, &window);
        events::expiry_window_set(&env, window);
        Self::extend_instance_ttl(&env);
    }

    /// Returns the current listing expiry window in seconds.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `u64` - The expiry window in seconds. Defaults to 7 days (`7 * 24 * 60 * 60`) if unset.
    ///
    /// # Example
    /// ```ignore
    /// let window = client.get_expiry_window();
    /// ```
    pub fn get_expiry_window(env: Env) -> u64 {
        Self::expiry_window(&env)
    }

    /// Expires a listing whose expiry window has passed.
    ///
    /// Callable by the invoice's stored issuer or the contract admin — the
    /// same dual-caller pattern as `confirm_delivery` and escrow's
    /// `handle_default`: `caller` must explicitly authorize, and is then
    /// verified against the stored issuer and admin. The previous
    /// `try_invoke_contract`-based issuer probe (a private `check_auth`
    /// self-call) violated Soroban's no-re-entry rule under real signatures,
    /// so the issuer path could never authenticate outside mocked tests.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice whose listing should be expired.
    /// * `caller` - The address authorizing the expiry (issuer or admin).
    ///
    /// # Auth
    /// Requires authorization from `caller`, who must be the invoice's stored
    /// issuer or the stored admin address.
    ///
    /// # Panics
    /// * `InvoiceError::NotFound` if the invoice or admin cannot be found.
    /// * `InvoiceError::InvalidStatusTransition` if invoice status is not `Listed`.
    /// * `InvoiceError::NotAuthorized` if `caller` is neither the issuer nor the admin.
    /// * `InvoiceError::ListingNotExpired` if `now < listed_at + expiry_window`.
    /// * `InvoiceError::MathOverflow` if `listed_at + expiry_window` overflows.
    ///
    /// # Returns
    /// * `bool` - `true` when the listing is expired.
    ///
    /// # Example
    /// ```ignore
    /// client.expire_listing(&invoice_id, &issuer);
    /// ```
    pub fn expire_listing(env: Env, invoice_id: BytesN<32>, caller: Address) -> bool {
        require_not_paused(&env);
        caller.require_auth();

        let inv_key = DataKey::Invoice(invoice_id.clone());
        let mut invoice: Invoice = env
            .storage()
            .persistent()
            .get(&inv_key)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

        if invoice.status != InvoiceStatus::Listed {
            panic_with_error!(&env, InvoiceError::InvalidStatusTransition);
        }

        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));

        if caller != invoice.issuer && caller != admin {
            panic_with_error!(&env, InvoiceError::NotAuthorized);
        }

        let listed_at = invoice.listed_at.unwrap_or(0);
        let current_time = env.ledger().timestamp();
        let deadline = Self::listing_expiry_timestamp(&env, listed_at);

        if current_time < deadline {
            panic_with_error!(&env, InvoiceError::ListingNotExpired);
        }

        let prev_status = invoice.status;
        invoice.status = InvoiceStatus::Expired;
        Self::save_invoice(&env, inv_key, &invoice);
        Self::extend_instance_ttl(&env);

        move_status_index(&env, &invoice_id, prev_status, InvoiceStatus::Expired);
        events::invoice_expired(&env, &invoice_id);
        true
    }

    /// Returns the status code of an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `u32` - The invoice status as a numeric code.
    ///
    /// # Example
    /// ```ignore
    /// let status = client.get_status(&invoice_id);
    /// ```
    pub fn get_status(env: Env, invoice_id: BytesN<32>) -> u32 {
        Self::get_invoice(&env, invoice_id).status as u32
    }

    /// Returns `true` when `invoice_id` is currently a member of `status`'s
    /// index.
    ///
    /// Backed by [`DataKey::StatusPosition`], so the check is O(1): a single
    /// persistent-storage read whose cost does not grow with the number of
    /// invoices sharing that status. The position is written when an invoice
    /// enters a status (`create` / [`Self::list_for_financing`] and every
    /// status transition routed through `move_status_index`) and removed when
    /// it leaves, so a `false` result means the invoice is not — or is no
    /// longer — in that status. Pre-upgrade rows without a position fall back
    /// to the legacy membership marker until migrated.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `status` - The status to check membership against.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic. An unknown `invoice_id` simply returns `false`.
    ///
    /// # Returns
    /// * `bool` - `true` if the invoice currently belongs to `status`.
    ///
    /// # Example
    /// ```ignore
    /// let listed = client.has_status_membership(&InvoiceStatus::Listed, &invoice_id);
    /// ```
    pub fn has_status_membership(env: Env, status: InvoiceStatus, invoice_id: BytesN<32>) -> bool {
        read_status_membership(&env, status, &invoice_id)
    }

    /// Returns the face value of an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `u128` - The invoice face value.
    ///
    /// # Example
    /// ```ignore
    /// let face_value = client.get_face_value(&invoice_id);
    /// ```
    pub fn get_face_value(env: Env, invoice_id: BytesN<32>) -> u128 {
        Self::get_invoice(&env, invoice_id).face_value
    }

    /// Returns the amount committed when an invoice was funded.
    ///
    /// This is the value `mark_funded` recorded in [`Invoice::funded_amount`],
    /// denominated in USDC stroops. The accessor is read-only: it requires no
    /// authorization, writes no state, and does not extend TTL.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    /// * `InvoiceError::NotFunded` if the invoice exists but has not been
    ///   funded yet. An invoice that has never been funded has no committed
    ///   amount, so this is reported explicitly rather than as `0`.
    ///
    /// # Returns
    /// * `u128` - The amount committed at funding, in USDC stroops.
    ///
    /// # Example
    /// ```ignore
    /// let funded_amount = client.get_funded_amount(&invoice_id);
    /// ```
    pub fn get_funded_amount(env: Env, invoice_id: BytesN<32>) -> u128 {
        let invoice = Self::get_invoice(&env, invoice_id);
        if invoice.funded_at.is_none() {
            panic_with_error!(&env, InvoiceError::NotFunded);
        }
        invoice.funded_amount
    }

    /// Returns the remaining balance to be repaid on an invoice.
    pub fn get_remaining_balance(env: Env, invoice_id: BytesN<32>) -> u128 {
        Self::get_invoice(&env, invoice_id).remaining_balance
    }

    /// Returns the cumulative amount repaid on an invoice.
    pub fn get_repaid_amount(env: Env, invoice_id: BytesN<32>) -> u128 {
        Self::get_invoice(&env, invoice_id).repaid_amount
    }

    /// Returns the discount basis points for an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `u32` - The discount rate in basis points.
    ///
    /// # Example
    /// ```ignore
    /// let discount = client.get_discount_bps(&invoice_id);
    /// ```
    pub fn get_discount_bps(env: Env, invoice_id: BytesN<32>) -> u32 {
        Self::get_invoice(&env, invoice_id).discount_bps
    }

    /// Returns (status, face_value, discount_bps) in a single cross-contract call.
    ///
    /// # Arguments
    /// - `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// - None.
    ///
    /// # Panics
    /// - If the invoice does not exist (`InvoiceError::NotFound`).
    ///
    /// # Returns
    /// A tuple of `(invoice_status as u32, face_value, discount_bps)`.
    pub fn get_funding_terms(env: Env, invoice_id: BytesN<32>) -> (u32, u128, u32) {
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(invoice_id))
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
        (
            invoice.status as u32,
            invoice.face_value,
            invoice.discount_bps,
        )
    }

    /// Returns the funding asset for an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `Address` - The funding asset address.
    ///
    /// # Example
    /// ```ignore
    /// let asset = client.get_funding_asset(&invoice_id);
    /// ```
    pub fn get_funding_asset(env: Env, invoice_id: BytesN<32>) -> Address {
        Self::get_invoice(&env, invoice_id).funding_asset
    }

    /// Retrieves the full invoice record by ID.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to retrieve.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `Invoice` - The full invoice object.
    ///
    /// # Example
    /// ```ignore
    /// let invoice = client.get(&invoice_id);
    /// ```
    pub fn get(env: Env, invoice_id: BytesN<32>) -> Invoice {
        Self::get_invoice(&env, invoice_id)
    }

    /// Lists a page of invoices for a given status.
    ///
    /// The index stores only current members: entering a status appends to the
    /// compact slot array and leaving it swap-and-pops the departed slot using
    /// the O(1) [`DataKey::StatusPosition`] map. A page therefore hydrates only
    /// the requested members, never the full historical invoice list, and its
    /// cost is bounded by `page_size` (issue #71 / #22).
    ///
    /// # Ordering
    ///
    /// Results follow the compacted slot order. This preserves insertion order
    /// while no member leaves; a departure moves the previous last member into
    /// the vacated slot, matching the documented swap-and-pop compaction
    /// behavior (see `test_move_status_index_compacts_storage`).
    ///
    /// # Legacy rows
    ///
    /// Rows written before the position map existed remain in the array until
    /// they are migrated or transition. Each page is still filtered to its
    /// *current* status, so legacy stale rows never leak into results; such a
    /// page can be shorter than `page_size` until
    /// [`Self::migrate_status_index`] finishes.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `status` - The invoice status filter.
    /// * `page` - Zero-based page index. Pages starting past the end of the
    ///   index return an empty `Vec`.
    /// * `page_size` - Maximum invoices to return for this page. Must be
    ///   `<= MAX_PAGE_SIZE`.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::InvalidPageSize` if `page_size > MAX_PAGE_SIZE`.
    ///
    /// # Returns
    /// * `Vec<Invoice>` - The invoices matching the status on the requested page.
    ///
    /// # Example
    /// ```ignore
    /// let invoices = client.get_by_status(&InvoiceStatus::Created, 0, 20);
    /// ```
    pub fn get_by_status(
        env: Env,
        status: InvoiceStatus,
        page: u32,
        page_size: u32,
    ) -> Vec<Invoice> {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::StatusIndexCount(status as u32))
            .unwrap_or(0);
        let (start, end) = page_range(&env, count, page, page_size);
        let mut ids: Vec<BytesN<32>> = Vec::new(&env);
        for i in start..end {
            let id: BytesN<32> = env
                .storage()
                .persistent()
                .get(&DataKey::StatusIndexEntry(status as u32, i))
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            ids.push_back(id);
        }
        let invoices = hydrate_ids(&env, ids);
        let mut result: Vec<Invoice> = Vec::new(&env);
        for i in 0..invoices.len() {
            let invoice = invoices
                .get(i)
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            if invoice.status == status {
                result.push_back(invoice);
            }
        }
        result
    }

    /// Lists a page of invoices created by a given issuer.
    ///
    /// Pagination bounds the number of invoices hydrated per call, keeping the
    /// read within the Soroban CPU/memory budget (issue #71).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `address` - The issuer address.
    /// * `page` - Zero-based page index. Pages starting past the end of the
    ///   index return an empty `Vec`.
    /// * `page_size` - Maximum invoices to return for this page. Must be
    ///   `<= MAX_PAGE_SIZE`.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::InvalidPageSize` if `page_size > MAX_PAGE_SIZE`.
    ///
    /// # Returns
    /// * `Vec<Invoice>` - The invoices for the issuer on the requested page.
    ///
    /// # Example
    /// ```ignore
    /// let invoices = client.get_by_issuer(&issuer, 0, 20);
    /// ```
    pub fn get_by_issuer(env: Env, address: Address, page: u32, page_size: u32) -> Vec<Invoice> {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::IssuerIndexCount(address.clone()))
            .unwrap_or(0);
        let (start, end) = page_range(&env, count, page, page_size);
        let mut ids: Vec<BytesN<32>> = Vec::new(&env);
        for i in start..end {
            let id: BytesN<32> = env
                .storage()
                .persistent()
                .get(&DataKey::IssuerIndexEntry(address.clone(), i))
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            ids.push_back(id);
        }
        hydrate_ids(&env, ids)
    }

    /// Lists a page of invoices associated with a given buyer.
    ///
    /// Pagination bounds the number of invoices hydrated per call, keeping the
    /// read within the Soroban CPU/memory budget (issue #71).
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `address` - The buyer address.
    /// * `page` - Zero-based page index. Pages starting past the end of the
    ///   index return an empty `Vec`.
    /// * `page_size` - Maximum invoices to return for this page. Must be
    ///   `<= MAX_PAGE_SIZE`.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::InvalidPageSize` if `page_size > MAX_PAGE_SIZE`.
    ///
    /// # Returns
    /// * `Vec<Invoice>` - The invoices for the buyer on the requested page.
    ///
    /// # Example
    /// ```ignore
    /// let invoices = client.get_by_buyer(&buyer, 0, 20);
    /// ```
    pub fn get_by_buyer(env: Env, address: Address, page: u32, page_size: u32) -> Vec<Invoice> {
        let count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::BuyerIndexCount(address.clone()))
            .unwrap_or(0);
        let (start, end) = page_range(&env, count, page, page_size);
        let mut ids: Vec<BytesN<32>> = Vec::new(&env);
        for i in start..end {
            let id: BytesN<32> = env
                .storage()
                .persistent()
                .get(&DataKey::BuyerIndexEntry(address.clone(), i))
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            ids.push_back(id);
        }
        hydrate_ids(&env, ids)
    }

    /// Returns the number of invoices created by a given issuer.
    ///
    /// Reads only the issuer index counter, so it is much cheaper than
    /// [`get_by_issuer`](Self::get_by_issuer), which hydrates every record.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `address` - The issuer address.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `u32` - The number of invoices issued by `address`, or `0` if the
    ///   address has never issued an invoice.
    ///
    /// # Example
    /// ```ignore
    /// let count = client.get_invoice_count_by_issuer(&issuer);
    /// ```
    pub fn get_invoice_count_by_issuer(env: Env, address: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::IssuerIndexCount(address))
            .unwrap_or(0)
    }

    /// Returns the number of invoices associated with a given buyer.
    ///
    /// Reads only the buyer index counter, so it is much cheaper than
    /// [`get_by_buyer`](Self::get_by_buyer), which hydrates every record.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `address` - The buyer address.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `u32` - The number of invoices bought by `address`, or `0` if the
    ///   address has never bought an invoice.
    ///
    /// # Example
    /// ```ignore
    /// let count = client.get_invoice_count_by_buyer(&buyer);
    /// ```
    pub fn get_invoice_count_by_buyer(env: Env, address: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::BuyerIndexCount(address))
            .unwrap_or(0)
    }

    /// Returns a map of invoice counts keyed by status name.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// Does not panic.
    ///
    /// # Returns
    /// * `Map<String, u64>` - Counts for each `InvoiceStatus` variant.
    ///
    /// # Example
    /// ```ignore
    /// let counts = client.get_counts();
    /// ```
    pub fn get_counts(env: Env) -> Map<String, u64> {
        let mut counts: Map<String, u64> = Map::new(&env);
        let statuses = [
            InvoiceStatus::Created,
            InvoiceStatus::Listed,
            InvoiceStatus::Funded,
            InvoiceStatus::Active,
            InvoiceStatus::Confirmed,
            InvoiceStatus::Repaid,
            InvoiceStatus::Defaulted,
            InvoiceStatus::Expired,
            InvoiceStatus::Cancelled,
        ];
        for status in statuses {
            let key = String::from_str(&env, status.as_str());
            let value = read_status_count(&env, status);
            counts.set(key, value);
        }
        counts
    }

    /// Migrates one status's pre-upgrade index to the bounded membership
    /// index, one bounded page at a time.
    ///
    /// Call repeatedly with increasing progress until the returned `done` flag
    /// is `true`. Each call processes at most `page_size` source slots
    /// (`page_size <= MAX_PAGE_SIZE`), reads each invoice once, rewrites only
    /// live members into the compact prefix, and records their O(1)
    /// [`DataKey::StatusPosition`]. On the final page, stale trailing slots
    /// and the old legacy markers are removed.
    ///
    /// # Migration note
    ///
    /// Upgrading from the old append-only shape does not require downtime for
    /// correctness: transitions lazily seed a missing position with a one-time
    /// compatibility scan, and `get_by_status` keeps filtering each page. Run
    /// this function for every status (ideally while paused) to remove stale
    /// historical rows and to make all transitions fully O(1) without a
    /// lazy scan. Repeat for `Created`, `Listed`, `Funded`, `Active`,
    /// `Confirmed`, `Repaid`, `Defaulted`, `Expired`, and `Cancelled`.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `status` - The status index to migrate.
    /// * `page_size` - Maximum source slots to scan in this call; must be in
    ///   `1..=MAX_PAGE_SIZE`.
    ///
    /// # Auth
    /// Requires authorization from the stored admin.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::InvalidPageSize` if `page_size` is zero or greater than
    ///   [`MAX_PAGE_SIZE`].
    /// * `InvoiceError::NotFound` if an indexed invoice record is missing.
    ///
    /// # Returns
    /// * `(u32, bool)` - `(slots_scanned, done)` where `slots_scanned` is the
    ///   source cursor after this call.
    ///
    /// # Example
    /// ```ignore
    /// let (cursor, done) = client.migrate_status_index(&InvoiceStatus::Created, &50);
    /// ```
    pub fn migrate_status_index(
        env: Env,
        status: InvoiceStatus,
        page_size: u32,
    ) -> (u32, bool) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        if page_size == 0 || page_size > MAX_PAGE_SIZE {
            panic_with_error!(&env, InvoiceError::InvalidPageSize);
        }

        let status_u32 = status as u32;
        let count_key = DataKey::StatusIndexCount(status_u32);
        let source_count: u32 = env
            .storage()
            .persistent()
            .get(&count_key)
            .unwrap_or(0);

        let cursor_key = DataKey::StatusMigrationCursor(status);
        let kept_key = DataKey::StatusMigrationKept(status);
        let cursor: u32 = env
            .storage()
            .persistent()
            .get(&cursor_key)
            .unwrap_or(0);
        let mut kept: u32 = env.storage().persistent().get(&kept_key).unwrap_or(0);

        let end = core::cmp::min(cursor.saturating_add(page_size), source_count);
        for i in cursor..end {
            let entry_key = DataKey::StatusIndexEntry(status_u32, i);
            let id: BytesN<32> = env
                .storage()
                .persistent()
                .get(&entry_key)
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));
            let invoice: Invoice = env
                .storage()
                .persistent()
                .get(&DataKey::Invoice(id.clone()))
                .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotFound));

            if invoice.status == status {
                let target_key = DataKey::StatusIndexEntry(status_u32, kept);
                if kept != i {
                    env.storage().persistent().set(&target_key, &id);
                }
                env.storage()
                    .persistent()
                    .extend_ttl(&target_key, TTL_THRESHOLD, TTL_EXTEND_TO);
                write_status_position(&env, status, &id, kept);
                kept += 1;
            } else {
                // Stale historical row: drop any membership artefacts.
                clear_status_membership(&env, status, &id);
            }
        }

        let done = end >= source_count;
        if done {
            let mut i = kept;
            while i < source_count {
                env.storage()
                    .persistent()
                    .remove(&DataKey::StatusIndexEntry(status_u32, i));
                i += 1;
            }
            env.storage().persistent().set(&count_key, &kept);
            env.storage()
                .persistent()
                .extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
            let status_count_key = DataKey::StatusCount(status_u32);
            env.storage()
                .persistent()
                .set(&status_count_key, &(kept as u64));
            env.storage()
                .persistent()
                .extend_ttl(&status_count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
            env.storage().persistent().remove(&cursor_key);
            env.storage().persistent().remove(&kept_key);
        } else {
            env.storage().persistent().set(&cursor_key, &end);
            env.storage()
                .persistent()
                .extend_ttl(&cursor_key, TTL_THRESHOLD, TTL_EXTEND_TO);
            env.storage().persistent().set(&kept_key, &kept);
            env.storage()
                .persistent()
                .extend_ttl(&kept_key, TTL_THRESHOLD, TTL_EXTEND_TO);
        }

        (end, done)
    }

    /// Returns the issuer address for an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `Address` - The issuer address.
    ///
    /// # Example
    /// ```ignore
    /// let issuer = client.get_issuer(&invoice_id);
    /// ```
    pub fn get_issuer(env: Env, invoice_id: BytesN<32>) -> Address {
        Self::get_invoice(&env, invoice_id).issuer
    }

    /// Returns the buyer address for an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `Address` - The buyer address.
    ///
    /// # Example
    /// ```ignore
    /// let buyer = client.get_buyer(&invoice_id);
    /// ```
    pub fn get_buyer(env: Env, invoice_id: BytesN<32>) -> Address {
        Self::get_invoice(&env, invoice_id).buyer
    }

    /// Returns the due date of an invoice.
    ///
    /// # Arguments
    /// * `env` - The Soroban environment.
    /// * `invoice_id` - The invoice to query.
    ///
    /// # Auth
    /// No authorization is required.
    ///
    /// # Panics
    /// * `InvoiceError::NotInitialized` if the contract has not been initialized.
    /// * `InvoiceError::NotFound` if the invoice cannot be found.
    ///
    /// # Returns
    /// * `u64` - The invoice due date as a Unix timestamp.
    ///
    /// # Example
    /// ```ignore
    /// let due = client.get_due_date(&invoice_id);
    /// ```
    pub fn get_due_date(env: Env, invoice_id: BytesN<32>) -> u64 {
        Self::get_invoice(&env, invoice_id).due_date
    }

    /// Transfers invoice-contract admin ownership to `new_admin`.
    ///
    /// Uses the same dual-authorization pattern as `RegistryContract` and
    /// `PoolContract`: both the current admin and `new_admin` must sign, so
    /// ownership cannot be handed to an address that has not consented and a
    /// compromised admin cannot unilaterally install a key it controls.
    /// Emits `ownership_transferred`.
    ///
    /// # Auth
    /// Requires authorization from both the current admin and `new_admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    /// * `ContractPaused` (via `clearledger_pause::require_not_paused`) while
    ///   the circuit breaker is engaged.
    pub fn transfer_ownership(env: Env, new_admin: Address) {
        require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        new_admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::ownership_transferred(&env, &admin, &new_admin);
        Self::extend_instance_ttl(&env);
    }

    /// Engages the emergency circuit breaker.
    ///
    /// While paused every state-changing entry point reverts with
    /// `ContractPaused`, while read-only views (`get`, `get_status`,
    /// `get_counts`, ...) stay callable. Only the stored admin may pause, and
    /// [`Self::unpause`] is intentionally never guarded so a paused contract
    /// can always be resumed. Emits `paused`.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.pause();
    /// ```
    pub fn pause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        set_paused(&env, true);
        events::paused(&env, &admin);
        Self::extend_instance_ttl(&env);
    }

    /// Disengages the emergency circuit breaker, restoring state-changing calls.
    ///
    /// Deliberately *not* guarded by `require_not_paused`: otherwise a paused
    /// contract could never be resumed. Emits `unpaused`.
    ///
    /// # Auth
    /// Requires authorization from the stored `admin`.
    ///
    /// # Panics
    /// * `NotInitialized` if the contract has not been initialized.
    ///
    /// # Example
    /// ```ignore
    /// client.unpause();
    /// ```
    pub fn unpause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .unwrap_or_else(|| panic_with_error!(&env, InvoiceError::NotInitialized));
        admin.require_auth();
        set_paused(&env, false);
        events::unpaused(&env, &admin);
        Self::extend_instance_ttl(&env);
    }

    /// Rejects wiring an address that aliases a reserved or already-configured
    /// contract role. `other_wired_contracts` excludes the slot being updated.
    fn assert_valid_wiring_address(
        env: &Env,
        candidate: &Address,
        admin: &Address,
        registry_contract: Option<Address>,
        other_wired_contracts: Vec<Address>,
    ) {
        if candidate == admin
            || candidate == &env.current_contract_address()
            || registry_contract.as_ref() == Some(candidate)
        {
            panic_with_error!(env, InvoiceError::InvalidConfiguration);
        }
        for configured in other_wired_contracts {
            if &configured == candidate {
                panic_with_error!(env, InvoiceError::InvalidConfiguration);
            }
        }
    }

    fn require_initialized(env: &Env) {
        if !env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(env, InvoiceError::NotInitialized);
        }
    }

    fn get_invoice(env: &Env, invoice_id: BytesN<32>) -> Invoice {
        Self::require_initialized(env);
        env.storage()
            .persistent()
            .get(&DataKey::Invoice(invoice_id))
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound))
    }

    fn extend_instance_ttl(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
    }
}

/// Checks that an address is verified in the registry, panicking with the
/// provided error if not.
fn require_verified(env: &Env, registry_id: &Address, addr: &Address, err: InvoiceError) {
    let mut args = Vec::new(env);
    args.push_back(addr.clone().into_val(env));
    let verified: bool = env.invoke_contract(registry_id, &Symbol::new(env, "is_verified"), args);
    if !verified {
        panic_with_error!(env, err);
    }
}

/// Adds an invoice ID to the issuer's index if not already present.
///
/// # Arguments
/// * `env` - The Soroban environment.
/// * `issuer` - The issuer address.
/// * `invoice_id` - The invoice ID to add.
///
/// # Panics
/// Panics with `CounterOverflow` when the index has no remaining capacity.
///
/// # Returns
/// * `()` - No value is returned.
fn extend_issuer_index(env: &Env, issuer: &Address, invoice_id: &BytesN<32>) {
    let count_key = DataKey::IssuerIndexCount(issuer.clone());
    let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
    let next_count = count
        .checked_add(1)
        .unwrap_or_else(|| panic_with_error!(env, InvoiceError::CounterOverflow));

    // Check if invoice_id already exists in this issuer index
    for i in 0..count {
        let entry_key = DataKey::IssuerIndexEntry(issuer.clone(), i);
        let existing_id: BytesN<32> = env
            .storage()
            .persistent()
            .get(&entry_key)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
        if existing_id == *invoice_id {
            return; // Already exists, skip duplicate
        }
    }

    let entry_key = DataKey::IssuerIndexEntry(issuer.clone(), count);
    env.storage().persistent().set(&entry_key, invoice_id);
    env.storage().persistent().set(&count_key, &next_count);
    env.storage()
        .persistent()
        .extend_ttl(&entry_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    env.storage()
        .persistent()
        .extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Adds an invoice ID to the buyer's index if not already present.
///
/// # Arguments
/// * `env` - The Soroban environment.
/// * `buyer` - The buyer address.
/// * `invoice_id` - The invoice ID to add.
///
/// # Panics
/// Panics with `CounterOverflow` when the index has no remaining capacity.
///
/// # Returns
/// * `()` - No value is returned.
fn extend_buyer_index(env: &Env, buyer: &Address, invoice_id: &BytesN<32>) {
    let count_key = DataKey::BuyerIndexCount(buyer.clone());
    let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
    let next_count = count
        .checked_add(1)
        .unwrap_or_else(|| panic_with_error!(env, InvoiceError::CounterOverflow));

    // Check if invoice_id already exists in this buyer index
    for i in 0..count {
        let entry_key = DataKey::BuyerIndexEntry(buyer.clone(), i);
        let existing_id: BytesN<32> = env
            .storage()
            .persistent()
            .get(&entry_key)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
        if existing_id == *invoice_id {
            return; // Already exists, skip duplicate
        }
    }

    let entry_key = DataKey::BuyerIndexEntry(buyer.clone(), count);
    env.storage().persistent().set(&entry_key, invoice_id);
    env.storage().persistent().set(&count_key, &next_count);
    env.storage()
        .persistent()
        .extend_ttl(&entry_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    env.storage()
        .persistent()
        .extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Adds an invoice ID to the status index if not already present.
///
/// Membership is the O(1) [`DataKey::StatusPosition`] map (full invoice id ->
/// slot), so this never scans the status's historical entries. The id is
/// appended at the current slot count and the position record is written with
/// the usual `TTL_THRESHOLD`/`TTL_EXTEND_TO` extension. Because a leaving
/// invoice is swap-and-popped by [`remove_from_status_index`], the slot array
/// only ever contains current members and cannot grow with historical
/// transitions.
///
/// # Arguments
/// * `env` - The Soroban environment.
/// * `status` - The invoice status.
/// * `invoice_id` - The invoice ID to add.
///
/// # Panics
/// Panics with `CounterOverflow` when the index has no remaining capacity.
///
/// # Returns
/// * `()` - No value is returned.
fn extend_status_index(env: &Env, status: InvoiceStatus, invoice_id: &BytesN<32>) {
    // Position presence is authoritative membership. The legacy short-id
    // marker is deliberately not consulted here: it is only a read fallback
    // for data written before this index shape existed.
    if read_status_position(env, status, invoice_id).is_some() {
        return; // Already a member, skip duplicate
    }

    let status_u32 = status as u32;
    let count_key = DataKey::StatusIndexCount(status_u32);
    let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
    let next_count = count
        .checked_add(1)
        .unwrap_or_else(|| panic_with_error!(env, InvoiceError::CounterOverflow));

    let entry_key = DataKey::StatusIndexEntry(status_u32, count);
    env.storage().persistent().set(&entry_key, invoice_id);
    env.storage().persistent().set(&count_key, &next_count);
    env.storage()
        .persistent()
        .extend_ttl(&entry_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    env.storage()
        .persistent()
        .extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
    write_status_position(env, status, invoice_id, count);
}

/// Moves an invoice ID from one status index to another, with idempotency for replayed transitions.
///
/// Both sides of the move are O(1): the target status is checked through its
/// [`DataKey::StatusPosition`] map, the source position is looked up directly
/// for a swap-and-pop removal, and `extend_status_index` appends to the
/// target. No step scans the status's historical invoice list. A replayed
/// transition returns early without modifying counts or indexes.
///
/// # Arguments
/// * `env` - The Soroban environment.
/// * `invoice_id` - The invoice ID to move.
/// * `from` - The source status.
/// * `to` - The target status.
///
/// # Panics
/// * `InvoiceError::InvalidStatusTransition` if the source status count underflows.
///
/// # Returns
/// * `()` - No value is returned.
fn move_status_index(env: &Env, invoice_id: &BytesN<32>, from: InvoiceStatus, to: InvoiceStatus) {
    // O(1) idempotency check for replayed transitions: membership in the
    // target status means every step below has already been applied.
    if read_status_membership(env, to, invoice_id) {
        return;
    }

    decrement_status_count(env, from);
    increment_status_count(env, to);
    remove_from_status_index(env, from, invoice_id);
    extend_status_index(env, to, invoice_id);
}

/// Removes an invoice from the `StatusIndexEntry` array for `status` using a
/// swap-and-pop strategy: the entry holding `invoice_id` is overwritten with
/// the last entry in the array, the moved invoice's position is rewritten, and
/// the now-trailing key is deleted from storage. `StatusIndexCount` is
/// decremented accordingly.
///
/// The source slot comes from [`DataKey::StatusPosition`], so a normal
/// transition performs a constant number of reads/writes regardless of how
/// many invoices share the status. The slot array holds only current members
/// (plus any pre-upgrade rows awaiting migration), so it cannot grow with
/// historical transitions (issues #22 / #435).
///
/// If the position is absent — an invoice written before this storage shape
/// existed — the function performs a one-time compatibility scan to locate the
/// legacy slot, records the position for future O(1) transitions, and then
/// compacts normally. If the id is not present at all, the function is a
/// silent no-op so that legacy transitions don't panic.
fn remove_from_status_index(env: &Env, status: InvoiceStatus, invoice_id: &BytesN<32>) {
    let status_u32 = status as u32;
    let count_key = DataKey::StatusIndexCount(status_u32);
    let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);

    let pos = match read_status_position(env, status, invoice_id) {
        Some(pos) => pos,
        None => {
            // Legacy compatibility path: older rows have no position map. Find
            // the slot once, seed the map, and from then on removal is O(1).
            let mut found: Option<u32> = None;
            for i in 0..count {
                let entry_id: BytesN<32> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::StatusIndexEntry(status_u32, i))
                    .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
                if entry_id == *invoice_id {
                    found = Some(i);
                    break;
                }
            }
            match found {
                Some(pos) => {
                    write_status_position(env, status, invoice_id, pos);
                    pos
                }
                None => {
                    clear_status_membership(env, status, invoice_id);
                    return;
                }
            }
        }
    };

    if pos >= count {
        // Corrupt/legacy position: do not index past the live slot array.
        clear_status_membership(env, status, invoice_id);
        return;
    }

    let last = count - 1;
    if pos != last {
        // Swap the found entry with the last entry.
        let last_id: BytesN<32> = env
            .storage()
            .persistent()
            .get(&DataKey::StatusIndexEntry(status_u32, last))
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
        let swap_key = DataKey::StatusIndexEntry(status_u32, pos);
        env.storage().persistent().set(&swap_key, &last_id);
        env.storage()
            .persistent()
            .extend_ttl(&swap_key, TTL_THRESHOLD, TTL_EXTEND_TO);

        if read_status_position(env, status, &last_id).is_some() {
            write_status_position(env, status, &last_id, pos);
        } else {
            // Legacy row without a position: only claim membership if the
            // invoice really is in this status. Otherwise leave it as a stale
            // row for `migrate_status_index` to drop (reads already filter it).
            let last_invoice: Invoice = env
                .storage()
                .persistent()
                .get(&DataKey::Invoice(last_id.clone()))
                .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
            if last_invoice.status == status {
                write_status_position(env, status, &last_id, pos);
            } else {
                clear_status_membership(env, status, &last_id);
            }
        }
    }

    // Remove the trailing entry, the leaving invoice's position, and update
    // the slot count.
    env.storage()
        .persistent()
        .remove(&DataKey::StatusIndexEntry(status_u32, last));
    clear_status_membership(env, status, invoice_id);
    env.storage().persistent().set(&count_key, &last);
    env.storage()
        .persistent()
        .extend_ttl(&count_key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

/// Storage key for the legacy status-membership marker.
///
/// `DataKey::StatusMembership` is declared as `(InvoiceStatus, u64)`, so the
/// 32-byte invoice ID is projected onto its first 8 bytes (big-endian). This
/// marker predates the bounded index and is retained only so pre-upgrade
/// invoices still answer [`InvoiceContract::has_status_membership`] until they
/// are migrated or transition once. New writes never create it.
fn status_membership_key(status: InvoiceStatus, invoice_id: &BytesN<32>) -> DataKey {
    let id_bytes = invoice_id.to_array();
    let mut short_id = [0u8; 8];
    short_id.copy_from_slice(&id_bytes[..8]);
    DataKey::StatusMembership(status, u64::from_be_bytes(short_id))
}

/// Storage key for an invoice's exact slot in its status index.
///
/// The full 32-byte id is used, so unlike the legacy short-id marker this map
/// is collision-free.
fn status_position_key(status: InvoiceStatus, invoice_id: &BytesN<32>) -> DataKey {
    DataKey::StatusPosition(status, invoice_id.clone())
}

/// O(1) position probe: one persistent read, independent of the status size.
fn read_status_position(env: &Env, status: InvoiceStatus, invoice_id: &BytesN<32>) -> Option<u32> {
    env.storage()
        .persistent()
        .get(&status_position_key(status, invoice_id))
}

/// O(1) membership probe: the position map is authoritative for post-upgrade
/// rows; the legacy short-id marker is consulted only as a read fallback for
/// rows written before the position map existed.
fn read_status_membership(env: &Env, status: InvoiceStatus, invoice_id: &BytesN<32>) -> bool {
    if read_status_position(env, status, invoice_id).is_some() {
        return true;
    }
    env.storage()
        .persistent()
        .get::<_, bool>(&status_membership_key(status, invoice_id))
        .unwrap_or(false)
}

/// Records `invoice_id` at `slot` in `status`, extending the entry's TTL with
/// the same threshold/extension policy as the rest of invoice storage.
fn write_status_position(
    env: &Env,
    status: InvoiceStatus,
    invoice_id: &BytesN<32>,
    slot: u32,
) {
    let key = status_position_key(status, invoice_id);
    env.storage().persistent().set(&key, &slot);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
    // Complete the migration for this member: the position is authoritative.
    env.storage()
        .persistent()
        .remove(&status_membership_key(status, invoice_id));
}

/// Removes both the position and the legacy marker when an invoice leaves
/// `status`.
///
/// Removing an entry that is already absent is a silent no-op.
fn clear_status_membership(env: &Env, status: InvoiceStatus, invoice_id: &BytesN<32>) {
    env.storage()
        .persistent()
        .remove(&status_position_key(status, invoice_id));
    env.storage()
        .persistent()
        .remove(&status_membership_key(status, invoice_id));
}

fn increment_status_count(env: &Env, status: InvoiceStatus) {
    let key = DataKey::StatusCount(status as u32);
    let current: u64 = env.storage().persistent().get(&key).unwrap_or(0u64);
    env.storage().persistent().set(&key, &(current + 1));
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn decrement_status_count(env: &Env, status: InvoiceStatus) {
    let key = DataKey::StatusCount(status as u32);
    let current: u64 = env.storage().persistent().get(&key).unwrap_or(0u64);
    let next = current
        .checked_sub(1)
        .unwrap_or_else(|| panic_with_error!(env, InvoiceError::InvalidStatusTransition));
    env.storage().persistent().set(&key, &next);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn read_status_count(env: &Env, status: InvoiceStatus) -> u64 {
    env.storage()
        .persistent()
        .get(&DataKey::StatusCount(status as u32))
        .unwrap_or(0u64)
}

/// Computes the half-open `[start, end)` index range for one page of an index
/// that currently holds `count` entries.
///
/// Returns `(0, 0)` for any request that starts at or past the end of the
/// index (including an overflowing `page * page_size`), so callers naturally
/// yield an empty `Vec` instead of panicking on out-of-range pages.
///
/// # Panics
/// * [`InvoiceError::InvalidPageSize`] if `page_size > MAX_PAGE_SIZE`.
fn page_range(env: &Env, count: u32, page: u32, page_size: u32) -> (u32, u32) {
    if page_size > MAX_PAGE_SIZE {
        panic_with_error!(env, InvoiceError::InvalidPageSize);
    }
    let start = match page.checked_mul(page_size) {
        Some(start) => start,
        // An overflowing offset is necessarily past the end of any index.
        None => return (0, 0),
    };
    if start >= count {
        return (0, 0);
    }
    let end = core::cmp::min(start.saturating_add(page_size), count);
    (start, end)
}

fn hydrate_ids(env: &Env, ids: Vec<BytesN<32>>) -> Vec<Invoice> {
    let mut result: Vec<Invoice> = Vec::new(env);
    for i in 0..ids.len() {
        let id = ids
            .get(i)
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
        let invoice: Invoice = env
            .storage()
            .persistent()
            .get(&DataKey::Invoice(id))
            .unwrap_or_else(|| panic_with_error!(env, InvoiceError::NotFound));
        result.push_back(invoice);
    }
    result
}
