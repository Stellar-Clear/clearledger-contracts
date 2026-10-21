# Measuring invoice and registry batch host cost

Run the opt-in integration measurement after reviewing the repository:

```sh
cargo test -p clearledger-integration-tests --test lifecycle measure_batch_create_budget_through_fifty_entries -- --ignored --nocapture
```

It prints native test-host CPU instruction and memory counters for fresh invoice
batch_create and registry issuer/buyer registration batches of 1, 10, 25, and 50 entries, with input construction and fixture deployment
excluded. The fixture uses real registry/invoice contracts and mock token and
agent registry. It also checks that 51 entries are rejected. No measurements are
pre-filled: record the toolchain, SDK, network protocol, and commit alongside
actual output before publishing a baseline.

The contract cap of 50 is a validation limit, not a guarantee that 50 entries fit
a network transaction budget. Unlimited native test-host counters do not model
production WASM instruction charges, transaction envelope size, ledger footprint,
fees, storage rent, or contention. State growth and issuer/buyer index duplicate
scans can materially affect cost. This initial harness covers invoice batch_create and registry registration;
listing and funding require independent measurements with realistic liquidity
and distinct invoice fixtures.

For production, simulate the actual WASM transaction against the intended network
and current state. Start with a small batch, retain headroom under all advertised
resource limits, and lower or split the batch when simulation approaches a limit.
Do not derive a universal safe batch size or a fee quote from these counters.
