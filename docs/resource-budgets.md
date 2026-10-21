# Native Soroban resource budget reports

Run `bash scripts/report-budgets.sh [output.json]` from a trusted checkout with
the Rust toolchain and dependencies installed. The script uses Cargo.lock and
runs only the ignored measurement test, serially. It writes and validates a
machine-readable report after all seven workflows succeed. Normal test runs do
not generate or overwrite a baseline.

Measurements reset the budget immediately before create, list, fund, repay,
deposit, withdraw and pool.handle_default; setup, attestation and state assertions
are excluded. Repayment and default use separate, deterministic fixtures. Default
uses the pool entry point because the existing lifecycle suite documents a
reentrancy issue in invoice.trigger_default. It does not represent that broken
end-to-end path. The fixture uses the existing mock token and mocked authorization,
a 30-day invoice, 10,000,000,000 face-value stroops, 12,000,000,000 deposit stroops
and 200 bps discount. No real USDC or network account is involved.

The JSON includes schema version, execution mode, fixture parameters and one
`workflow`, `cpu_instructions`, `memory_bytes` row per operation. Memory is the
Soroban host budget's metered memory cost, not process RSS. Unlimited limits allow
measurement without conflating a configurable cap with operation cost.

Before publishing a baseline, run the script twice on the same revision and
compare results; attach the JSON plus Git SHA, Cargo.lock hash, rustc version and
host platform to the PR/release artifact. Compare the same SDK/toolchain, fixture
and execution mode when reviewing regressions. Do not treat native testutils
figures as WASM execution costs, network fees or production transaction ceilings.
A numeric baseline has not been fabricated or checked in: this PR supplies the
repeatable generator; a measured artifact still requires running it.
