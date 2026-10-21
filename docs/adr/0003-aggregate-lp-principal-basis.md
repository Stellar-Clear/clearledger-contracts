# ADR 0003: Use Aggregate Pro-Rata LP Principal Basis

- **Status:** Accepted
- **Date:** 2026-10-19

## Context

An LP may deposit more than once, including at different share prices. The pool
tracks one `LPInitialDeposit` amount and one share balance per address, not
per-deposit lots. Withdrawal yield attribution therefore needs a policy for
allocating that aggregate principal across redeemed shares.

## Decision

Use aggregate principal basis, allocated pro rata by shares on every
withdrawal. For a withdrawal of `shares` from a pre-withdrawal balance
`lp_shares`, the principal portion is the current aggregate principal multiplied
by `shares / lp_shares`, rounded down using integer division. Realized yield is
the positive excess of the returned asset amount over that principal portion;
losses do not create negative `yield_earned`. The remaining aggregate principal
is reduced by the allocated portion. A full withdrawal clears principal so a
later deposit starts a new accounting cycle, while previously realized yield
remains cumulative.

This is intentionally not FIFO, LIFO, or HIFO lot accounting. When deposits
occur at different share prices, the yield attributed to a particular
withdrawal can differ from a lot-based policy. Share transfers do not transfer
principal history; the accounting position remains attached to the LP address.

## Consequences

- Storage and withdrawal work remain bounded independently of deposit count.
- Partial withdrawals consistently reallocate the remaining basis across the
  remaining shares.
- `yield_earned` is a realized, cumulative statistic under this policy, not a
  tax basis, per-lot return, or total unrealized position yield.
- Tests should cover mixed-price deposits, multiple partial withdrawals, full
  withdrawal, and a fresh deposit after a completed cycle.

## References

- `contracts/pool/src/lib.rs` (`deposit`, `withdraw`, and `get_lp_position`)
- `contracts/pool/src/types.rs` (`LPPosition`)