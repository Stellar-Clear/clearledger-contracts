# Registry batch registration benchmarks

These benchmarks measure the two admin batch entry points at the 50-entry cap
imposed by `BatchSizeExceeded`. They run in the Soroban test host and are
asserted by `src/bench.rs`, so the suite fails if CPU, memory, or storage grows
past the thresholds below.

## Recorded baseline (50 entries)

Each entry is a fresh address with one metadata pair (`name` =
`benchmark profile`). The budget is reset immediately before the contract call,
so the numbers include the full invocation: pause check, auth, metadata
validation, profile/index writes, TTL extension, and events.

| Entry point              | CPU instructions | Memory bytes | Persistent entries | Serialized key/value bytes |
| ------------------------ | ---------------: | -----------: | -----------------: | -------------------------: |
| `batch_register_issuers` |       10,448,977 |    2,002,819 |                101 |                     14,068 |
| `batch_register_buyers`  |       10,467,405 |    2,008,569 |                101 |                     16,068 |

The 101 persistent entries are 50 profiles, 50 `ProfileIndex` slots, and one
`ProfileCount` entry. The buyer batch stores 2,000 more bytes because buyers
persist the supplied metadata while the issuer batch follows its existing
behavior of storing an empty metadata map.

## Regression thresholds

| Entry point              | CPU max  | Memory max | Storage bytes max |
| ------------------------ | -------: | ---------: | ----------------: |
| `batch_register_issuers` | 12,000,000 | 2,500,000 |           15,500 |
| `batch_register_buyers`  | 12,000,000 | 2,500,000 |           17,700 |

The ceilings leave roughly 10-25% headroom over the recorded baseline. Storage
bytes sum the XDR-encoded keys and values only; fixed per-entry ledger overhead
(durability, TTL, contract id) is excluded.

## Boundary coverage

`src/bench.rs` also asserts the persistent footprint for empty and single-entry
batches (0 and 3 entries respectively), and that a 51-entry batch is rejected
without writing anything. Invalid metadata and over-limit failures are covered
by the existing registry tests.

## Notes

- Values come from `Env::budget()` (`cpu_instruction_cost` /
  `memory_bytes_cost`) and are deterministic for the pinned `soroban-sdk`
  version.
- Per the SDK, CPU and memory measured from native Rust tests underestimate the
  equivalent WASM execution; treat these as relative regression guards, not
  on-chain fee estimates.
- Run with `cargo test -p clearledger-registry bench_batch_register_ -- --nocapture`.
