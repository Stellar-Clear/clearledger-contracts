## Summary

Fixes four issues in the pool contract (`contracts/pool/src/`):

- **#263** — `initialize()` now rejects any pairwise collision among `admin`, `invoice_contract`, `escrow_contract`, and `usdc_asset`, panicking with the new `PoolError::InvalidConfiguration` (#15). Previously nothing stopped these four addresses from being configured identically, which could let the `handle_default` gate collide with the admin path.
- **#265** — `fund_invoice()` now checks for an existing `FundedInvoice` entry before writing one, panicking with the new `PoolError::AlreadyFunded` (#16) instead of silently overwriting a prior entry (which would double-lock escrow funds and double-count `active_invoice_count` for a single invoice).
- **#268** — Added `test_withdraw_after_repayment_returns_more_than_deposited`, covering the previously untested "yield grows share price" invariant end-to-end: deposit → fund → repay → withdraw the same shares, asserting the returned USDC exceeds the deposit by exactly the discount portion of yield.
- **#281** — Every instance/persistent `extend_ttl` call in the pool contract now goes through shared `THRESHOLD`/`EXTEND_TO` constants (new `contracts/pool/src/ttl.rs`) instead of repeating the `100, 2_000_000` literals. Instance TTL extension was already wired into every state-changing entrypoint; added a regression test (`test_deposit_extends_instance_ttl_when_below_threshold`) that drives the instance ttl below the threshold and confirms a state-changing call bumps it back up.

Also added:
- `test_initialize_accepts_distinct_addresses` and `test_initialize_rejects_each_pairwise_address_collision` (all 6 pairwise combinations) for the new `InvalidConfiguration` guard.
- `test_fund_invoice_rejects_replay_when_funded_invoice_entry_exists` and `test_fund_invoice_succeeds_when_no_prior_funded_entry` for the new `AlreadyFunded` guard.
- Rustdoc `# Panics` entries updated on `initialize` and `fund_invoice` for the new error variants.

Closes #263, Closes #265, Closes #268, Closes #281

## Test plan

- [x] `cargo fmt --all --check`
- [x] `cargo clippy --all-targets -- -D warnings` (workspace-wide, no new warnings)
- [x] `cargo test -p clearledger-pool` (58 passed, 0 failed)
- [x] `cargo test --workspace` (194 passed, 0 failed, across registry/invoice/escrow/pool)
