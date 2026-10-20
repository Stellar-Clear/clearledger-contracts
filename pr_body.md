## Summary

Implements pre-listing invoice cancellation, admin-gated Wasm upgrades for the
invoice and escrow contracts, and deployment guidance for Stellar multisig
admins.

## Changes

- Add `InvoiceStatus::Cancelled` without changing existing status discriminants.
- Add `invoice::cancel(invoice_id)`, requiring issuer authorization and
  accepting only invoices in `Created` status. Update the status count/index and
  emit `invoice_cancelled`.
- Add admin-gated `upgrade(new_wasm_hash)` entry points and `upgraded` events to
  invoice and escrow.
- Document the upgrade path and a worked 3-of-5 Testnet multisig setup. Link the
  README centralization-risk section to the deployment guidance.
- Add cancellation transition/auth tests and negative-auth tests for upgrades.

## Tests

- [x] `cargo fmt --all -- --check`
- [x] `cargo test -p clearledger-invoice -p clearledger-escrow`
- [x] `cargo test -p clearledger-pool -p clearledger-registry -p clearledger-integration-tests`
- [x] `cargo test -p clearledger-ttl`
- [ ] `cargo test` — 10 existing `pool_factory` tests fail with
  `Func(MismatchingParameterLen)` during contract initialization; reproduced
  when running `clearledger-pool-factory` alone.

Closes #727, #726, #723, #717
