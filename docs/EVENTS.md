# Event Catalog

See [public reads and storage cost expectations](READ_METHODS.md) for RPC
pagination guidance; event-driven indexing can avoid repeated full-history reads.

This document lists every event emitted by the ClearLedger smart contracts. Events are published via `env.events().publish(topics, data)` and are indexed by the Go indexer for off-chain consumption.

All amounts are in **USDC stroops** (1 USDC = 10,000,000 stroops).  
All timestamps are **Unix seconds** (u64).  
Invoice IDs are **BytesN<32>** (32-byte identifiers).

---

## Invoice Contract

**Contract:** `invoice_contract`  
**Source:** `contracts/invoice/src/events.rs`

### `invoice_created`

Emitted when a new invoice is created.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_created"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| `topic[2]` | `Address` | Issuer address |
| `topic[3]` | `Address` | Buyer address |
| `topic[4]` | `Address` | Funding asset (USDC token contract) |
| **Data** | `u128` | Face value of the invoice |

**Emitted by:** `create()` in `contracts/invoice/src/lib.rs:276`

---

### `invoice_listed`

Emitted when an invoice is listed for financing.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_listed"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `(u32, u64)` | `(discount_bps, expires_at)` — discount in basis points (1 bp = 0.01%) and the effective listing expiry deadline in Unix seconds |

`expires_at` is computed as `listed_at + expiry_window`, where `expiry_window`
is the window configured at listing time (default `604800`, i.e. 7 days). It is
the deadline used by `expire_listing()` to decide when the listing may be
expired.

> **Data shape change:** topics are unchanged, but the data payload changed
> from a bare `u32` discount to the `(u32, u64)` tuple above. Indexers that
> decoded the old `u32` payload must be updated. The window is read when the
> invoice is listed; an admin change to `set_expiry_window` after listing
> affects the deadline subsequently enforced by `expire_listing()`, so treat
> `expires_at` as the deadline announced at listing time.

**Emitted by:** `list_for_financing()` in `contracts/invoice/src/lib.rs` and
`batch_list_for_financing()` per successfully listed entry

---

### `invoice_funded`

Emitted when an invoice is funded by the pool.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_funded"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `u128` | Funded amount |

**Emitted by:** `mark_funded()` in `contracts/invoice/src/lib.rs:398`

---

### `invoice_shipped`

Emitted when the issuer marks the invoice as shipped.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_shipped"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `()` | None |

**Emitted by:** `mark_shipped()` in `contracts/invoice/src/lib.rs:444`

---

### `delivery_confirmed`

Emitted when either the issuer or buyer confirms delivery.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"delivery_confirmed"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| `topic[2]` | `Address` | Confirmer address (issuer or buyer) |
| **Data** | `()` | None |

**Emitted by:** `confirm_delivery()` in `contracts/invoice/src/lib.rs:513`

---

### `both_confirmed`

Emitted when both issuer and buyer have confirmed delivery (invoice advances to Confirmed).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"both_confirmed"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `()` | None |

**Emitted by:** `confirm_delivery()` in `contracts/invoice/src/lib.rs:508`

---

### `invoice_repaid`

Emitted when the buyer repays the invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_repaid"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `u128` | Repaid amount (face value) |

**Emitted by:** `repay()` in `contracts/invoice/src/lib.rs:600` and `trigger_default()` (recovery path) at line 673

---

### `partial_repayment_received`

Emitted when a buyer makes a partial repayment towards an invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"partial_repayment_received"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `(u128, u128)` | Tuple of `(amount_paid, remaining_balance)` in USDC stroops |

**Emitted by:** `repay_partial()` in `contracts/invoice/src/lib.rs`

---


### `invoice_defaulted`

Emitted when an invoice defaults (past due date, not repaid).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_defaulted"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `()` | None |

**Emitted by:** `trigger_default()` in `contracts/invoice/src/lib.rs:741`

---

### `invoice_expired`

Emitted when an invoice expires (extended past due date without action).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_expired"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `()` | None |

**Emitted by:** `check_expiry()` in `contracts/invoice/src/lib.rs:844`

---

### `expiry_window_set`

Emitted when the admin updates the expiry window.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"expiry_window_set"` |
| **Data** | `u64` | New expiry window in seconds |

**Emitted by:** `set_expiry_window()` in `contracts/invoice/src/lib.rs:755`

---

### `ownership_transferred`

Emitted when contract ownership is transferred to a new admin.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"ownership_transferred"` |
| `topic[1]` | `Address` | Previous admin |
| `topic[2]` | `Address` | New admin |
| **Data** | `()` | None |

**Emitted by:** `transfer_ownership()` in `contracts/invoice/src/lib.rs:1180`

---

### `paused`

Emitted when the admin engages the emergency circuit breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"paused"` |
| `topic[1]` | `Address` | Admin who authorized the pause |
| **Data** | `()` | None |

**Emitted by:** `pause()` in `contracts/invoice/src/lib.rs`

---

### `unpaused`

Emitted when the admin disengages the emergency circuit breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"unpaused"` |
| `topic[1]` | `Address` | Admin who authorized the unpause |
| **Data** | `()` | None |

**Emitted by:** `unpause()` in `contracts/invoice/src/lib.rs`

---

### `pool_contract_updated`

Emitted when the pool contract address is updated.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"pool_contract_updated"` |
| `topic[1]` | `Address` | Old pool contract |
| `topic[2]` | `Address` | New pool contract |
| **Data** | `()` | None |

**Emitted by:** `update_pool_contract()` in `contracts/invoice/src/lib.rs:116` and `118`

---

### `contract_initialized`

Emitted when the contract is initialized.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"contract_initialized"` |
| `topic[1]` | `Address` | Admin address |
| `topic[2]` | `Address` | Registry contract address |
| **Data** | `()` | None |

**Emitted by:** `initialize()` in `contracts/invoice/src/lib.rs:82`

---

### `attestation_submitted`

Emitted when an authorized agent submits a risk attestation for an invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"attestation_submitted"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| `topic[2]` | `Symbol` | Agent ID |
| **Data** | `u32` | Risk score (0–10000 bps) |

**Emitted by:** `submit_attestation()` in `contracts/invoice/src/lib.rs:890`

---

### `agent_registry_contract_updated`

Emitted when the admin updates the agent registry contract address.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"agent_registry_contract_updated"` |
| `topic[1]` | `Address` | Old agent registry contract |
| `topic[2]` | `Address` | New agent registry contract |
| **Data** | `()` | None |

**Emitted by:** `set_agent_registry_contract()` in `contracts/invoice/src/lib.rs:230`

---

### `batch_invoices_created`

Emitted once per `batch_create()` call, after the per-invoice `invoice_created`
events, summarizing the batch outcome.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"batch_invoices_created"` |
| **Data** | `(u32, u32)` | `(created, failed)` counts |

> `failed` is always `0` today: `batch_create()` is atomic and reverts the whole
> batch on the first invalid entry, so a non-zero `failed` can never reach the
> chain. The field exists so indexers can key off a stable batch-summary shape.

**Emitted by:** `batch_create()` in `contracts/invoice/src/lib.rs`

---

### `batch_invoices_listed`

Emitted once per `batch_list_for_financing()` call, summarizing the outcome as
`(listed, failed)` counts. Unlike `batch_create()`, listing is per-entry
tolerant, so `failed` can legitimately be non-zero — the same IDs are also
returned by the call.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"batch_invoices_listed"` |
| **Data** | `(u32, u32)` | `(listed, failed)` counts |

**Emitted by:** `batch_list_for_financing()` in `contracts/invoice/src/lib.rs`

---

## Pool Contract

**Contract:** `pool_contract`  
**Source:** `contracts/pool/src/events.rs`

### `lp_deposited`

Emitted when an LP deposits USDC into the pool.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"lp_deposited"` |
| `topic[1]` | `Address` | LP address |
| **Data** | `(u128, u128)` | `(usdc_amount, shares_issued)` |

**Emitted by:** `deposit()` in `contracts/pool/src/lib.rs:210`

---

### `lp_withdrawn`

Emitted when an LP withdraws USDC from the pool.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"lp_withdrawn"` |
| `topic[1]` | `Address` | LP address |
| **Data** | `(u128, u128)` | `(usdc_amount, shares_burned)` |

**Emitted by:** `withdraw()` in `contracts/pool/src/lib.rs:314`

---

### `invoice_funded`

Emitted when the pool funds an invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_funded"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `u128` | Funded amount |

**Emitted by:** `fund_invoice()` in `contracts/pool/src/lib.rs:464`

---

### `repayment_received`

Emitted when the pool receives repayment for an invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"repayment_received"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `(u128, u128)` | `(amount, yield_amount)` — amount = face value, yield = discount earned |

**Emitted by:** `receive_repayment()` in `contracts/pool/src/lib.rs:551` and `handle_default()` at line 660

---

### `invoice_defaulted`

Emitted when the pool handles a defaulted invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"invoice_defaulted"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `u128` | Loss amount (funded amount lost) |

**Emitted by:** `handle_default()` in `contracts/pool/src/lib.rs:741`

---

### `protocol_fee_updated`

Emitted when the admin changes the protocol fee and/or the treasury address. The
old value is included so an indexer can report what changed rather than only
what it changed to. A rejected call (fee above `MAX_PROTOCOL_FEE_BPS`) emits
nothing.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"protocol_fee_updated"` |
| **Data** | `(u32, u32, Address)` | `(old_fee_bps, new_fee_bps, treasury)` |

**Emitted by:** `set_protocol_fee()` in `contracts/pool/src/lib.rs:1525`  
**Source:** `contracts/pool/src/events.rs:72`

---

### `allowance_approved`

Emitted by the SEP-41 `approve()` whenever an LP's spending grant changes.
`amount` is the new **total** grant (not a delta), and `amount == 0` is the
revoke signal.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"allowance_approved"` |
| `topic[1]` | `Address` | Share owner (`from`) |
| **Data** | `(Address, i128, u32)` | `(spender, amount, expiration_ledger)` |

**Emitted by:** `approve()` in `contracts/pool/src/lib.rs:676`  
**Source:** `contracts/pool/src/events.rs:83`

---

### `ownership_transferred`

Emitted when pool ownership is transferred to a new admin.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"ownership_transferred"` |
| `topic[1]` | `Address` | Old admin |
| **Data** | `Address` | New admin |

**Emitted by:** `transfer_ownership()` in `contracts/pool/src/lib.rs`  
**Source:** `contracts/pool/src/events.rs`

---

### `paused`

Emitted when the admin engages the emergency circuit breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"paused"` |
| `topic[1]` | `Address` | Admin who authorized the pause |
| **Data** | `()` | None |

**Emitted by:** `pause()` in `contracts/pool/src/lib.rs`  
**Source:** `contracts/pool/src/events.rs`

---

### `unpaused`

Emitted when the admin disengages the emergency circuit breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"unpaused"` |
| `topic[1]` | `Address` | Admin who authorized the unpause |
| **Data** | `()` | None |

**Emitted by:** `unpause()` in `contracts/pool/src/lib.rs`  
**Source:** `contracts/pool/src/events.rs`

---

## Escrow Contract

**Contract:** `escrow_contract`  
**Source:** `contracts/escrow/src/events.rs`

### `funds_locked`

Emitted when funds are locked in escrow for an invoice.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"funds_locked"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `u128` | Locked amount |

**Emitted by:** `lock()` in `contracts/escrow/src/lib.rs:118`

---

### `released_to_issuer`

Emitted when locked funds are released to the issuer.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"released_to_issuer"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| `topic[2]` | `Address` | Issuer recorded at lock time |
| **Data** | `u128` | Released amount |

`topic[2]` is always the issuer recorded in the escrow lock. A caller-supplied
address that does not match it is rejected with `InvalidRecipient` before any
transfer or event is emitted.

**Emitted by:** `release_to_issuer()` in `contracts/escrow/src/lib.rs:184`

---

### `released_to_pool`

Emitted when locked funds are released back to the pool (on repayment).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"released_to_pool"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| `topic[2]` | `Address` | Pool contract address |
| **Data** | `u128` | Released amount (repayment amount) |

**Emitted by:** `release_to_pool()` in `contracts/escrow/src/lib.rs:243`

---

### `default_resolved`

Emitted when default is resolved and funds returned to pool.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"default_resolved"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| `topic[2]` | `Address` | Pool contract address |
| **Data** | `u128` | Returned amount |

**Emitted by:** `handle_default()` in `contracts/escrow/src/lib.rs:311`

---

### `history_compacted`

Emitted whenever escrow history is pruned, either automatically on append once
the per-invoice cap is exceeded or explicitly via `compact_history`. Pruned
entries are still recoverable by replaying the per-action escrow events, so
indexers should ingest those events rather than relying on `get_history`.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"history_compacted"` |
| `topic[1]` | `BytesN<32>` | Invoice ID |
| **Data** | `(u32, u32)` | `(retained, removed)` — events still stored, events removed |

**Emitted by:** `append_history()` and `compact_history()` in `contracts/escrow/src/lib.rs`

---

### `ownership_transferred` (dead code)

Emitted when escrow ownership is transferred. Currently unused (marked `#[allow(dead_code)]`).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"ownership_transferred"` |
| `topic[1]` | `Address` | Old admin |
| **Data** | `Address` | New admin |

**Source:** `contracts/escrow/src/events.rs:44`

---

### `paused`

Emitted when the escrow's admin-gated emergency circuit breaker is engaged
(issue #716). `admin` is indexed so indexers can track who flipped the breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"paused"` |
| `topic[1]` | `Address` | Admin that authorized the pause |
| **Data** | `()` | None |

**Emitted by:** `pause()` in `contracts/escrow/src/lib.rs`

---

### `unpaused`

Emitted when the escrow's admin-gated emergency circuit breaker is disengaged.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"unpaused"` |
| `topic[1]` | `Address` | Admin that authorized the unpause |
| **Data** | `()` | None |

**Emitted by:** `unpause()` in `contracts/escrow/src/lib.rs`

---

## Registry Contract

**Contract:** `registry_contract`  
**Source:** `contracts/registry/src/events.rs`

Registry events publish addresses and status changes, but they do not carry
profile metadata. That is not a confidentiality guarantee: metadata is public
through `get_profile`, storage reads, and the transaction arguments of the
register/update call. See [Registry Metadata Privacy and Data Minimization](METADATA_PRIVACY.md)
for the public visibility, data-minimization, and deletion rules.

### `contract_initialized`

Emitted when the registry contract is initialized.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"contract_initialized"` |
| `topic[1]` | `Address` | Admin address |
| **Data** | `()` | None |

**Emitted by:** `initialize()` in `contracts/registry/src/lib.rs:55`

---

### `issuer_registered`

Emitted when an issuer is registered (single or batch).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"issuer_registered"` |
| `topic[1]` | `Address` | Issuer address |
| **Data** | `()` | None |

**Emitted by:** `register_issuer()` in `contracts/registry/src/lib.rs:102` and `batch_register_issuers()` at line 145

---

### `buyer_registered`

Emitted when a buyer is registered.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"buyer_registered"` |
| `topic[1]` | `Address` | Buyer address |
| **Data** | `()` | None |

**Emitted by:** `register_buyer()` in `contracts/registry/src/lib.rs:207`

---

### `profile_updated`

Emitted when an account profile's details or metadata are updated.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"profile_updated"` |
| `topic[1]` | `Address` | Profile address |
| **Data** | `()` | None |

**Emitted by:** `update_profile()` in `contracts/registry/src/lib.rs:323` and `update_metadata()` at line 363

---

### `address_revoked`

Emitted when an address is revoked (issuer or buyer).

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"address_revoked"` |
| `topic[1]` | `Address` | Revoked address |
| **Data** | `RevocationReason` (`u32`) | Machine-readable reason code (see below) |

**Emitted by:** `revoke()` (defaults to `Unspecified`) and
`revoke_with_reason()` in `contracts/registry/src/lib.rs`.

#### Revocation reason codes

`RevocationReason` is a bounded `#[contracttype]` enum serialized as a
`u32` in the event data. These discriminants are ABI-stable; new codes may be
appended, but existing values are never renumbered.

| Code | Variant | Meaning | Suggested display |
|------|---------|---------|-------------------|
| `0` | `Unspecified` | No reason supplied, including all legacy `revoke(address)` calls. | "Revoked" (neutral) |
| `1` | `Compliance` | Regulatory or policy compliance action. | "Revoked: compliance" |
| `2` | `Fraud` | Suspected or confirmed fraud. | "Revoked: fraud" (high severity) |
| `3` | `Inactivity` | Account dormant under the operator's inactivity policy. | "Revoked: inactivity" |
| `4` | `Other` | Any reason not covered above. | "Revoked: other" |

Decoding guidance for off-chain indexers:

- Read the event **data** as a `u32`; do not infer the reason from topics.
- Preserve unknown codes as-is and display a generic "Revoked (reason code N)"
  label so future appended codes degrade gracefully.
- Treat `Fraud` and `Compliance` as high-severity labels; `Unspecified`,
  `Inactivity` and `Other` should not be presented as fraud accusations.
- `verify_profile(address, false)` does **not** emit this event (it emits
  `profile_verified` with `false` and still marks the profile revoked), so it
  carries no reason code.

---

### `address_reinstated`

Emitted when a previously revoked address is reinstated.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"address_reinstated"` |
| `topic[1]` | `Address` | Reinstated address |
| **Data** | `()` | None |

**Emitted by:** `reinstate()` in `contracts/registry/src/lib.rs:546`

---

### `batch_registered`

Emitted when a batch registration completes.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"batch_registered"` |
| **Data** | `(u32, u32)` | `(registered_count, skipped_count)` |

**Emitted by:** `batch_register_issuers()` in `contracts/registry/src/lib.rs:155` and `batch_register_buyers()` at line 218

---

### `profile_verified`

Emitted when a profile's verification status is checked/updated.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"profile_verified"` |
| `topic[1]` | `Address` | Profile address |
| **Data** | `bool` | Verification status (true = verified) |

**Emitted by:** `verify_profile()` in `contracts/registry/src/lib.rs:570`

---

### `ownership_transferred`

Emitted when registry ownership is transferred.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"ownership_transferred"` |
| `topic[1]` | `Address` | Old admin |
| **Data** | `Address` | New admin |

**Emitted by:** `transfer_ownership()` in `contracts/registry/src/lib.rs:593`

---

### `admin_transferred`

Emitted when the registry admin role is transferred.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"admin_transferred"` |
| `topic[1]` | `Address` | Old admin |
| **Data** | `Address` | New admin |

**Emitted by:** `transfer_admin()` in `contracts/registry/src/lib.rs:631`

---

### `paused`

Emitted when the admin engages the emergency circuit breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"paused"` |
| `topic[1]` | `Address` | Admin who authorized the pause |
| **Data** | `()` | None |

**Emitted by:** `pause()` in `contracts/registry/src/lib.rs`  
**Source:** `contracts/registry/src/events.rs`

---

### `unpaused`

Emitted when the admin disengages the emergency circuit breaker.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"unpaused"` |
| `topic[1]` | `Address` | Admin who authorized the unpause |
| **Data** | `()` | None |

**Emitted by:** `unpause()` in `contracts/registry/src/lib.rs`  
**Source:** `contracts/registry/src/events.rs`

---

## Pool Factory Contract

**Contract:** `pool_factory_contract`
**Source:** `contracts/pool_factory/src/events.rs`

### `contract_initialized`

Emitted when the pool factory is initialized.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"contract_initialized"` |
| `topic[1]` | `Address` | Admin address |
| **Data** | `()` | None |

**Emitted by:** `initialize()` in `contracts/pool_factory/src/lib.rs`

---

### `pool_instance_created`

Emitted when `register_asset()` deploys and initializes a new pool instance.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"pool_instance_created"` |
| `topic[1]` | `Address` | Registered asset address |
| **Data** | `Address` | Newly deployed pool address |

**Emitted by:** `register_asset()` in `contracts/pool_factory/src/lib.rs`

---

### `existing_pool_registered`

Emitted when `register_existing_pool()` registers a pre-existing pool for an asset.

| Field | Type | Description |
|-------|------|-------------|
| **Topics** | | |
| `topic[0]` | `Symbol` | `"existing_pool_registered"` |
| `topic[1]` | `Address` | Registered asset address |
| **Data** | `Address` | Existing pool address |

**Emitted by:** `register_existing_pool()` in `contracts/pool_factory/src/lib.rs`

---

## Cross-Reference: Duplicate Event Names

Note that some event names appear in multiple contracts. When indexing, filter by contract address:

| Event Name | Contracts |
|------------|-----------|
| `contract_initialized` | `invoice_contract`, `registry_contract`, `pool_factory_contract` |
| `invoice_funded` | `invoice_contract`, `pool_contract` |
| `invoice_defaulted` | `invoice_contract`, `pool_contract` |
| `ownership_transferred` | All four contracts |
| `paused` / `unpaused` | All four contracts |

---

## Indexing Tips for Go Indexer

1. **Filter by contract address** — each contract has a unique address on Stellar
2. **Use topics for filtering** — `topic[0]` is always the event name (Symbol)
3. **Data decoding** — payloads use Soroban SDK xdr encoding; decode based on the schemas above
4. **Invoice lifecycle** — track `invoice_created` → `invoice_listed` → `invoice_funded` → `invoice_shipped` → `delivery_confirmed` (×2) → `both_confirmed` → `invoice_repaid` / `invoice_defaulted`
5. **Pool events** — `lp_deposited` / `lp_withdrawn` track LP positions; `repayment_received` tracks yield accrual
6. **Escrow events** — `funds_locked` → `released_to_issuer` OR `released_to_pool` / `default_resolved`
7. **Batch events** — `batch_invoices_created` and `batch_invoices_listed` are summary events emitted after the per-invoice events of the same call. `pool::batch_fund_invoice` has no summary event; reconcile it from the individual `invoice_funded` events instead, which is also what tells you which entries a capacity stop skipped.

---

## Related Documentation

- [README](../README.md) — contract overview and deployment addresses
- [THREAT_MODEL.md](./THREAT_MODEL.md) — trust assumptions and auth gates
- [STORAGE.md](./STORAGE.md) — on-chain data layout and TTL patterns
- [LIMITATIONS.md](./LIMITATIONS.md) — known gaps and testnet constraints

## Golden XDR regression fixtures

`tests/integration/fixtures/event-xdr.tsv` pins topics and data as hexadecimal
serialized `ScVal` values for each emitter in invoice, registry, pool, escrow and
pool_factory. `event_golden.rs` invokes the real emitter source in a contract
context and compares both payloads byte for byte. The emitting contract ID is
checked separately and is not part of the topics payload.

Inputs are fixed in each case: contract StrKeys have distinct repeated-byte IDs,
BytesN values have 32 repeated bytes, and u128 values exceed u64::MAX to exercise
both halves. Addresses are synthetic test contracts, not deployed Mainnet assets.
Amount fields remain raw contract integers; fixtures do not assume token decimals.
A TSV row contains `contract.event`, topics XDR hex, then data XDR hex. `SCV_VEC`
is tag 16, symbols are tag 15, u128 is tag 9, and void is tag 1. Decode with the
Stellar XDR ScVal schema before interpreting event-specific fields.

Run `cargo test -p clearledger-integration-tests --test event_golden`. Any schema
change requires reviewing and updating the fixture and corresponding event docs;
tests never regenerate expected bytes. These emitter-level tests complement
workflow coverage: they do not prove that every entry point emits the right event
or that every legacy schema is acceptable to all deployed network versions.
