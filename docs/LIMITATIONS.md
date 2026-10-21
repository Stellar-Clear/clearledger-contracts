# Limitations

> **Updated:** 2026-09-30
> **Applies to:** ClearLedger protocol on Stellar testnet

This document captures known limitations, testnet-specific constraints,
budget/cost estimates, unimplemented features, and edge cases that are not
yet handled.

---

## Table of Contents

- [Testnet Limitations](#testnet-limitations)
- [Gas & Budget](#gas--budget)
- [Integer Arithmetic and Rounding](#integer-arithmetic-and-rounding)
- [Known Gaps](#known-gaps)
- [Unhandled Edge Cases](#unhandled-edge-cases)
- [Planned But Not Implemented](#planned-but-not-implemented)
- [Resolved](#resolved)
- [Related Issues](#related-issues)

---

## Testnet Limitations

### Friendbot-Derived Accounts

Testnet accounts are funded via Friendbot, which provides a fixed amount of
test XLM. All USDC on testnet is issued by the testnet USDC issuer and has no
real-world value. This means:

- **No real economic incentives** are at play — behaviour on testnet may not
  reflect mainnet dynamics.
- **Friendbot rate limits** can slow down testing. If `setup-testnet.sh`
  returns a `400` from Friendbot, the account is likely already funded
  (the error is benign).

### Ephemeral Validators

Stellar testnet validators may reset state periodically. Do not depend on
testnet contract state for production data.

### No Formal Audit

All contracts are unaudited. See [SECURITY.md](../SECURITY.md#audit-status)
for details.

### Deprecated Test CLI Commands

The `stellar contract` subcommand used in deploy scripts may change with CLI
upgrades. The repository pins `Rust 1.85.0` and the `wasm32v1-none` target,
but the Stellar CLI is expected to be at the latest version
(see [DEPLOYMENT.md](../DEPLOYMENT.md#prerequisites)).

---

## Gas & Budget

### Soroban Resource Model

Soroban charges fees based on a budget model: each operation consumes
CPU instructions, memory, and ledger I/O. The following are approximate
relative costs based on code analysis.

| Operation | Cross-Contract Calls | Token Transfers | CPU Instructions | Memory (Bytes) | Relative Budget |
|-----------|---------------------|-----------------|------------------|----------------|-----------------|
| `registry::register_issuer` | 0 | 0 | — | — | Very Low |
| `registry::revoke` | 0 | 0 | — | — | Very Low |
| `registry::get_profile_count` | 0 | 0 | — | — | Very Low |
| `registry::list_profiles` | 0 | 0 | — | — | Low (O(limit), ≤ 50) |
| `invoice::create` | 2 (`is_verified` ×2) | 0 | — | — | Low |
| `invoice::batch_create` | 2 per entry (`is_verified` ×2) | 0 | — | — | Low per entry, **up to 50× `create`** |
| `invoice::list_for_financing` | 0 | 0 | — | — | Low |
| `invoice::batch_list_for_financing` | 0 | 0 | — | — | Low per entry, **up to 50× `list_for_financing`** |
| `invoice::mark_funded` | 0 | 0 | — | — | Low |
| `invoice::repay` | 1 (`receive_repayment`) | 1 (buyer → pool) | — | — | Medium |
| `invoice::trigger_default` | 1 (`handle_default`) | 0 | — | — | Medium |
| `pool::deposit` (before refactor) | 0 | 1 (LP → pool) | ~456,800 | ~73,400 | Low-Medium |
| `pool::deposit` (after `mint()`) | 0 | 1 (LP → pool) | 457,992 | 73,495 | Low-Medium |
| `pool::withdraw` (before refactor) | 0 | 1 (pool → LP) | ~450,500 | ~63,550 | Medium |
| `pool::withdraw` (after `burn()`) | 0 | 1 (pool → LP) | 451,634 | 63,660 | Medium |
| `pool::fund_invoice` | 6 (`get_status`, `get_funding_asset`, `get_face_value`, `get_discount_bps`, `lock`, `mark_funded`) | 1 (pool → escrow) | — | — | **High** |
| `pool::batch_fund_invoice` | 6 per entry | 1 per entry | — | — | **High** per entry, **up to 50× `fund_invoice`** |
| `pool::receive_repayment` | 0 | 0 | — | — | Low |
| `pool::handle_default` | 1 (`escrow::handle_default`) | 1 (escrow → pool) | — | — | Medium |
| `pool_factory::register_existing_pool` | 0 | 0 | 69,486 | 7,604 | Low |
| `escrow::lock` | 0 | 1 (pool → escrow) | — | — | Medium |
| `escrow::release_to_issuer` | 0 | 1 (escrow → issuer) | — | — | Medium |
| `escrow::release_to_pool` | 0 | 1 (escrow → pool, partial allowed) | — | — | Medium |
| `escrow::handle_default` | 0 | 1 (escrow → pool) | — | — | Medium |

Every mutating entry point above also reads the shared pause flag
(`clearledger-pause`), adding one instance-storage read per call. That read is
instance-local and costs the same in every contract; it is also what pushed a few
long-running integration tests past the default _test-host_ budget, which is why
`pool_factory`'s test `setup()` lifts that budget — see
[Batch Operations](#batch-operations).

### Measured Benchmarks: `deposit()` and `withdraw()` (SEP-41 mint/burn refactor)

In preparation for SEP-41 token interface compliance, `deposit()` and `withdraw()`'s internal LP share storage updates were refactored from inline writes into shared internal `mint()` and `burn()` helpers. Resource measurements were taken via Soroban SDK test environment budget tracking (`env.budget()` in `test_gas_benchmark_deposit_and_withdraw` in `contracts/pool/src/test.rs`):

| Operation    | Implementation                                   | CPU Instructions | Memory (Bytes) | Delta (CPU)     | Delta (Memory) |
| ------------ | ------------------------------------------------ | ---------------- | -------------- | --------------- | -------------- |
| `deposit()`  | Inline `DataKey::LPShares` writes (pre-refactor) | 456,800          | 73,400         | Baseline        | Baseline       |
| `deposit()`  | Routed via `Self::mint()` (post-refactor)        | 457,992          | 73,495         | +1,192 (+0.26%) | +95 (+0.13%)   |
| `withdraw()` | Inline `DataKey::LPShares` writes (pre-refactor) | 450,500          | 63,550         | Baseline        | Baseline       |
| `withdraw()` | Routed via `Self::burn()` (post-refactor)        | 451,634          | 63,660         | +1,134 (+0.25%) | +110 (+0.17%)  |

The benchmark demonstrates negligible gas overhead (~0.25% CPU instruction delta from standard helper call frames) with zero regression to ledger write patterns or storage footprint.

### Budget Considerations

- `pool::fund_invoice` is the most expensive operation — it makes 6
  cross-contract calls (4 invoice reads + `escrow::lock` + `invoice::mark_funded`)
  and performs 1 token transfer. On a congested testnet, this may exceed the
  per-ledger resource limit.
- Read-only view functions (`get_stats`, `get`, `get_profile`, etc.) incur
  minimal cost as they only read from storage.
- Index enumeration functions (`get_by_status`, `get_by_issuer`,
  `get_by_buyer`) are paginated (`page`, `page_size`, capped at
  `MAX_PAGE_SIZE`), so each call hydrates a bounded number of invoices no
  matter how many entries the index holds (issue #71). Callers page until an
  empty result is returned.
- The status index is a bounded membership index (issue #22): entering a
  status appends one slot plus its `StatusPosition`, and leaving swap-and-pops
  in O(1) using that position, so transitions never scan the status's
  historical invoices. `get_by_status` reads only the requested page of live
  members. Pre-upgrade rows without a position are located once on their next
  transition, or eagerly via admin `migrate_status_index`; until migrated, a
  page may be shorter than `page_size` because stale rows are filtered out.
- `get_invoice_count_by_issuer` and `get_invoice_count_by_buyer` avoid that
  cost entirely: they read a single stored counter (`u32`) in O(1), so
  pagination and badge UIs should prefer them over `.len()` on the
  full-fetch views.
- `registry::list_profiles` is bounded per call: it returns at most 50
  addresses (`PageSizeExceeded` above that, mirroring the 50-entry batch cap)
  and costs O(1) in `limit`, not in the total number of registered profiles.
  Read `registry::get_profile_count(role)` once to size the final page, then
  walk `start = 0, limit, 2 * limit, …` until a page comes back short. Each
  enumerated address costs one persistent entry plus its TTL bump, so a page of
  50 is roughly 50 reads — indexers that want thousands of addresses should
  page off-chain rather than in one call.

### Batch Operations

`invoice::batch_create`, `invoice::batch_list_for_financing`, and
`pool::batch_fund_invoice` bound their input at `MAX_BATCH_SIZE` (50 entries)
with `BatchSizeExceeded` (`InvoiceError::28` / `PoolError::26`). The cap exists
so a caller cannot turn one transaction into an unbounded number of cross-contract
calls; a full 50-entry batch is the practical ceiling and can exceed the
per-ledger resource limit on a congested testnet. Callers that hit a budget
error should split the work across several transactions.

The three batch entry points deliberately differ in failure semantics, because
their failure modes differ:

| Entry point                | On a bad entry                      | Why                                                                                                                                                                  |
| -------------------------- | ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `batch_create`             | Reverts the whole batch             | Creation consumes the shared `Counter`; a partial success would burn counter values and persist a prefix the caller cannot identify without re-reading every invoice |
| `batch_list_for_financing` | Skips the entry, returns its ID     | The interesting failures are per-invoice state an issuer listing a backlog cannot avoid (already listed, missing attestation, revoked verification)                  |
| `batch_fund_invoice`       | Stops the batch, returns funded IDs | A liquidity/utilization stop is a normal outcome when an LP fills the pool; partial funding would drain the pool in a way no single `fund_invoice` could             |

`batch_fund_invoice` still reverts on _eligibility_ errors (not listed, already
funded by this pool, unverified issuer/buyer, asset mismatch), because those are
caller mistakes that should be fixed rather than silently skipped. Callers
reconcile the returned funded IDs against their input to see which entries a
capacity stop skipped.

### Storing the `History` Vector (resolved by #51)

`escrow::append_history` stores a `Vec<EscrowEvent>` under
`DataKey::History(invoice_id)` capped at `MAX_HISTORY_EVENTS` (32) entries.
Appends evict the oldest entry once the cap is reached, and the admin-gated
`compact_history(invoice_id, keep)` entry point can tighten retention further
(`1..=32`) in bounded batches of at most 32 removals per call. This keeps the
persistent entry well below the per-entry size limit (~100 KB in practice).
Complete historical detail remains available from the per-action escrow events
for indexers; see [ESCROW_HISTORY.md](./ESCROW_HISTORY.md).

## Integer Arithmetic and Rounding

Contract amounts use integer stroops and basis points; division truncates
toward zero (equivalent to floor for these non-negative values). The current
calculation policy is:

| Calculation | Rounding direction | Boundary behavior |
| --- | --- | --- |
| Invoice funding: `face_value * (10_000 - discount_bps) / 10_000` | Down | Fractional stroops are not funded and remain in the buyer's discount. |
| Protocol fee: `yield * fee_bps / 10_000` | Down | The unallocated whole-stroop remainder is credited to LP yield; treasury plus LPs conserve the yield. |
| Deposit shares: `amount * total_shares / total_deposits` | Down | A result of zero is rejected with `MinimumDeposit`; the smallest accepted amount mints at least one share. |
| Redemption and position value: `shares * total_deposits / total_shares` | Down | A zero-valued withdrawal is rejected; read-only position values may omit a fractional stroop. |
| Early repayment: `discount * elapsed / term` | Down | The buyer receives `discount - earned_by_pool`; fractional accrual is not overpaid to the pool. Partial installments do not separately accrue this discount. |
| Default recovery | No proportional split | Escrow returns the full locked principal; the pool records the full funded amount as realised loss while leaving LP deposits and share value unchanged. |

These are deliberate economic choices, not precision guarantees: callers should
expect at most a small whole-stroop difference from exact rational arithmetic.
Boundary regressions live alongside the pool and invoice contract tests.

---

## Known Gaps

### LP Withdrawal Yield Uses Aggregate Principal

The pool stores one cumulative principal amount per LP rather than deposit
lots. A partial withdrawal allocates that amount pro rata across the LP's
remaining shares. This is an average-basis approximation, not FIFO or HIFO
accounting; after deposits at different share prices it can attribute a
different yield amount to a particular withdrawal. Total USDC returned and
pool share ownership are unaffected, but the LP's cumulative `yield_earned`
report can differ based on the chosen lot-accounting policy.

For example, an LP deposits 1,000 USDC, the pool gains 2%, then the LP deposits
another 1,000 USDC. Withdrawing 500 shares returns about 510 USDC. FIFO would
attribute 500 USDC principal and 10 USDC yield, while aggregate-basis
accounting records about 504.95 USDC principal and 5.05 USDC yield. The
roughly 4.95 USDC difference is about 49.5% of the FIFO yield for this
withdrawal. The regression test in `contracts/pool/src/test.rs` covers this
scenario. Per-deposit lots would be required if the protocol needs a specific
FIFO/HIFO tax or reporting policy.

### Issuer Release Not Wired (Issue #56)

After `fund_invoice` locks USDC in escrow, the pooled funds should be
released to the issuer via `escrow.release_to_issuer()`. This call is
**not yet wired** into `fund_invoice`. See
Issue #56.
**Status:** Unresolved. This is the highest-priority gap before mainnet.
**Workaround:** An admin or automated off-chain bot must call
`escrow.release_to_issuer(invoice_id, issuer)` separately after funding, using
the exact issuer address recorded by `escrow.lock()`. Any other address is
rejected with `InvalidRecipient`.

### No Early Repayment Incentive Logic

The protocol does not apply dynamic discount rates based on how early
repayment occurs. The buyer pays the full `face_value` regardless of how
early they repay, so there is no on-chain rebate proportional to the time
remaining on the invoice.

### No Duplicate Invoice Detection

The `create` function generates an invoice ID from a SHA-256 hash of
(issuer, buyer, face_value, due_date, counter, asset). The counter
guarantees uniqueness even for otherwise identical invoices, but there is
no user-visible warning when an issuer re-submits the same commercial
terms.

---

## Unhandled Edge Cases

### What Happens When…

| Scenario                                                               | Current Behaviour                                                                                   | Notes                                                                                                                                   |
| ---------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| LP deposits in a pool with 0 shares and 0 deposits                     | 1:1 share minting                                                                                   | Correct                                                                                                                                 |
| LP deposits when `total_shares > 0` but `total_deposits = 0`           | Division by zero would panic                                                                        | Cannot happen in practice (shares only minted alongside deposits)                                                                       |
| Buyer repays exactly on `due_date`                                     | `invoice.repay` succeeds; `trigger_default` panics with `DueDateNotPassed`                          | Asymmetric: `repay` has no time gate beyond `status == Confirmed`; `trigger_default` requires `current_time > due_date` (strict)        |
| `confirm_delivery` called after invoice is already confirmed           | Panics with `InvalidStatusTransition` because status is `Confirmed` not `Active`                    | Correct — prevents replay                                                                                                               |
| `withdraw` when pool has funded invoices but no available liquidity    | Panics with `InsufficientLiquidity`                                                                 | Correct — prevents breaking the pool invariant                                                                                          |
| Admin attempts `set_max_utilization(0)`                                 | The configuration call fails with typed `InvalidConfiguration`; the previous cap remains active      | Zero is rejected because it would make every positive funding attempt fail                                                              |
| Calling `revoke` on an unregistered address                            | Panics with `NotFound`                                                                              | Per spec                                                                                                                                |
| Calling `batch_register_issuers` with more than 50 entries             | Panics with `BatchSizeExceeded`                                                                     | Gas protection                                                                                                                          |
| A buyer repays _less_ than `face_value`                                | Not possible on the buyer path — `invoice.repay` transfers the full `face_value` from buyer to pool | Partial amounts are only possible through `escrow::release_to_pool` during default / partial-repayment flows; see [Resolved](#resolved) |
| An issuer creates an invoice with `due_date = current_time + 1` second | Invoice accepted                                                                                    | `create` requires `due_date > env.ledger().timestamp()` (strict); `now + 1` passes, `now` does not                                      |
| The pool has exactly 0 USDC balance after funding all deposits         | `withdraw` succeeds for unfunded amounts, but funded invoices are locked                            | Escrow holds funded USDC, pool holds only unfunded USDC                                                                                 |
| `trigger_default` while status is exactly at `due_date` (timestamp)    | Panics with `DueDateNotPassed`                                                                      | The guard is `if current_time <= due_date`, so one full second must elapse past `due_date` before default can fire                      |

---

## Planned But Not Implemented

These features are tracked in the repository's issue tracker. Cross-links
are provided where available.

### UI / Frontend

- Event-driven Go indexer (see ClearLedger-app)
- Transaction simulation before signature

### Smart Contracts

- Multi-sig admin (3-of-5 Stellar signers)
- LP-governed invoice funding (stake LP tokens to vote on invoices). Design
  proposal: [NEW_DESIGN.md](NEW_DESIGN.md) (issue #718)
- Dynamic utilization-based interest rate model
- Swap-free multi-asset pools (USDC + XLM)
- On-chain governance for protocol parameters

### Tooling & Infrastructure

- Mainnet deployment support (currently testnet only)
- Formal verification or symbolic execution audit
- `cargo-soroban` integration for automated WASM budget reporting
- TypeScript SDK for contract interaction
- Automated fuzz testing across all four contracts
- Smart-contract upgrade mechanism (`__constructor` + `__upgrade`)

> **Note:** Partial repayments are now supported on the **escrow** path
> (`escrow::release_to_pool(invoice_id, repayment_amount)` accepts a
> partial `repayment_amount` and tracks the residual in the escrow
> record). They are **not** yet supported on the buyer-driven repayment
> path (`invoice::repay` still requires the full `face_value`). This
> item has been removed from the active scope above and moved to the
> [Resolved](#resolved) section.

---

## Resolved

These limitations were previously listed here but have been fixed since
the 2025-07-25 revision.

> **Correction (issue
> #835):**
> An earlier entry here claimed that "Index entries not compacted on status
> transition" was resolved because `move_status_index` performs O(1)
> membership checks via `DataKey::StatusMembership` (crediting PR #121).
> No such storage key exists anywhere in `contracts/invoice/src/lib.rs`.
> In reality `move_status_index` and the `extend_*_index` helpers linearly
> scan the index on every transition, index rows are still not compacted,
> and `get_by_status` skips stale rows only by loading each invoice and
> comparing its status. This was subsequently fixed by issue #22: the status
> index is now a compacted membership index with an O(1)
> `DataKey::StatusPosition` slot map, swap-and-pop removal, and a bounded
> admin migration (`migrate_status_index`) for pre-existing rows. See
> [STORAGE.md](STORAGE.md#status-index-migration-upgrade-note). The historical
> tracker was #831.

- **Pool does not track individual LP yield accrual.** `pool::withdraw`
  now writes `DataKey::LPYieldEarned(lp)` and `pool::get_lp_position`
  returns the running yield figure per LP. Yield is therefore visible
  before the LP triggers a withdrawal.
- **Partial default-side repayment.** `escrow::release_to_pool` accepts
  a `repayment_amount` that may be less than the original locked amount,
  keeps the residual escrow record in place, and emits history events
  per partial release. See PR #120
  (`feat/partial-repayment-default-flow`). Buyer-driven partial
  repayments via `invoice.repay` remain a future-feature — see
  [Known Gaps](#known-gaps).
- **Security findings #94, #96, #98, #103.** Addressed together in
  PR #124 (`fix: address security issues #94, #96, #98, #103`).

---

## Related Issues

- #56 — Wire issuer release into fund_invoice (open)
- #285 — Event schema documentation
- #286 — Gas budget report
- #287 — Storage schema docs
- #294 — Testnet limitation docs
- #307 — General repo documentation
- #460 — LIMITATIONS.md stale date (this PR fixes it)
- #831 — Index helpers linearly scan whole indexes (fixed for the status index by issue #22; issuer/buyer indexes remain append-only by design)
- #835 — LIMITATIONS.md documents a `DataKey::StatusMembership` that does not exist (this PR fixes it)

Resolved items reference the PRs that closed them:

- Issue #67 — status index filtering (closed by PR #121; the O(1) membership claim was retracted — see #835 and #831)
- Issues #94, #96, #98, #103 — security (closed by PR #124)
- Partial default repayment (closed by PR #120)
