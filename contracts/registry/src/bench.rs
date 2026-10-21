#![cfg(test)]

//! Deterministic resource benchmarks for the batch registration entry points.
//!
//! These measurements use the Soroban test host's budget tracker
//! (`Env::budget`) and count/size the persistent entries that a batch leaves
//! behind. They are regression guards, not wall-clock benchmarks: the recorded
//! baselines and thresholds live in `contracts/registry/BENCHMARKS.md`.
//!
//! Test snapshots are disabled here on purpose — a 50-entry batch produces a
//! large ledger snapshot that has no value as a checked-in fixture, and the
//! storage assertions below already pin the exact persistent footprint.

extern crate std;

use crate::{RegistryContract, RegistryContractClient};
use soroban_sdk::{
    map,
    testutils::{storage::Persistent as _, Address as _, EnvTestConfig},
    xdr::ToXdr,
    Address, Env, Map, String, Vec,
};

/// Batch registration cap enforced by both entry points.
const BATCH_LIMIT: usize = 50;

/// Recorded baselines and thresholds for a 50-entry batch (see BENCHMARKS.md).
///
/// Thresholds are ceilings with headroom above the recorded baseline so small
/// toolchain/host differences do not make the suite flaky, while an accidental
/// change in algorithmic complexity still fails the test.
const ISSUER_50_BASELINE_CPU: u64 = 10_448_977;
const ISSUER_50_MAX_CPU: u64 = 12_000_000;
const ISSUER_50_BASELINE_MEM: u64 = 2_002_819;
const ISSUER_50_MAX_MEM: u64 = 2_500_000;
const ISSUER_50_STORAGE_BYTES: u64 = 14_068;
const ISSUER_50_MAX_STORAGE_BYTES: u64 = 15_500;

const BUYER_50_BASELINE_CPU: u64 = 10_467_405;
const BUYER_50_MAX_CPU: u64 = 12_000_000;
const BUYER_50_BASELINE_MEM: u64 = 2_008_569;
const BUYER_50_MAX_MEM: u64 = 2_500_000;
const BUYER_50_STORAGE_BYTES: u64 = 16_068;
const BUYER_50_MAX_STORAGE_BYTES: u64 = 17_700;

/// Persistent entries a batch of `n` fresh profiles must create: one profile
/// and one index slot per address, plus the role's profile counter.
const fn expected_entries(n: usize) -> u32 {
    if n == 0 {
        0
    } else {
        (n as u32) * 2 + 1
    }
}

#[derive(Debug)]
struct Metrics {
    cpu: u64,
    mem: u64,
    persistent_entries: u32,
    persistent_bytes: u64,
}

fn setup_bench() -> (Env, Address, RegistryContractClient<'static>) {
    let mut env = Env::default();
    env.mock_all_auths();
    env.set_config(EnvTestConfig {
        capture_snapshot_at_drop: false,
    });

    let contract_id = env.register_contract(None, RegistryContract);
    let client = RegistryContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, contract_id, client)
}

fn entries(env: &Env, n: usize) -> Vec<(Address, Map<String, String>)> {
    let mut entries = Vec::new(env);
    for _ in 0..n {
        let metadata = map![
            env,
            (
                String::from_str(env, "name"),
                String::from_str(env, "benchmark profile")
            )
        ];
        entries.push_back((Address::generate(env), metadata));
    }
    entries
}

/// Number of persistent entries and the size of their serialized keys/values.
///
/// The byte count is the sum of the XDR-encoded `ScVal` keys and values, i.e.
/// the payload the registry actually stores. It deliberately excludes the
/// fixed per-entry ledger overhead (durability, TTL, contract id), which is not
/// controlled by the registry.
fn persistent_footprint(env: &Env, contract_id: &Address) -> (u32, u64) {
    env.as_contract(contract_id, || {
        let stored = env.storage().persistent().all();
        let mut bytes = 0u64;
        for (key, value) in stored.iter() {
            bytes += key.to_xdr(env).len() as u64;
            bytes += value.to_xdr(env).len() as u64;
        }
        (stored.len(), bytes)
    })
}

fn measure_issuers(n: usize) -> Metrics {
    let (env, contract_id, client) = setup_bench();
    let entries = entries(&env, n);
    let (before_entries, before_bytes) = persistent_footprint(&env, &contract_id);

    env.budget().reset_default();
    let skipped = client.batch_register_issuers(&entries);
    let cpu = env.budget().cpu_instruction_cost();
    let mem = env.budget().memory_bytes_cost();

    let (after_entries, after_bytes) = persistent_footprint(&env, &contract_id);
    assert!(skipped.is_empty(), "all benchmark addresses are new");
    Metrics {
        cpu,
        mem,
        persistent_entries: after_entries - before_entries,
        persistent_bytes: after_bytes - before_bytes,
    }
}

fn measure_buyers(n: usize) -> Metrics {
    let (env, contract_id, client) = setup_bench();
    let entries = entries(&env, n);
    let (before_entries, before_bytes) = persistent_footprint(&env, &contract_id);

    env.budget().reset_default();
    let skipped = client.batch_register_buyers(&entries);
    let cpu = env.budget().cpu_instruction_cost();
    let mem = env.budget().memory_bytes_cost();

    let (after_entries, after_bytes) = persistent_footprint(&env, &contract_id);
    assert!(skipped.is_empty(), "all benchmark addresses are new");
    Metrics {
        cpu,
        mem,
        persistent_entries: after_entries - before_entries,
        persistent_bytes: after_bytes - before_bytes,
    }
}

#[test]
fn bench_batch_register_issuers_at_limit() {
    let m = measure_issuers(BATCH_LIMIT);
    std::println!("issuer batch @50: {m:?}");

    assert!(
        m.cpu <= ISSUER_50_MAX_CPU,
        "issuer batch CPU {} exceeds threshold {} (baseline {})",
        m.cpu,
        ISSUER_50_MAX_CPU,
        ISSUER_50_BASELINE_CPU
    );
    assert!(
        m.mem <= ISSUER_50_MAX_MEM,
        "issuer batch memory {} exceeds threshold {} (baseline {})",
        m.mem,
        ISSUER_50_MAX_MEM,
        ISSUER_50_BASELINE_MEM
    );
    assert!(
        m.persistent_bytes <= ISSUER_50_MAX_STORAGE_BYTES,
        "issuer batch storage {} exceeds threshold {} (baseline {})",
        m.persistent_bytes,
        ISSUER_50_MAX_STORAGE_BYTES,
        ISSUER_50_STORAGE_BYTES
    );
    assert_eq!(m.persistent_entries, expected_entries(BATCH_LIMIT));
}

#[test]
fn bench_batch_register_buyers_at_limit() {
    let m = measure_buyers(BATCH_LIMIT);
    std::println!("buyer batch @50: {m:?}");

    assert!(
        m.cpu <= BUYER_50_MAX_CPU,
        "buyer batch CPU {} exceeds threshold {} (baseline {})",
        m.cpu,
        BUYER_50_MAX_CPU,
        BUYER_50_BASELINE_CPU
    );
    assert!(
        m.mem <= BUYER_50_MAX_MEM,
        "buyer batch memory {} exceeds threshold {} (baseline {})",
        m.mem,
        BUYER_50_MAX_MEM,
        BUYER_50_BASELINE_MEM
    );
    assert!(
        m.persistent_bytes <= BUYER_50_MAX_STORAGE_BYTES,
        "buyer batch storage {} exceeds threshold {} (baseline {})",
        m.persistent_bytes,
        BUYER_50_MAX_STORAGE_BYTES,
        BUYER_50_STORAGE_BYTES
    );
    assert_eq!(m.persistent_entries, expected_entries(BATCH_LIMIT));
}

#[test]
fn bench_batch_register_boundary_sizes() {
    for n in [0usize, 1] {
        let issuers = measure_issuers(n);
        assert_eq!(issuers.persistent_entries, expected_entries(n));
        let buyers = measure_buyers(n);
        assert_eq!(buyers.persistent_entries, expected_entries(n));
    }
}

#[test]
fn bench_batch_register_rejects_over_limit_without_writes() {
    let (env, contract_id, client) = setup_bench();
    let entries = entries(&env, BATCH_LIMIT + 1);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.batch_register_issuers(&entries);
    }));
    assert!(result.is_err(), "51 issuer entries must be rejected");
    assert_eq!(persistent_footprint(&env, &contract_id).0, 0);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.batch_register_buyers(&entries);
    }));
    assert!(result.is_err(), "51 buyer entries must be rejected");
    assert_eq!(persistent_footprint(&env, &contract_id).0, 0);
}
