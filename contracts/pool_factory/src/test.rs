extern crate std;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use clearledger_escrow::EscrowContractClient;
use clearledger_invoice::InvoiceContractClient;
use clearledger_pool::PoolContractClient;
use clearledger_registry::{RegistryContract, RegistryContractClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger as _},
    xdr::ToXdr,
    Address, Bytes, BytesN, Env, Map, String, Symbol, TryFromVal, Vec,
};

use crate::{
    DataKey, PoolFactoryContract, PoolFactoryContractClient, PoolStats,
    DEFAULT_MIN_INITIAL_DEPOSIT, DEFAULT_SHARE_DECIMALS, DEFAULT_SHARE_NAME, DEFAULT_SHARE_SYMBOL,
    MAX_AGGREGATE_STATS_PAGE_LIMIT,
};

/// Everything a test needs to drive the factory, already wired together: an
/// initialized factory plus the invoice/escrow/registry contracts and the pool
/// Wasm hash that `register_asset` deploys.
struct TestEnv {
    env: Env,
    factory: PoolFactoryContractClient<'static>,
    factory_id: Address,
    admin: Address,
    asset: Address,
    invoice_id: Address,
    escrow_id: Address,
    registry_id: Address,
    pool_wasm_hash: BytesN<32>,
}

fn setup() -> TestEnv {
    let env = Env::default();
    // `register_asset` requires the factory admin's authorization at the root
    // invocation *and* the freshly deployed pool requires that same admin's
    // authorization from a nested invocation, which only the non-root-capable
    // mock authorizes.
    env.mock_all_auths_allowing_non_root_auth();
    // These tests drive many real cross-contract pool calls, and every pool /
    // invoice entry point now also reads the shared pause flag. That is enough
    // for the longer round trips to cross the default *test-host* budget (not
    // the on-chain budget), so lift it suite-wide. Tests that actually measure
    // gas call `budget().reset_default()` themselves before measuring.
    env.budget().reset_unlimited();

    let admin = Address::generate(&env);
    let asset = Address::generate(&env);
    let registry_id = env.register_contract(None, RegistryContract);
    let registry = RegistryContractClient::new(&env, &registry_id);
    registry.initialize(&admin);

    // Real invoice and escrow contracts: the pool's `initialize` cross-checks
    // the escrow's configured asset and the factory reads the invoice's
    // registry, so both have to be real contracts for the round trip to work.
    let invoice_id = env.register_contract(None, clearledger_invoice::InvoiceContract);
    let invoice = InvoiceContractClient::new(&env, &invoice_id);
    invoice.initialize(&admin, &registry_id);

    let escrow_id = new_escrow(&env, &admin, &asset);

    let factory_id = env.register_contract(None, PoolFactoryContract);
    let factory = PoolFactoryContractClient::new(&env, &factory_id);
    factory.initialize(&admin);

    let pool_wasm_hash = env
        .deployer()
        .upload_contract_wasm(Bytes::from_slice(&env, pool_wasm()));

    TestEnv {
        env,
        factory,
        factory_id,
        admin,
        asset,
        invoice_id,
        escrow_id,
        registry_id,
        pool_wasm_hash,
    }
}

/// Deploys and initializes an escrow contract for `asset`.
///
/// The escrow's stored pool address is irrelevant to the factory flow (the
/// factory only deploys and wires pools, it never moves funds), so a generated
/// address stands in for it.
fn new_escrow(env: &Env, admin: &Address, asset: &Address) -> Address {
    let escrow_id = env.register_contract(None, clearledger_escrow::EscrowContract);
    let escrow = EscrowContractClient::new(env, &escrow_id);
    escrow.initialize(admin, &Address::generate(env), asset);
    escrow_id
}

fn register_asset(te: &TestEnv, asset: &Address) -> Address {
    let predicted_pool = te
        .env
        .deployer()
        .with_address(
            te.factory_id.clone(),
            PoolFactoryContract::asset_salt(&te.env, asset),
        )
        .deployed_address();
    let escrow_id = if *asset == te.asset {
        te.escrow_id.clone()
    } else {
        new_escrow_for_pool(&te.env, &te.admin, asset, &predicted_pool)
    };
    te.factory
        .register_asset(asset, &te.pool_wasm_hash, &te.invoice_id, &escrow_id)
}

/// Creates an initialized pool outside the factory to model a migrated pool.
fn new_initialized_pool(te: &TestEnv, asset: &Address) -> Address {
    let pool_address = te
        .env
        .register_contract(None, clearledger_pool::PoolContract);
    let escrow_id = if *asset == te.asset {
        te.escrow_id.clone()
    } else {
        new_escrow(&te.env, &te.admin, asset)
    };
    PoolContractClient::new(&te.env, &pool_address).initialize(
        &te.admin,
        &te.invoice_id,
        &escrow_id,
        asset,
        &te.registry_id,
        &te.admin,
        &DEFAULT_MIN_INITIAL_DEPOSIT,
        &String::from_str(&te.env, DEFAULT_SHARE_NAME),
        &String::from_str(&te.env, DEFAULT_SHARE_SYMBOL),
        &DEFAULT_SHARE_DECIMALS,
    );
    pool_address
}

fn new_escrow_for_pool(env: &Env, admin: &Address, asset: &Address, pool: &Address) -> Address {
    let escrow_id = env.register_contract(None, clearledger_escrow::EscrowContract);
    let escrow = EscrowContractClient::new(env, &escrow_id);
    escrow.initialize(admin, pool, asset);
    escrow_id
}

/// The `clearledger_pool.wasm` artifact that `register_asset` deploys.
///
/// `register_asset` deploys through `Env::deployer()` and then invokes
/// `initialize` on the fresh instance. The SDK 21 test host can only dispatch
/// that call when the instance was created from real Wasm: an instance created
/// from a placeholder Wasm has no exports, so the `initialize` call traps with
/// `Error(Context, InvalidAction)`. The deploy-and-initialize tests therefore
/// upload the actual pool artifact rather than a stub.
///
/// It is resolved on first use instead of being committed to the repository
/// (a ~50 KiB binary that would also go stale whenever the pool's ABI
/// changes). Resolution order:
///
/// 1. reuse the workspace release artifact when it is already there, which is
///    what `cargo build --workspace --release --target wasm32v1-none`
///    (and therefore CI) produces, otherwise
/// 2. build it into `target/pool-factory-test-fixtures`, a dedicated target
///    directory so the slow (`lto = true`) Wasm build is cached across
///    `cargo test` runs. It cannot share the outer `target/`, because the outer
///    cargo holds an exclusive lock on that directory while this test runs.
fn pool_wasm() -> &'static [u8] {
    static WASM: OnceLock<std::vec::Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        const WASM_FILE: &str = "clearledger_pool.wasm";

        let manifest_dir =
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
        let workspace_dir = manifest_dir
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");

        let release_artifact = workspace_dir
            .join("target/wasm32v1-none/release")
            .join(WASM_FILE);
        let path = if release_artifact.exists() {
            release_artifact
        } else {
            let fixture_dir = workspace_dir.join("target/pool-factory-test-fixtures");
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let status = Command::new(cargo)
                .current_dir(workspace_dir)
                .args([
                    "build",
                    "--package",
                    "clearledger-pool",
                    "--release",
                    "--target",
                    "wasm32v1-none",
                    "--target-dir",
                ])
                .arg(&fixture_dir)
                // The fixture is a plain Wasm build and must not inherit the
                // outer build's rustflags: under `cargo tarpaulin` (the
                // `coverage` CI job) they carry `-C instrument-coverage` plus
                // an `--extern profiler_builtins` that only exists for the host
                // target, which fails the Wasm build outright.
                .env_remove("RUSTFLAGS")
                .env_remove("CARGO_ENCODED_RUSTFLAGS")
                .env_remove("RUSTDOCFLAGS")
                .status()
                .expect("failed to run cargo to build the clearledger-pool Wasm");
            assert!(
                status.success(),
                "building the clearledger-pool Wasm failed ({status}); the \
                 wasm32v1-none target is required, install it with \
                 `rustup target add wasm32v1-none`"
            );
            fixture_dir.join("wasm32v1-none/release").join(WASM_FILE)
        };

        std::fs::read(&path).unwrap_or_else(|e| panic!("reading pool Wasm {}: {e}", path.display()))
    })
}

#[test]
fn test_initialize() {
    let env = Env::default();
    env.mock_all_auths();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let factory_client = PoolFactoryContractClient::new(&env, &factory_id);

    let admin = Address::generate(&env);
    factory_client.initialize(&admin);

    let events = env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, factory_id);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&env, "contract_initialized")
    );
    assert_eq!(
        Address::try_from_val(&env, &topics.get(1).unwrap()).unwrap(),
        admin
    );
    let _: () = TryFromVal::try_from_val(&env, &data).unwrap();

    // Verify admin is stored
    env.as_contract(&factory_id, || {
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        assert_eq!(stored_admin, admin);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn test_initialize_twice_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let factory_client = PoolFactoryContractClient::new(&env, &factory_id);

    let admin = Address::generate(&env);
    factory_client.initialize(&admin);
    factory_client.initialize(&admin);
}

#[test]
fn test_register_asset_deploys_and_initializes_pool() {
    let te = setup();

    let pool_address = register_asset(&te, &te.asset.clone());

    // The factory recorded the mapping and the enumeration index.
    assert_eq!(
        te.factory.get_pool_for_asset(&te.asset),
        Some(pool_address.clone())
    );
    let assets = te.factory.list_assets();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets.get(0), Some(te.asset.clone()));

    // The deployed instance is a live pool contract, initialized for this asset
    // and wired to the invoice/escrow the caller passed in.
    let pool = PoolContractClient::new(&te.env, &pool_address);
    assert_eq!(pool.get_usdc_asset(), te.asset);
    assert_eq!(pool.get_invoice_contract(), te.invoice_id);
    assert_eq!(pool.get_escrow_contract(), te.escrow_id);
    assert_eq!(pool.get_admin(), te.admin);
    assert_eq!(pool.get_treasury(), te.admin);

    // The registry the new pool verifies issuer/buyer profiles against is the
    // one the invoice contract itself uses, read back off the pool's storage
    // because the pool exposes no public registry getter.
    let registry: Address = te
        .env
        .as_contract(&pool_address, || {
            te.env
                .storage()
                .instance()
                .get(&clearledger_pool::DataKey::RegistryContract)
        })
        .expect("pool instance should record a registry contract");
    assert_eq!(registry, te.registry_id);
}

#[test]
fn test_register_asset_emits_pool_instance_created_event() {
    let te = setup();

    let pool_address = register_asset(&te, &te.asset.clone());

    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.factory_id);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "pool_instance_created")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.asset
    );
    assert_eq!(Address::try_from_val(&te.env, &data).unwrap(), pool_address);
}

#[test]
fn test_register_asset_pool_address_is_deterministic() {
    let te = setup();

    // The instance address is fixed by (deploying contract, salt), and the salt
    // is derived from the asset alone, so a caller can predict the pool address
    // for an asset before building the transaction.
    let salt = PoolFactoryContract::asset_salt(&te.env, &te.asset);
    let predicted = te
        .env
        .deployer()
        .with_address(te.factory_id.clone(), salt)
        .deployed_address();

    let pool_address = register_asset(&te, &te.asset.clone());

    assert_eq!(pool_address, predicted);
}

#[test]
fn test_register_asset_gives_each_asset_its_own_pool() {
    let te = setup();
    let other_asset = Address::generate(&te.env);

    let pool = register_asset(&te, &te.asset.clone());
    let other_pool = register_asset(&te, &other_asset);

    // Instances never share state, and each asset resolves to its own pool.
    assert_ne!(pool, other_pool);
    assert_eq!(te.factory.get_pool_for_asset(&te.asset), Some(pool));
    assert_eq!(
        te.factory.get_pool_for_asset(&other_asset),
        Some(other_pool.clone())
    );
    assert_eq!(
        PoolContractClient::new(&te.env, &other_pool).get_usdc_asset(),
        other_asset
    );

    let assets = te.factory.list_assets();
    assert_eq!(assets.len(), 2);
    assert_eq!(assets.get(0), Some(te.asset.clone()));
    assert_eq!(assets.get(1), Some(other_asset));
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_register_asset_without_admin_auth_panics() {
    let te = setup();
    // Drop every mocked authorization: only the factory's stored admin may
    // register an asset and have the factory deploy a pool on its behalf.
    te.env.set_auths(&[]);
    register_asset(&te, &te.asset.clone());
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_register_asset_duplicate_panics() {
    let te = setup();
    register_asset(&te, &te.asset.clone());
    // Re-registering would silently repoint the asset at a second instance and
    // orphan the first one, so the second call is rejected.
    register_asset(&te, &te.asset.clone());
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn test_register_asset_with_uninitialized_invoice_panics() {
    let te = setup();
    // An invoice contract that was never initialized reports no registry, so
    // the pool cannot be wired and registration is rejected rather than
    // deploying a pool pointed at nothing.
    let uninitialized_invoice = te
        .env
        .register_contract(None, clearledger_invoice::InvoiceContract);

    te.factory.register_asset(
        &te.asset,
        &te.pool_wasm_hash,
        &uninitialized_invoice,
        &te.escrow_id,
    );
}

#[test]
fn test_register_existing_pool() {
    let te = setup();
    let pool_address = new_initialized_pool(&te, &te.asset);

    te.factory.register_existing_pool(&te.asset, &pool_address);

    let events = te.env.events().all();
    let (contract, topics, data) = events.get(events.len() - 1).unwrap();
    assert_eq!(contract, te.factory_id);
    assert_eq!(topics.len(), 2);
    assert_eq!(
        Symbol::try_from_val(&te.env, &topics.get(0).unwrap()).unwrap(),
        Symbol::new(&te.env, "existing_pool_registered")
    );
    assert_eq!(
        Address::try_from_val(&te.env, &topics.get(1).unwrap()).unwrap(),
        te.asset
    );
    assert_eq!(Address::try_from_val(&te.env, &data).unwrap(), pool_address);

    // Verify registration
    te.env.as_contract(&te.factory_id, || {
        let stored_pool: Address = te
            .env
            .storage()
            .instance()
            .get(&DataKey::PoolForAsset(te.asset.clone()))
            .unwrap();
        assert_eq!(stored_pool, pool_address);

        let count: u32 = te
            .env
            .storage()
            .instance()
            .get(&DataKey::AssetCount)
            .unwrap();
        assert_eq!(count, 1);

        let indexed_asset: Address = te
            .env
            .storage()
            .instance()
            .get(&DataKey::AssetIndex(0))
            .unwrap();
        assert_eq!(indexed_asset, te.asset);
    });
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_register_existing_pool_rejects_account_address() {
    let te = setup();
    te.factory
        .register_existing_pool(&te.asset, &Address::generate(&te.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_register_existing_pool_rejects_non_pool_contract() {
    let te = setup();
    let token = te.env.register_contract(None, MockToken);
    te.factory.register_existing_pool(&te.asset, &token);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn test_register_existing_pool_rejects_wrong_asset_pool() {
    let te = setup();
    let other_asset = Address::generate(&te.env);
    let pool_address = new_initialized_pool(&te, &other_asset);
    te.factory.register_existing_pool(&te.asset, &pool_address);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_register_existing_pool_duplicate_panics() {
    let te = setup();
    let pool_address = new_initialized_pool(&te, &te.asset);

    te.factory.register_existing_pool(&te.asset, &pool_address);
    te.factory.register_existing_pool(&te.asset, &pool_address);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")]
fn test_register_existing_pool_after_register_asset_panics() {
    let te = setup();
    // The two registration paths share one mapping, so an asset deployed by
    // `register_asset` cannot be silently repointed at another pool.
    register_asset(&te, &te.asset.clone());
    te.factory
        .register_existing_pool(&te.asset, &Address::generate(&te.env));
}

#[test]
fn test_get_pool_for_asset_returns_deployed_pool() {
    let te = setup();
    let pool_address = register_asset(&te, &te.asset.clone());

    assert_eq!(te.factory.get_pool_for_asset(&te.asset), Some(pool_address));
}

#[test]
fn test_get_pool_for_asset_returns_migrated_pool() {
    let te = setup();
    let existing_pool = new_initialized_pool(&te, &te.asset);
    te.factory.register_existing_pool(&te.asset, &existing_pool);

    // `register_existing_pool` writes the same key, so an asset adopted during
    // the migration resolves through the same lookup.
    assert_eq!(
        te.factory.get_pool_for_asset(&te.asset),
        Some(existing_pool)
    );
}

#[test]
fn test_get_pool_for_asset_unregistered_returns_none() {
    let te = setup();
    assert_eq!(te.factory.get_pool_for_asset(&te.asset), None);
    assert_eq!(
        te.factory.get_pool_for_asset(&Address::generate(&te.env)),
        None
    );
}

#[test]
fn test_get_pool_for_asset_requires_no_auth() {
    let te = setup();
    let pool_address = register_asset(&te, &te.asset.clone());
    te.env.set_auths(&[]);

    // Read-only view: no authorization is required, even with every mocked
    // authorization dropped.
    assert_eq!(te.factory.get_pool_for_asset(&te.asset), Some(pool_address));
}

#[test]
fn test_list_assets_empty() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let factory_client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = factory_client.list_assets();
    assert_eq!(assets.len(), 0);
}

#[test]
fn test_gas_benchmark_register_existing_pool() {
    extern crate std;
    let te = setup();
    let pool_address = new_initialized_pool(&te, &te.asset);

    // Measure register_existing_pool resource cost
    te.env.budget().reset_default();
    let cpu_before = te.env.budget().cpu_instruction_cost();
    let mem_before = te.env.budget().memory_bytes_cost();

    te.factory.register_existing_pool(&te.asset, &pool_address);

    let cpu_after = te.env.budget().cpu_instruction_cost();
    let mem_after = te.env.budget().memory_bytes_cost();

    let cpu_delta = cpu_after - cpu_before;
    let mem_delta = mem_after - mem_before;

    // Log for manual inspection
    std::eprintln!("register_existing_pool CPU instructions: {}", cpu_delta);
    std::eprintln!("register_existing_pool Memory bytes: {}", mem_delta);
}

#[test]
fn test_list_assets_single() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let factory_client = PoolFactoryContractClient::new(&env, &factory_id);

    // Simulate asset registration by setting the index directly
    let asset = Address::generate(&env);
    env.as_contract(&factory_id, || {
        env.storage().instance().set(&DataKey::AssetCount, &1u32);
        env.storage()
            .instance()
            .set(&DataKey::AssetIndex(0), &asset);
    });

    let assets = factory_client.list_assets();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets.get(0), Some(asset));
}

#[test]
fn test_list_assets_multiple() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let factory_client = PoolFactoryContractClient::new(&env, &factory_id);

    // Simulate multiple asset registrations
    let asset1 = Address::generate(&env);
    let asset2 = Address::generate(&env);
    let asset3 = Address::generate(&env);

    env.as_contract(&factory_id, || {
        env.storage().instance().set(&DataKey::AssetCount, &3u32);
        env.storage()
            .instance()
            .set(&DataKey::AssetIndex(0), &asset1);
        env.storage()
            .instance()
            .set(&DataKey::AssetIndex(1), &asset2);
        env.storage()
            .instance()
            .set(&DataKey::AssetIndex(2), &asset3);
    });

    let assets = factory_client.list_assets();
    assert_eq!(assets.len(), 3);
    assert_eq!(assets.get(0), Some(asset1));
    assert_eq!(assets.get(1), Some(asset2));
    assert_eq!(assets.get(2), Some(asset3));
}

// ---------------- Aggregate stats pagination ----------------

/// Seeds the asset index and pool mapping exactly as repeated registrations
/// would, without paying for a pool deploy/initialize per order-sensitive
/// pagination test. Each asset gets its own mock stats pool so the returned
/// pair order is observable.
fn seed_aggregate_assets(
    env: &Env,
    factory_id: &Address,
    assets: &[Address],
) -> Vec<Address> {
    // A max-size page makes up to `MAX_AGGREGATE_STATS_PAGE_LIMIT` real
    // cross-contract calls, which can cross the default test-host budget.
    env.budget().reset_unlimited();
    let mut pools = Vec::new(env);
    for _ in assets {
        pools.push_back(env.register_contract(None, MockStatsPool));
    }
    env.as_contract(factory_id, || {
        env.storage()
            .instance()
            .set(&DataKey::AssetCount, &(assets.len() as u32));
        for (index, asset) in assets.iter().enumerate() {
            let pool = pools.get(index as u32).unwrap();
            env.storage()
                .instance()
                .set(&DataKey::AssetIndex(index as u32), asset);
            env.storage()
                .instance()
                .set(&DataKey::PoolForAsset(asset.clone()), &pool);
        }
    });
    pools
}

/// Appends one asset and its pool mapping the way `record_pool` does, returning
/// the new mock pool address.
fn append_aggregate_asset(env: &Env, factory_id: &Address, asset: &Address) -> Address {
    let pool = env.register_contract(None, MockStatsPool);
    env.as_contract(factory_id, || {
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
            .set(&DataKey::PoolForAsset(asset.clone()), &pool);
    });
    pool
}

/// Walks every aggregate-stats page from cursor 0 and concatenates the items.
fn walk_aggregate_stats(
    env: &Env,
    client: &PoolFactoryContractClient<'_>,
    limit: u32,
) -> Vec<(Address, PoolStats)> {
    let mut cursor = 0u32;
    let mut items = Vec::new(env);
    loop {
        let page = client.get_aggregate_stats_page(&cursor, &limit);
        let mut index = 0;
        while index < page.items.len() {
            items.push_back(page.items.get(index).unwrap());
            index += 1;
        }
        match page.next_cursor {
            Some(next) => cursor = next,
            None => break,
        }
    }
    items
}

#[test]
fn test_get_aggregate_stats_page_empty() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let page = client.get_aggregate_stats_page(&0, &10);
    assert_eq!(page.items.len(), 0);
    assert!(page.next_cursor.is_none());
}

#[test]
fn test_get_aggregate_stats_page_first_page() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let pools = seed_aggregate_assets(&env, &factory_id, &assets);

    let page = client.get_aggregate_stats_page(&0, &2);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items.get(0).unwrap().0, pools.get(0).unwrap());
    assert_eq!(page.items.get(1).unwrap().0, pools.get(1).unwrap());
    assert_eq!(page.next_cursor, Some(2));
}

#[test]
fn test_get_aggregate_stats_page_multi_page_walk_has_no_gaps_or_duplicates() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let pools = seed_aggregate_assets(&env, &factory_id, &assets);

    let walked = walk_aggregate_stats(&env, &client, 2);

    assert_eq!(walked.len(), 5);
    for (index, pool) in pools.iter().enumerate() {
        assert_eq!(walked.get(index as u32).unwrap().0, pool);
    }
}

#[test]
fn test_get_aggregate_stats_page_exact_and_max_limit() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    // An exact multiple of the page size ends on a full page whose
    // `next_cursor` is already `None` rather than an extra empty page.
    let assets = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    seed_aggregate_assets(&env, &factory_id, &assets);

    let first = client.get_aggregate_stats_page(&0, &2);
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.next_cursor, Some(2));

    let last = client.get_aggregate_stats_page(&2, &2);
    assert_eq!(last.items.len(), 2);
    assert!(last.next_cursor.is_none());

    // A full max-size request is accepted and bounded by the constant.
    let max_assets: std::vec::Vec<Address> = (0..MAX_AGGREGATE_STATS_PAGE_LIMIT)
        .map(|_| Address::generate(&env))
        .collect();
    seed_aggregate_assets(&env, &factory_id, &max_assets);

    let page = client.get_aggregate_stats_page(&0, &MAX_AGGREGATE_STATS_PAGE_LIMIT);
    assert_eq!(page.items.len(), MAX_AGGREGATE_STATS_PAGE_LIMIT);
    assert!(page.next_cursor.is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn test_get_aggregate_stats_page_zero_limit_panics() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    client.get_aggregate_stats_page(&0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")]
fn test_get_aggregate_stats_page_limit_above_max_panics() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    client.get_aggregate_stats_page(&0, &(MAX_AGGREGATE_STATS_PAGE_LIMIT + 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")]
fn test_get_aggregate_stats_page_cursor_past_end_panics() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = [Address::generate(&env), Address::generate(&env)];
    seed_aggregate_assets(&env, &factory_id, &assets);

    client.get_aggregate_stats_page(&3, &2);
}

#[test]
fn test_get_aggregate_stats_page_cursor_at_end_is_empty() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = [Address::generate(&env), Address::generate(&env)];
    seed_aggregate_assets(&env, &factory_id, &assets);

    // `cursor == count` is a valid, empty final page rather than an error.
    let page = client.get_aggregate_stats_page(&2, &2);
    assert_eq!(page.items.len(), 0);
    assert!(page.next_cursor.is_none());
}

#[test]
fn test_get_aggregate_stats_page_append_between_pages() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let pools = seed_aggregate_assets(&env, &factory_id, &assets);

    let first = client.get_aggregate_stats_page(&0, &2);
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.items.get(0).unwrap().0, pools.get(0).unwrap());
    assert_eq!(first.items.get(1).unwrap().0, pools.get(1).unwrap());
    assert_eq!(first.next_cursor, Some(2));

    // Registration between reads appends at the end: the already-read items
    // keep their cursors and the new asset appears on the next page.
    let appended_asset = Address::generate(&env);
    let appended_pool = append_aggregate_asset(&env, &factory_id, &appended_asset);

    let second = client.get_aggregate_stats_page(&2, &2);
    assert_eq!(second.items.len(), 2);
    assert_eq!(second.items.get(0).unwrap().0, pools.get(2).unwrap());
    assert_eq!(second.items.get(1).unwrap().0, appended_pool);
    assert!(second.next_cursor.is_none());
}

#[test]
fn test_get_aggregate_stats_page_cursor_advances_past_skipped_pools() {
    let env = Env::default();
    let factory_id = env.register_contract(None, PoolFactoryContract);
    let client = PoolFactoryContractClient::new(&env, &factory_id);

    let assets = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let pools = seed_aggregate_assets(&env, &factory_id, &assets);

    // Model a corrupt/legacy mapping for the middle asset: its pool call
    // fails, so it is skipped without stalling the walk.
    let broken_asset = assets[1].clone();
    let broken_pool = Address::generate(&env);
    env.as_contract(&factory_id, || {
        env.storage()
            .instance()
            .set(&DataKey::PoolForAsset(broken_asset), &broken_pool);
    });

    let page = client.get_aggregate_stats_page(&0, &3);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items.get(0).unwrap().0, pools.get(0).unwrap());
    assert_eq!(page.items.get(1).unwrap().0, pools.get(2).unwrap());
    assert!(page.next_cursor.is_none());
}

// --------------- Mock Token for Integration Tests ---------------

use soroban_sdk::{contract, contractimpl, contracttype};

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        let from_key = TKey(from.clone());
        let to_key = TKey(to.clone());
        let from_bal: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        let to_bal: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_bal - amount));
        env.storage().persistent().set(&to_key, &(to_bal + amount));
    }

    pub fn balance(env: Env, addr: Address) -> i128 {
        env.storage().persistent().get(&TKey(addr)).unwrap_or(0)
    }
}

/// Minimal pool stand-in whose only job is to answer the factory's aggregate
/// `get_stats` cross-contract call, so pagination tests can observe item order
/// without deploying and initializing a real pool per asset.
#[contract]
pub struct MockStatsPool;

#[contractimpl]
impl MockStatsPool {
    pub fn get_stats(_env: Env) -> PoolStats {
        PoolStats {
            total_deposits: 1,
            total_funded: 2,
            available_liquidity: 3,
            utilization_rate_bps: 4,
            total_yield_distributed: 5,
            total_loss_realised: 6,
            active_invoice_count: 7,
            total_shares: 8,
            max_utilization_bps: 9,
        }
    }
}

#[contracttype]
pub struct TKey(Address);

#[contract]
pub struct MockAgentRegistry;

#[contractimpl]
impl MockAgentRegistry {
    pub fn get_agent(env: Env, agent_id: Symbol) -> Option<clearledger_invoice::Agent> {
        env.storage().persistent().get(&AgentKey(agent_id))
    }

    pub fn register_agent(env: Env, agent_id: Symbol, agent: clearledger_invoice::Agent) {
        env.storage().persistent().set(&AgentKey(agent_id), &agent);
    }
}

#[contracttype]
pub struct AgentKey(Symbol);

const TEST_AGENT_SEED: [u8; 32] = [7; 32];

fn test_agent_key() -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_slice(&TEST_AGENT_SEED).unwrap()
}

fn attest_invoice(env: &Env, invoice: &InvoiceContractClient, invoice_id: &BytesN<32>) {
    let public_key = test_agent_key().verifying_key().to_encoded_point(false);
    let mut public_key_bytes = [0u8; 65];
    public_key_bytes.copy_from_slice(public_key.as_bytes());
    let agent_id = Symbol::new(env, "test_agent");
    let agent_registry_id = env.register_contract(None, MockAgentRegistry);
    let agent_registry = MockAgentRegistryClient::new(env, &agent_registry_id);
    agent_registry.register_agent(
        &agent_id,
        &clearledger_invoice::Agent {
            active: true,
            pubkey: BytesN::from_array(env, &public_key_bytes),
        },
    );
    invoice.set_agent_registry_contract(&agent_registry_id);

    let payload = clearledger_invoice::AttestationPayload {
        domain_separator: BytesN::from_array(
            env,
            &clearledger_invoice::ATTESTATION_DOMAIN_SEPARATOR,
        ),
        invoice_id: invoice_id.clone(),
        risk_score: 5000,
        evidence_hash: BytesN::from_array(env, &[9; 32]),
        agent_id,
        nonce: 1,
    };
    let payload_bytes = payload.to_xdr(env);
    let digest = env.crypto().keccak256(&payload_bytes).to_array();
    let (signature, recovery_id) = test_agent_key().sign_prehash_recoverable(&digest).unwrap();
    let mut signature_bytes = [0u8; 65];
    signature_bytes[..64].copy_from_slice(&signature.to_bytes());
    signature_bytes[64] = recovery_id.to_byte();
    invoice.submit_attestation(
        invoice_id,
        &payload_bytes,
        &BytesN::from_array(env, &signature_bytes),
    );
}

fn create_listed_invoice(
    te: &TestEnv,
    issuer: &Address,
    buyer: &Address,
    asset: &Address,
) -> BytesN<32> {
    let invoice = InvoiceContractClient::new(&te.env, &te.invoice_id);
    let due_date = te.env.ledger().timestamp() + 86_400;
    let invoice_id = invoice.create(issuer, buyer, &1_000_000_000, &due_date, asset);
    attest_invoice(&te.env, &invoice, &invoice_id);
    invoice.list_for_financing(&invoice_id, &200);
    invoice_id
}

fn assert_pool_stats_equal(actual: crate::types::PoolStats, expected: clearledger_pool::PoolStats) {
    assert_eq!(actual.total_deposits, expected.total_deposits);
    assert_eq!(actual.total_funded, expected.total_funded);
    assert_eq!(actual.available_liquidity, expected.available_liquidity);
    assert_eq!(actual.utilization_rate_bps, expected.utilization_rate_bps);
    assert_eq!(
        actual.total_yield_distributed,
        expected.total_yield_distributed
    );
    assert_eq!(actual.total_loss_realised, expected.total_loss_realised);
    assert_eq!(actual.active_invoice_count, expected.active_invoice_count);
    assert_eq!(actual.total_shares, expected.total_shares);
    assert_eq!(actual.max_utilization_bps, expected.max_utilization_bps);
}

// --------------- New Tests ---------------

#[test]
fn test_factory_pools_fund_and_repay_invoices_for_their_own_assets() {
    let te = setup();
    // This integration path deploys two Wasm pools and invokes real invoice,
    // escrow, and registry contracts; keep its host budget independent of the
    // default unit-test resource ceiling.
    te.env.budget().reset_unlimited();
    let invoice = InvoiceContractClient::new(&te.env, &te.invoice_id);
    let asset_a = te.env.register_contract(None, MockToken);
    let asset_b = te.env.register_contract(None, MockToken);
    invoice.add_supported_asset(&asset_a);
    invoice.add_supported_asset(&asset_b);

    let pool_a_address = register_asset(&te, &asset_a);
    let pool_b_address = register_asset(&te, &asset_b);
    assert_eq!(
        te.factory.get_pool_for_asset(&asset_a),
        Some(pool_a_address.clone())
    );
    assert_eq!(
        te.factory.get_pool_for_asset(&asset_b),
        Some(pool_b_address.clone())
    );
    let pool_a = PoolContractClient::new(&te.env, &pool_a_address);
    let pool_b = PoolContractClient::new(&te.env, &pool_b_address);

    let issuer = Address::generate(&te.env);
    let buyer = Address::generate(&te.env);
    let lp_a = Address::generate(&te.env);
    let lp_b = Address::generate(&te.env);
    let registry = RegistryContractClient::new(&te.env, &te.registry_id);
    let metadata: Map<String, String> = Map::new(&te.env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);

    for (asset, lp) in [(&asset_a, &lp_a), (&asset_b, &lp_b)] {
        let lp_key = TKey(lp.clone());
        let buyer_key = TKey(buyer.clone());
        te.env.as_contract(asset, || {
            te.env
                .storage()
                .persistent()
                .set(&lp_key, &100_000_000_000i128);
            te.env
                .storage()
                .persistent()
                .set(&buyer_key, &100_000_000_000i128);
        });
    }
    pool_a.deposit(&lp_a, &2_000_000_000);
    pool_b.deposit(&lp_b, &2_000_000_000);

    let invoice_a = create_listed_invoice(&te, &issuer, &buyer, &asset_a);
    let invoice_b = create_listed_invoice(&te, &issuer, &buyer, &asset_b);

    // Each real pool rejects a listed invoice denominated in the other asset.
    let wrong_a = pool_a.try_fund_invoice(&invoice_b);
    let wrong_b = pool_b.try_fund_invoice(&invoice_a);
    assert!(
        std::format!("{wrong_a:?}").contains("#11"),
        "expected AssetMismatch, got {wrong_a:?}"
    );
    assert!(
        std::format!("{wrong_b:?}").contains("#11"),
        "expected AssetMismatch, got {wrong_b:?}"
    );
    assert_eq!(invoice.get_status(&invoice_a), 1);
    assert_eq!(invoice.get_status(&invoice_b), 1);
    assert_eq!(pool_a.get_stats().total_funded, 0);
    assert_eq!(pool_b.get_stats().total_funded, 0);

    // Invoice authorizes one funding pool at a time; each invoice records the
    // factory-resolved pool that funds its own asset.
    invoice.set_pool_contract(&pool_a_address);
    assert!(pool_a.fund_invoice(&invoice_a));
    invoice.set_pool_contract(&pool_b_address);
    assert!(pool_b.fund_invoice(&invoice_b));
    assert_eq!(invoice.get_status(&invoice_a), 2);
    assert_eq!(invoice.get_status(&invoice_b), 2);

    let funded_amount = 1_000_000_000u128 * 9_800 / 10_000;
    let stats_a_funded = pool_a.get_stats();
    let stats_b_funded = pool_b.get_stats();
    assert_eq!(stats_a_funded.total_funded, funded_amount);
    assert_eq!(stats_b_funded.total_funded, funded_amount);
    assert_eq!(stats_a_funded.active_invoice_count, 1);
    assert_eq!(stats_b_funded.active_invoice_count, 1);

    invoice.mark_shipped(&invoice_a);
    invoice.confirm_delivery(&invoice_a, &issuer);
    invoice.confirm_delivery(&invoice_a, &buyer);
    invoice.mark_shipped(&invoice_b);
    invoice.confirm_delivery(&invoice_b, &issuer);
    invoice.confirm_delivery(&invoice_b, &buyer);
    te.env
        .ledger()
        .set_timestamp(te.env.ledger().timestamp() + 43_200);
    invoice.set_escrow_contract(&pool_a.get_escrow_contract());
    assert!(invoice.repay_early(&invoice_a));
    invoice.set_escrow_contract(&pool_b.get_escrow_contract());
    assert!(invoice.repay_early(&invoice_b));
    assert_eq!(invoice.get_status(&invoice_a), 5);
    assert_eq!(invoice.get_status(&invoice_b), 5);

    let stats_a = pool_a.get_stats();
    let stats_b = pool_b.get_stats();
    assert_eq!(stats_a.total_funded, 0);
    assert_eq!(stats_b.total_funded, 0);
    assert_eq!(stats_a.active_invoice_count, 0);
    assert_eq!(stats_b.active_invoice_count, 0);
    assert!(stats_a.total_yield_distributed > 0);
    assert!(stats_b.total_yield_distributed > 0);

    let aggregate = te.factory.get_aggregate_stats();
    assert_eq!(aggregate.len(), 2);
    let (aggregate_pool_a, aggregate_stats_a) = aggregate.get(0).unwrap();
    let (aggregate_pool_b, aggregate_stats_b) = aggregate.get(1).unwrap();
    assert_eq!(aggregate_pool_a, pool_a_address);
    assert_eq!(aggregate_pool_b, pool_b_address);
    assert_pool_stats_equal(aggregate_stats_a.clone(), stats_a.clone());
    assert_pool_stats_equal(aggregate_stats_b.clone(), stats_b.clone());
    assert_eq!(
        aggregate_stats_a.total_deposits + aggregate_stats_b.total_deposits,
        stats_a.total_deposits + stats_b.total_deposits
    );
    assert_eq!(
        aggregate_stats_a.total_yield_distributed + aggregate_stats_b.total_yield_distributed,
        stats_a.total_yield_distributed + stats_b.total_yield_distributed
    );
    assert_eq!(
        aggregate_stats_a.total_loss_realised + aggregate_stats_b.total_loss_realised,
        stats_a.total_loss_realised + stats_b.total_loss_realised
    );
}

#[test]
fn test_pool_instances_do_not_cross_contaminate() {
    let te = setup();

    // Register two actual mock tokens
    let asset1 = te.env.register_contract(None, MockToken);
    let asset2 = te.env.register_contract(None, MockToken);

    let pool1 = register_asset(&te, &asset1);
    let pool2 = register_asset(&te, &asset2);

    let lp1 = Address::generate(&te.env);
    let lp2 = Address::generate(&te.env);

    // Give LP1 and LP2 some mock tokens
    let lp1_bal_key = TKey(lp1.clone());
    let lp2_bal_key = TKey(lp2.clone());
    te.env.as_contract(&asset1, || {
        te.env
            .storage()
            .persistent()
            .set(&lp1_bal_key, &1_000_000_000_i128);
    });
    te.env.as_contract(&asset2, || {
        te.env
            .storage()
            .persistent()
            .set(&lp2_bal_key, &2_000_000_000_i128);
    });

    let pool1_client = PoolContractClient::new(&te.env, &pool1);
    let pool2_client = PoolContractClient::new(&te.env, &pool2);

    // LP1 deposits 20_000_000 into pool1
    pool1_client.deposit(&lp1, &20_000_000);

    // LP2 deposits 30_000_000 into pool2
    pool2_client.deposit(&lp2, &30_000_000);

    let stats1 = pool1_client.get_stats();
    let stats2 = pool2_client.get_stats();

    // Verify no cross contamination
    assert_eq!(stats1.total_deposits, 20_000_000);
    assert_eq!(stats1.total_shares, 20_000_000);

    assert_eq!(stats2.total_deposits, 30_000_000);
    assert_eq!(stats2.total_shares, 30_000_000);
}

/// Proves that two factory-deployed pools for different assets keep deposits,
/// shares and token custody fully separate, including across a withdrawal.
#[test]
fn test_two_asset_pools_isolate_deposits_shares_and_balances() {
    let te = setup();
    let asset_a = te.env.register_contract(None, MockToken);
    let asset_b = te.env.register_contract(None, MockToken);

    let pool_a_address = register_asset(&te, &asset_a);
    let pool_b_address = register_asset(&te, &asset_b);

    // Factory lookups are asset-scoped: each asset resolves to exactly its own
    // instance, and an address with no registration resolves to nothing.
    assert_eq!(
        te.factory.get_pool_for_asset(&asset_a),
        Some(pool_a_address.clone())
    );
    assert_eq!(
        te.factory.get_pool_for_asset(&asset_b),
        Some(pool_b_address.clone())
    );
    assert_ne!(pool_a_address, pool_b_address);
    assert_eq!(
        te.factory.get_pool_for_asset(&Address::generate(&te.env)),
        None
    );
    let listed = te.factory.list_assets();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed.get(0), Some(asset_a.clone()));
    assert_eq!(listed.get(1), Some(asset_b.clone()));

    let pool_a = PoolContractClient::new(&te.env, &pool_a_address);
    let pool_b = PoolContractClient::new(&te.env, &pool_b_address);
    assert_eq!(pool_a.get_funding_asset(), asset_a);
    assert_eq!(pool_b.get_funding_asset(), asset_b);

    let lp_a = Address::generate(&te.env);
    let lp_b = Address::generate(&te.env);
    let token_a = MockTokenClient::new(&te.env, &asset_a);
    let token_b = MockTokenClient::new(&te.env, &asset_b);

    // Seed each LP with only the token of the pool it deposits into. If either
    // pool reached across assets, the deposit would fail against the token it
    // does not hold.
    te.env.as_contract(&asset_a, || {
        te.env
            .storage()
            .persistent()
            .set(&TKey(lp_a.clone()), &100_000_000_000i128);
    });
    te.env.as_contract(&asset_b, || {
        te.env
            .storage()
            .persistent()
            .set(&TKey(lp_b.clone()), &100_000_000_000i128);
    });

    pool_a.deposit(&lp_a, &2_000_000_000);
    pool_b.deposit(&lp_b, &4_000_000_000);

    let stats_a = pool_a.get_stats();
    let stats_b = pool_b.get_stats();
    assert_eq!(stats_a.total_deposits, 2_000_000_000);
    assert_eq!(stats_a.total_shares, 2_000_000_000);
    assert_eq!(stats_b.total_deposits, 4_000_000_000);
    assert_eq!(stats_b.total_shares, 4_000_000_000);
    assert_eq!(stats_a.total_funded, 0);
    assert_eq!(stats_b.total_funded, 0);
    assert_eq!(stats_a.utilization_rate_bps, 0);
    assert_eq!(stats_b.utilization_rate_bps, 0);

    // Token custody is per-instance as well.
    assert_eq!(token_a.balance(&pool_a_address), 2_000_000_000);
    assert_eq!(token_a.balance(&pool_b_address), 0);
    assert_eq!(token_b.balance(&pool_b_address), 4_000_000_000);
    assert_eq!(token_b.balance(&pool_a_address), 0);

    // Withdrawing from pool A must not burn shares or release tokens from B.
    pool_a.withdraw(&lp_a, &1_000_000_000);
    let stats_a_after = pool_a.get_stats();
    let stats_b_after = pool_b.get_stats();
    assert_eq!(stats_a_after.total_deposits, 1_000_000_000);
    assert_eq!(stats_a_after.total_shares, 1_000_000_000);
    assert_eq!(stats_b_after.total_deposits, 4_000_000_000);
    assert_eq!(stats_b_after.total_shares, 4_000_000_000);
    assert_eq!(token_a.balance(&pool_a_address), 1_000_000_000);
    assert_eq!(token_b.balance(&pool_b_address), 4_000_000_000);
}

/// Proves that invoice eligibility, funded totals and utilization are scoped to
/// the asset a pool was deployed for: an invoice in asset A cannot be funded by
/// pool B, and funding one pool leaves the other's counters untouched.
#[test]
fn test_two_asset_pools_isolate_funding_utilization_and_invoice_eligibility() {
    let te = setup();
    te.env.budget().reset_unlimited();

    let asset_a = te.env.register_contract(None, MockToken);
    let asset_b = te.env.register_contract(None, MockToken);
    let invoice = InvoiceContractClient::new(&te.env, &te.invoice_id);
    invoice.add_supported_asset(&asset_a);
    invoice.add_supported_asset(&asset_b);

    let pool_a_address = register_asset(&te, &asset_a);
    let pool_b_address = register_asset(&te, &asset_b);
    let pool_a = PoolContractClient::new(&te.env, &pool_a_address);
    let pool_b = PoolContractClient::new(&te.env, &pool_b_address);

    let issuer = Address::generate(&te.env);
    let buyer = Address::generate(&te.env);
    let lp_a = Address::generate(&te.env);
    let lp_b = Address::generate(&te.env);
    let registry = RegistryContractClient::new(&te.env, &te.registry_id);
    let metadata: Map<String, String> = Map::new(&te.env);
    registry.register_issuer(&issuer, &metadata);
    registry.register_buyer(&buyer, &metadata);
    registry.verify_profile(&issuer, &true);
    registry.verify_profile(&buyer, &true);

    for asset in [&asset_a, &asset_b] {
        te.env.as_contract(asset, || {
            te.env
                .storage()
                .persistent()
                .set(&TKey(lp_a.clone()), &100_000_000_000i128);
            te.env
                .storage()
                .persistent()
                .set(&TKey(lp_b.clone()), &100_000_000_000i128);
            te.env
                .storage()
                .persistent()
                .set(&TKey(buyer.clone()), &100_000_000_000i128);
        });
    }
    pool_a.deposit(&lp_a, &2_000_000_000);
    pool_b.deposit(&lp_b, &2_000_000_000);

    let invoice_a = create_listed_invoice(&te, &issuer, &buyer, &asset_a);
    let invoice_b = create_listed_invoice(&te, &issuer, &buyer, &asset_b);

    // Cross-asset funding is rejected before any pool accounting moves.
    let wrong_a = pool_a.try_fund_invoice(&invoice_b);
    let wrong_b = pool_b.try_fund_invoice(&invoice_a);
    assert!(
        std::format!("{wrong_a:?}").contains("#11"),
        "expected AssetMismatch, got {wrong_a:?}"
    );
    assert!(
        std::format!("{wrong_b:?}").contains("#11"),
        "expected AssetMismatch, got {wrong_b:?}"
    );
    for stats in [pool_a.get_stats(), pool_b.get_stats()] {
        assert_eq!(stats.total_funded, 0);
        assert_eq!(stats.utilization_rate_bps, 0);
        assert_eq!(stats.active_invoice_count, 0);
        assert_eq!(stats.available_liquidity, 2_000_000_000);
    }
    assert_eq!(invoice.get_status(&invoice_a), 1);
    assert_eq!(invoice.get_status(&invoice_b), 1);

    // Fund A only: A's funded total and utilization move, B's stay zero.
    invoice.set_pool_contract(&pool_a_address);
    assert!(pool_a.fund_invoice(&invoice_a));

    let funded_amount = 1_000_000_000u128 * 9_800 / 10_000;
    let stats_a = pool_a.get_stats();
    let stats_b = pool_b.get_stats();
    assert_eq!(stats_a.total_funded, funded_amount);
    assert_eq!(stats_a.utilization_rate_bps, 4_900);
    assert_eq!(stats_a.active_invoice_count, 1);
    assert_eq!(stats_a.available_liquidity, 2_000_000_000 - funded_amount);
    assert_eq!(stats_b.total_funded, 0);
    assert_eq!(stats_b.utilization_rate_bps, 0);
    assert_eq!(stats_b.active_invoice_count, 0);
    assert_eq!(stats_b.available_liquidity, 2_000_000_000);
    assert_eq!(invoice.get_status(&invoice_a), 2);
    assert_eq!(invoice.get_status(&invoice_b), 1);

    // Pool B can still fund its own asset, and the two ledgers remain
    // independent rather than sharing a global funded counter.
    invoice.set_pool_contract(&pool_b_address);
    assert!(pool_b.fund_invoice(&invoice_b));

    let stats_a = pool_a.get_stats();
    let stats_b = pool_b.get_stats();
    assert_eq!(stats_a.total_funded, funded_amount);
    assert_eq!(stats_b.total_funded, funded_amount);
    assert_eq!(stats_a.utilization_rate_bps, 4_900);
    assert_eq!(stats_b.utilization_rate_bps, 4_900);
    assert_eq!(stats_a.active_invoice_count, 1);
    assert_eq!(stats_b.active_invoice_count, 1);
    assert_eq!(stats_a.available_liquidity, 2_000_000_000 - funded_amount);
    assert_eq!(stats_b.available_liquidity, 2_000_000_000 - funded_amount);

    // The factory's aggregate view reads each instance's own ledger.
    let aggregate = te.factory.get_aggregate_stats();
    assert_eq!(aggregate.len(), 2);
    assert_eq!(aggregate.get(0).unwrap().0, pool_a_address);
    assert_eq!(aggregate.get(1).unwrap().0, pool_b_address);
    assert_pool_stats_equal(aggregate.get(0).unwrap().1, stats_a);
    assert_pool_stats_equal(aggregate.get(1).unwrap().1, stats_b);
}

/// Registration must not record a pool when the escrow it is wired to custodies
/// a different asset: the pool initializer rejects the mismatch before the
/// factory writes its asset -> pool mapping.
#[test]
fn test_register_asset_rejects_escrow_asset_mismatch_without_recording_pool() {
    let te = setup();
    let other_asset = te.env.register_contract(None, MockToken);

    // `te.escrow_id` was initialized for `te.asset`, not `other_asset`.
    let result = te.factory.try_register_asset(
        &other_asset,
        &te.pool_wasm_hash,
        &te.invoice_id,
        &te.escrow_id,
    );
    assert!(
        std::format!("{result:?}").contains("#20"),
        "expected EscrowAssetMismatch, got {result:?}"
    );

    assert_eq!(te.factory.get_pool_for_asset(&other_asset), None);
    assert_eq!(te.factory.get_pool_for_asset(&te.asset), None);
    assert_eq!(te.factory.list_assets().len(), 0);
}

/// A wrong-asset `register_existing_pool` is rejected and leaves the factory's
/// mapping/index untouched, so a failed migration cannot poison an asset lookup.
#[test]
fn test_register_existing_pool_wrong_asset_does_not_record_mapping() {
    let te = setup();
    let other_asset = Address::generate(&te.env);
    let other_pool = new_initialized_pool(&te, &other_asset);

    let result = te.factory.try_register_existing_pool(&te.asset, &other_pool);
    assert!(
        std::format!("{result:?}").contains("#5"),
        "expected InvalidPool, got {result:?}"
    );

    assert_eq!(te.factory.get_pool_for_asset(&te.asset), None);
    assert_eq!(te.factory.get_pool_for_asset(&other_asset), None);
    assert_eq!(te.factory.list_assets().len(), 0);
}

#[test]
fn test_get_aggregate_stats() {
    let te = setup();

    let asset1 = te.env.register_contract(None, MockToken);
    let asset2 = te.env.register_contract(None, MockToken);

    let pool1 = register_asset(&te, &asset1);
    let pool2 = register_asset(&te, &asset2);

    let lp1 = Address::generate(&te.env);
    let lp2 = Address::generate(&te.env);

    let lp1_bal_key = TKey(lp1.clone());
    let lp2_bal_key = TKey(lp2.clone());
    te.env.as_contract(&asset1, || {
        te.env
            .storage()
            .persistent()
            .set(&lp1_bal_key, &1_000_000_000_i128);
    });
    te.env.as_contract(&asset2, || {
        te.env
            .storage()
            .persistent()
            .set(&lp2_bal_key, &2_000_000_000_i128);
    });

    let pool1_client = PoolContractClient::new(&te.env, &pool1);
    let pool2_client = PoolContractClient::new(&te.env, &pool2);

    pool1_client.deposit(&lp1, &20_000_000);
    pool2_client.deposit(&lp2, &30_000_000);

    let aggregate = te.factory.get_aggregate_stats();
    assert_eq!(aggregate.len(), 2);

    // The order depends on listing order, which is insertion order.
    // pool1 was inserted first.
    let (addr1, stat1) = aggregate.get(0).unwrap();
    let (addr2, stat2) = aggregate.get(1).unwrap();

    assert_eq!(addr1, pool1);
    assert_eq!(stat1.total_deposits, 20_000_000);

    assert_eq!(addr2, pool2);
    assert_eq!(stat2.total_deposits, 30_000_000);
}

#[test]
fn test_get_aggregate_stats_skips_pool_when_stats_call_fails() {
    let te = setup();
    let valid_pool = register_asset(&te, &te.asset);
    let invalid_asset = Address::generate(&te.env);
    let invalid_pool = Address::generate(&te.env);

    // Simulate a legacy/corrupt mapping: valid registration now rejects this
    // address, but aggregate reads must remain resilient to old entries.
    te.env.as_contract(&te.factory_id, || {
        te.env.storage().instance().set(&DataKey::AssetCount, &2u32);
        te.env
            .storage()
            .instance()
            .set(&DataKey::AssetIndex(1), &invalid_asset);
        te.env
            .storage()
            .instance()
            .set(&DataKey::PoolForAsset(invalid_asset), &invalid_pool);
    });

    let aggregate = te.factory.get_aggregate_stats();
    assert_eq!(aggregate.len(), 1);
    assert_eq!(aggregate.get(0).unwrap().0, valid_pool);
}
