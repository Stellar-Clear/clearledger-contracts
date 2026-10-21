# Escrow History Retention Policy

> **Applies to:** `escrow_contract` (`contracts/escrow`)
> **Issue:** #51 — define a compaction policy for long-lived escrow histories

Escrow history records the state transitions of a locked invoice. It is useful
for clients that want a single-call view of recent activity, but it must not grow
without bound for invoices that are funded and released repeatedly.

## Storage model

On this branch history is a **single persistent vector per invoice**, stored at:

```
DataKey::History(BytesN<32>) -> Vec<EscrowEvent>
```

There are no per-event storage keys. A record is appended by `lock`,
`release_to_issuer`, `release_to_pool`, and `handle_default`, and the vector's
TTL is extended on every append.

## Retention policy

| Constant | Value | Purpose |
|----------|-------|---------|
| `MAX_HISTORY_EVENTS` | 32 | Hard cap. A 33rd append evicts the oldest event, so on-chain storage is always `<= 32` entries per invoice. |
| `MIN_HISTORY_EVENTS` | 1 | Lower bound accepted by `compact_history`; the latest transition is never removed by compaction. |
| `MAX_COMPACT_PER_CALL` | 32 | Upper bound on entries removed by one `compact_history` call. |

### What is retained on-chain

- The most recent **up to 32** events per invoice, in chronological order
  (oldest first), including `invoice_id`, `action`, `amount`, `timestamp`, and
  `caller`.
- Retention can be tightened to `1..=32` events with `compact_history`.

### What is only available via events/indexers

Events evicted from the vector are **not deleted from ledger history**. The
escrow contract emits one event per transition:

- `funds_locked`
- `released_to_issuer`
- `released_to_pool`
- `default_resolved`
- `history_compacted` (records `(retained, removed)`)

An indexer that ingested those events retains the full audit trail even after
on-chain compaction. `get_history` is a convenience view of the recent tail, not
an authoritative archive.

## ABI

```rust
/// Admin-only. Prune the oldest history entries for `invoice_id`, keeping the
/// newest `keep` in their original order.
pub fn compact_history(env: Env, invoice_id: BytesN<32>, keep: u32) -> u32;
```

- **Auth:** stored `admin` (`admin.require_auth()`).
- **Parameters:** `keep` must be in `MIN_HISTORY_EVENTS..=MAX_HISTORY_EVENTS`.
- **Returns:** number of events removed by this call (`0` when already compact
  enough).
- **Bounded:** removes at most `MAX_COMPACT_PER_CALL` entries per call. A legacy
  history larger than that may need repeated calls until the return value is
  `0`.
- **Idempotent:** once the retained length is `<= keep`, further calls return
  `0` and do not rewrite storage.
- **Event:** `history_compacted(invoice_id, retained, removed)` when
  `removed > 0`.
- **Errors:**
  - `NotInitialized` (#6) — contract has no stored admin.
  - `InvalidRetention` (#13) — `keep` is `0` or greater than
    `MAX_HISTORY_EVENTS`.
  - `NotFound` (#2) — no history vector exists for the invoice.
  - `NotAuthorized` / auth failure — caller is not the stored admin.

```rust
/// Read-only. Returns the retained tail, oldest first, or an empty vector.
pub fn get_history(env: Env, invoice_id: BytesN<32>) -> Vec<EscrowEvent>;
```

### Why admin-only?

Compaction removes data that an on-chain client can no longer read. A
permissionless caller could prune an invoice before a client or auditor had a
chance to sync, so the operation is deliberately gated to the accountable
admin. Automatic append-time trimming is the exception: it requires no auth
because it is a deterministic, bounded consequence of a normal state
transition and retains the newest window.

### Error-code allocation

`EscrowError` codes `9`/`10` are reserved for issue #83 and `11`/`12` for issue
#50. The new `InvalidRetention` is therefore **#13**. `NotFound` (#2) is reused
for a missing history key; no new code was needed for that case.

## Client guidance

- Treat `get_history` as a **recent-activity window**, not an archive. Do not
  assume the first event is the invoice's original `Locked` event once an
  invoice has more than 32 transitions.
- If you need the complete history, subscribe to the escrow events listed above
  and persist them off-chain keyed by `invoice_id`.
- After a compaction, re-read `get_history` rather than assuming indices are
  stable; eviction always removes from the front, so surviving events keep
  their relative order.

## Indexer guidance

- Ingest `funds_locked`, `released_to_issuer`, `released_to_pool`, and
  `default_resolved` as the source of truth for complete history.
- Record `history_compacted` so you can mark the on-chain tail boundary
  (`retained`, `removed`) and avoid treating a short `get_history` response as
  missing data.
- Pruning is front-only and never reorders or rewrites entries, so event-derived
  history and the remaining vector agree on the overlapping suffix.

## Operational guidance

- New histories are bounded automatically; no maintenance call is required for
  normal operation.
- Run `compact_history` only to tighten retention below 32 or to drain a legacy
  history written before this policy. Repeat until it returns `0`.
- Pruning requires the admin key. Protect it with a multisig as described in
  [DEPLOYMENT.md](../DEPLOYMENT.md).
