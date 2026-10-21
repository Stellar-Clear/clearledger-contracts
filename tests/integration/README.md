# Contributing Soroban integration tests

The `clearledger-integration-tests` package lives here. Its library is an empty
`no_std` integration anchor; `tests/lifecycle.rs` exercises the real registry,
invoice, pool and escrow native clients against an in-memory Soroban environment.
Read that file before introducing another parallel set of fixtures.

## Run a focused case

From the repository root with the pinned Rust toolchain installed:

```sh
cargo test -p clearledger-integration-tests --test lifecycle
cargo test -p clearledger-integration-tests --test lifecycle test_cross_contract_invoice_pool_escrow_lifecycle -- --exact
cargo fmt --all --check
```

Use `--locked` when reproducing a checked-in Cargo.lock. No RPC, wallet secret,
Friendbot account or deployed Testnet contract is required for these native tests.
They do not establish deployed WASM costs or real-asset correctness.

## Fixture wiring and assets

`setup_fee_lifecycle()` creates a fresh Env, generated admin/issuer/buyer/LP
addresses, MockToken, registry, invoice, escrow, pool and a mock agent registry.
It initializes escrow before pool so the pool can check the configured asset,
then wires invoice dependencies and a verified issuer/buyer. The helper returns
clients and IDs in `FeeLifecycle`; build additional clients from those IDs.
Clients are native contract clients, not JSON HTTP adapters.

The token fixture stores balances under `TKey(Address)` in persistent storage.
It implements only the operations required by these scenarios and omits important
real-token authorization and trustline semantics. Use its balances to assert
exact deltas, but do not treat a passing MockToken test as SAC or SEP-41 compliance.
Amounts are integers; the fixtures conventionally use seven decimal places.
Synthetic addresses and signing seed constants are test data, not network custody.

Invoice listing now requires a valid agent attestation. Reuse `attest_invoice`,
which signs the actual XDR payload with the fixture key, rather than stubbing out
verification. `create_fee_lifecycle_invoice` creates, attests, lists, funds, marks
shipped and confirms both parties. This helper does not automatically pay the
issuer; inspect current escrow release/repayment behavior before adding payouts.

## Authorization boundaries

The shared setup uses `mock_all_auths_allowing_non_root_auth()` to focus lifecycle
coverage on business behavior. It is unsuitable as evidence that authorization
rejects attackers. For an authorization test, finish setup, clear setup auth with
`env.set_auths(&[])`, then provide narrowly scoped `MockAuth` / `MockAuthInvoke`
entries for the actual caller and exact invocation tree. Exercise both direct
calls and an unrelated forwarding contract. Assert the expected error category
where possible; “some error occurred” can pass because of unrelated validation.
Include a correctly authorized positive control and verify balances/state after
rejection. Never leave mock-all enabled during the negative assertion.

## Two clocks

Import `soroban_sdk::testutils::Ledger` to control the ledger:

```rust
// Advance business deadlines without assuming a ledger duration.
env.ledger().set_timestamp(due_date);
// Advance allowance/storage lifetime independently.
env.ledger().set_sequence_number(env.ledger().sequence() + 1);
```

TTL tests also need the storage testutils traits to inspect TTL. Do not advance
beyond live-entry expiry and assume the entry is still usable: archival and RPC
restoration require separate coverage. Use a fresh Env for independent boundary
cases and retain the original sequence/timestamp when checking no-op behavior.

## Failure, rollback and resource assertions

Capture the pre-call invoice, pool totals, relevant balances, escrow lock and
committed events. Use generated `try_*` clients for an expected failure. Compare
state after rejection and demonstrate a valid retry; do not count successful
setup calls as proof the failure path is atomic. Include an invalid late batch
entry when checking prefix rollback. Avoid depending on test execution order.

Use `env.budget().reset_unlimited()` immediately before an operation when the goal
is measurement, then read `cpu_instruction_cost()` and `memory_bytes_cost()`.
Exclude setup/attestation and post-call assertions from the measurement window.
Native host memory cost is not process RSS; native costs are not WASM network fees.
Unlimited measurement does not prove the operation fits a production resource cap.
A deliberately large correctness loop may disable its budget, but document why
and keep separate resource-limit tests for the behavior being bounded.

Current lifecycle cases document known default-path/reentrancy limitations. Do
not silently bypass a failing end-to-end method and claim that method was tested.
Name the actual entry point exercised and explain any mocked settlement path.
Keep new fixtures small, scope tests to one issue, and add public method/event
changes to the corresponding contract documentation.
