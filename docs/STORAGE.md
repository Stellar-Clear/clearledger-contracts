# Storage Schema

> **Updated:** 2025-07-25
> **Applies to:** All four Soroban smart contracts

This document describes the on-chain storage layout for every contract in the
ClearLedger protocol. Integrators, indexer operators, and contributors should
refer to this when designing off-chain read paths or auditing storage usage.

---

## Table of Contents

- [Storage Overview](#storage-overview)
- [TTL Management](#ttl-management)
- [Contract: registry\_contract](#contract-registry_contract)
- [Contract: invoice\_contract](#contract-invoice_contract)
- [Contract: escrow\_contract](#contract-escrow_contract)
- [Contract: pool\_contract](#contract-pool_contract)

---

## Storage Overview

Soroban storage is key-value, typed, and partitioned into two
`Env::storage()` namespaces:

| Namespace | Use | TTL | Used For |
|-----------|-----|-----|----------|
| `instance()` | Singleton keys | Extended on every state-changing call | Admin addresses, contract references, configuration values |
| `persistent()` | Keyed entries | Extended per-entry on write | Profiles, invoices, escrow records, LP positions |

All amount fields (`u128`) are denominated in **USDC stroops**
(1 USDC = 10,000,000 stroops, 7 decimals).

All timestamps (`u64`) are **Unix time** (seconds since epoch).

---

## TTL Management

All contracts follow a consistent TTL extension pattern:

| Storage Type | `live_until_ledger` | `seq` (TTL entries) |
|-------------|---------------------|---------------------|
| Instance (every state-changing call) | 100 | 2,000,000 |
| Persistent (after every `set()`) | 100 | 2,000,000 |

> **Important:** Every call to `persistent().set()` in the codebase is
> immediately followed by `persistent().extend_ttl()`. The `extend_instance_ttl()`
> helper is called at the end of every public state-changing function.

## Caller-Controlled Persistent Data Audit

Persistent values and contract events are public ledger data. TTL limits how
long a value remains live; it does not make the value private, and a hash does
not conceal a low-entropy secret. Do not submit personal data, credentials,
private reports, or other confidential material in metadata, addresses,
symbols, event fields, or signed payloads. Attestations store the evidence hash,
not the off-chain evidence itself.

| Contract / key family | Caller-controlled data and bounds | Retention / growth | Privacy and trust boundary |
|---|---|---|---|
| Registry `Profile(Address)` | Address is a fixed Soroban `Address`. Metadata accepts at most 20 entries; each non-empty key is at most 64 bytes and each non-empty value at most 512 bytes. | Persistent profile TTL is extended on registration and update. Profile and role-index cardinality has no protocol-wide cap; batch registration is limited to 50 entries per call. | A profile owner signs self-registration and metadata updates; admin batch registration is trusted. Profile values and addresses are public. |
| Registry `ProfileIndex` / `ProfileCount` | Fixed-width role, `u32` index/count, and address values. | Append-only; no deregistration or index compaction. Unique profile count can grow without a global limit. | Public enumeration exposes registered addresses and their registration order. |
| Invoice `Invoice`, indexes, `Attestation`, `AttestationExpiry` | Invoice IDs/evidence hashes are `BytesN<32>`, addresses are fixed Soroban `Address`, and scalar fields are fixed-width. Creation caps face value at `MAX_FACE_VALUE` and due date at 10 years. Attestation `agent_id` is a bounded `Symbol`; `risk_score` is `u32` with no semantic range check; nonce/expiry are `u64`. Raw XDR payload must decode but has no application-specific encoded-length cap and is not stored. | Invoice, index, attestation, and expiry entries receive persistent TTL extensions when written. Index entries are bounded per invoice but aggregate invoice count has no global cap. | Issuer/buyer authorize invoice actions; attestation trust is the signature plus active agent registry record. Stored scores, identifiers, and evidence hashes are public. Signed payload wire format includes `expires_at`; see [invoice lifecycle](../contracts/invoice/STATE_TRANSITIONS.md). |
| Pool LP positions and `Allowance(Address, Address)` | Address keys and numeric share/deposit/yield values are fixed-width. Allowances reject negative amounts, expire by ledger sequence, and amount zero removes the key. | Position keys are per address; allowance keys are per address pair. No global address-count cap. Non-zero writes extend TTL; expired grants read as zero. | LP authorizes share changes and grants; spender authorizes `transfer_from`. Balances, counterparties, and amounts are public. |
| Pool `FundedInvoice(BytesN<32>)` | Invoice ID key with a fixed-width `u128` funded amount, populated from verified invoice terms. | Persistent TTL is extended when funding is recorded; the entry is removed when settled/defaulted. Number of funded invoice IDs has no global cap beyond pool liquidity and network resources. | Pool controls writes after reading invoice/registry state. Invoice ID and funded amount are public. |
| Pool instance configuration | Admin supplies treasury address, minimum deposit, share name/symbol and decimals at initialization; name/symbol must be non-empty but have no application-level length cap, and decimals are any `u32`. Protocol fee is admin-set and capped by `MAX_PROTOCOL_FEE_BPS`. | Stored in instance storage and covered by instance TTL extension. | Admin is the trust boundary; configuration and share metadata are public. |
| Escrow `Locked(BytesN<32>)` | Fixed-width invoice ID, amount, timestamps, and issuer address. | One active lock per invoice; removed on release/default. Persistent TTL is extended on write. | Only the configured pool can lock/release; authorized release paths determine the recipient. Amounts and parties are public. |
| Escrow `History(BytesN<32>)` | Append-only `Vec<EscrowEvent>` containing fixed-width invoice ID, action, amount, timestamp, and optional caller address. | Each action appends and extends TTL. There is no application-level vector-length cap or compaction; storage/resource limits are the effective bound. | Only escrow-authorized flows append, but history and caller addresses are public. Never treat it as a private audit channel. |

All caller-provided byte/string inputs are also subject to Soroban transaction and
resource limits. Those network limits are not a substitute for application
validation: registry metadata caps are explicit, while escrow history and the
encoded attestation payload have no application-level length cap, and attestation
scores have no semantic range check. This audit describes the
contract behavior in this checkout; deployment TTL settings and archival
policies remain network configuration.

---

## Contract: registry_contract

### Instance Storage

| DataKey | Type | Description | Set During |
|---------|------|-------------|------------|
| `Admin` | `Address` | Contract admin address | `initialize()` |

### Persistent Storage

#### Profile Entry

| DataKey | Type | Description |
|---------|------|-------------|
| `Profile(Address)` | `Profile` | Profile record keyed by Stellar address |

#### Profile Enumeration Index

| DataKey | Type | Description |
|---------|------|-------------|
| `ProfileIndex(Role, u32)` | `Address` | The `index`-th registered address for `Role::Issuer` / `Role::Buyer`, in registration order |
| `ProfileCount(Role)` | `u32` | Number of populated `ProfileIndex` slots for that role |

Both keys are written by **every** registration path (`register_issuer`,
`register_buyer`, `batch_register_issuers`, `batch_register_buyers`) and are
TTL-extended with the same threshold/target as a profile entry
(`persistent().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO)`), on write and on read.
Entries skipped as already-registered are never indexed twice, so
`ProfileCount(role)` always equals the number of addresses reachable through
`list_profiles(role, …)`.

#### Profile Structure

```
Profile {
    packed_flags: u32,              // Bit 0: role (0=Issuer, 1=Buyer)
                                    // Bit 1: verified status
                                    // Bit 2: revoked status
    registered_at: u64,             // Unix timestamp of registration
    metadata:     Map<String, String>, // Arbitrary key-value metadata
}
```

The Stellar `Address` is not stored inside the struct: it is the
`DataKey::Profile(Address)` key and the profile's public identity. All profile
fields are public, and `get_profile(address)` returns the complete metadata map
without authentication.

Metadata is limited to 20 entries, with non-empty keys up to 64 bytes and
non-empty values up to 512 bytes (`RegistryError::InvalidMetadata`, `#6`). The
contract enforces only those size rules; it does not detect or redact personal
data. Treat every metadata key and value as public and effectively permanent:
there is no on-chain erasure, and updates leave earlier values in transaction
history. See [Registry Metadata Privacy and Data Minimization](METADATA_PRIVACY.md)
before storing references or submitting profile metadata.

#### Indexing Notes

- Profiles are looked up by `is_verified(address)`; the per-role
  `ProfileIndex` / `ProfileCount` keys back the enumeration views
  `get_profile_count(role)` and `list_profiles(role, start, limit)`.
- `list_profiles` returns pages of at most 50 addresses (the same cap as the
  batch registration entry points) and panics with `PageSizeExceeded` (`#8`)
  above it, so a single call can never walk the whole index. Pages are
  contiguous and non-overlapping: page `n` is
  `list_profiles(role, n * limit, limit)`.
- The index is append-only. The registry has no deregistration path —
  `revoke` only flips the profile's verification flags — so a slot is never
  rewritten or freed, and revoked profiles stay enumerated.
- `get_profile(address)` returns the full `Profile` struct or panics with
  `NotFound` if absent.
- `get_verification_status(address)` returns a three-valued enum
  (`Unregistered`, `Verified`, `Revoked`).

### Batch Registration Semantics

`batch_register_issuers` / `batch_register_buyers` process entries in order
and never revert merely because an address is already registered:

| Case | Behaviour |
|------|-----------|
| Address already registered (by a single or earlier batch call) | Skipped; existing profile is untouched and the address is returned in the skipped list |
| Same address repeated within one batch | First occurrence is registered; later occurrences are skipped and reported |
| All entries are duplicates | Nothing is written; the result lists every skipped occurrence |
| Valid entries mixed with duplicates | Valid new entries are registered; only duplicates are skipped |
| Invalid metadata | Entire call panics with `InvalidMetadata`; no entry from the batch is persisted |
| More than 50 entries | Entire call panics with `BatchSizeExceeded`; nothing is persisted |

On-chain `Address` values are always structurally valid, so there is no
invalid-address branch. Single-address registration differs deliberately:
`register_issuer` / `register_buyer` panic with `AlreadyRegistered` instead of
skipping.

### Storage Key Count

**Approximately 2 keys per registered address** — one `Profile` record and one
`ProfileIndex(role, n)` slot — **plus 2 constant `ProfileCount(role)` keys**
(one per role) that exist once the first profile of that role is registered.

---

## Contract: invoice_contract

### Instance Storage

| DataKey | Type | Description | Set During |
|---------|------|-------------|------------|
| `Admin` | `Address` | Contract admin | `initialize()` |
| `RegistryContract` | `Address` | Registry contract address | `initialize()` |
| `PoolContract` | `Address` | Pool contract address | `set_pool_contract()` |
| `Counter` | `u64` | Monotonically increasing invoice counter | `initialize()`, incremented on `create()` |
| `ExpiryWindow` | `u64` | Listing expiry window in seconds (default `604800`) | `set_expiry_window()` |
| `SupportedAssetCount` | `u32` | Number of supported funding assets | `add_supported_asset()` |
| `SupportedAsset(Address)` | `bool` | True if the asset is allowed for new invoices. *Note: Under the `pool_factory` model, an asset listed here should also be registered via `pool_factory::register_asset` to ensure `fund_invoice` is reachable.* | `add_supported_asset()` |

### Persistent Storage

#### Primary Invoice Entry

| DataKey | Type | Description |
|---------|------|-------------|
| `Invoice(BytesN<32>)` | `Invoice` | Full invoice record keyed by SHA-256 invoice ID |

#### Invoice Structure

```
Invoice {
    id:               BytesN<32>,       // SHA-256 hash of (issuer, buyer,
                                        //   face_value, due_date, counter, asset)
    issuer:           Address,          // Invoice creator
    buyer:            Address,          // Obligated repayer
    face_value:       u128,             // Full invoice value (stroops)
    discount_bps:     u32,              // Discount in basis points (max 5000)
    funded_amount:    u128,             // Amount actually funded (≤ face_value)
    due_date:         u64,              // Maturity timestamp
    status:           InvoiceStatus,    // Current lifecycle state
    created_at:       u64,
    listed_at:        Option<u64>,
    funded_at:        Option<u64>,
    shipped_at:       Option<u64>,
    issuer_confirmed: bool,
    buyer_confirmed:  bool,
    repaid_at:        Option<u64>,
    funding_asset:    Address,          // Token contract address
    funding_pool:     Option<Address>,  // Pool that funded this invoice
}
```

#### InvoiceStatus Enum (u32 repr)

```rust
Created   = 0   // initial state
Listed    = 1   // ready for funding
Funded    = 2   // escrow lock placed
Active    = 3   // goods shipped
Confirmed = 4   // dual delivery confirmation
Repaid    = 5   // buyer has repaid
Defaulted = 6   // past due without repayment
Expired   = 7   // listing expired before funding
Cancelled = 8   // issuer cancelled before listing
```

#### Index Entries

| DataKey | Type | Description |
|---------|------|-------------|
| `IssuerIndexCount(Address)` | `u32` | Number of invoices for this issuer |
| `IssuerIndexEntry(Address, u32)` | `BytesN<32>` | Invoice ID by issuer + index |
| `BuyerIndexCount(Address)` | `u32` | Number of invoices for this buyer |
| `BuyerIndexEntry(Address, u32)` | `BytesN<32>` | Invoice ID by buyer + index |
| `StatusIndexCount(u32)` | `u32` | Number of live slots in this status index |
| `StatusIndexEntry(u32, u32)` | `BytesN<32>` | Invoice ID by status + slot |
| `StatusPosition(InvoiceStatus, BytesN<32>)` | `u32` | O(1) slot of an invoice in its status index; presence is membership |
| `StatusCount(u32)` | `u64` | Count of invoices in each status |
| `StatusMigrationCursor(InvoiceStatus)` | `u32` | Temporary source cursor for `migrate_status_index` |
| `StatusMigrationKept(InvoiceStatus)` | `u32` | Temporary live-member counter for `migrate_status_index` |
| `StatusMembership(InvoiceStatus, u64)` | `bool` | **Legacy only.** Pre-upgrade short-id marker; read fallback, never written by new code |

#### Status Index Notes

- The status index is a **bounded membership index**, not an append-only
  history. `StatusPosition` maps `(status, full invoice id)` to the exact
  `StatusIndexEntry` slot in O(1).
- Entering a status appends one slot and writes its position. Leaving a
  status swap-and-pops: the last slot fills the vacated slot, the moved
  invoice's position is rewritten, and the departing position is deleted.
  Both operations are O(1) and never scan the status's invoice history.
- `StatusIndexCount` (slots) always equals the number of current members for
  post-upgrade data. Storage is therefore bounded by live members, not by the
  number of historical transitions.
- `get_by_status()` reads only the requested page of member slots and
  hydrates at most `page_size` invoices. The page is still filtered to the
  invoice's current status so un-migrated legacy rows cannot leak stale
  results; such a page can be short until migration completes.
- Ordering follows the compacted slot array: insertion order is preserved
  until a member leaves, at which point the previous last member moves into
  the vacated slot (swap-and-pop, as covered by
  `test_move_status_index_compacts_storage`).
- `get_by_issuer()` and `get_by_buyer()` remain append-only, paginated index
  families and are unaffected by this change.

#### Status Index Migration (upgrade note)

The old shape appended `StatusIndexEntry` rows and never reclaimed them, and
had no position map. Existing deployments should upgrade as follows:

1. Optionally `pause()` the contract while migrating so paginated reads do not
   observe a half-compacted status.
2. For each status, call
   `migrate_status_index(status, page_size)` repeatedly until it returns
   `done = true`. Each call scans at most `page_size` (≤ `MAX_PAGE_SIZE`)
   source slots, keeps only invoices whose current status matches, rewrites
   them into the compact prefix, writes their `StatusPosition`, and on the last
   page deletes stale trailing slots, sets `StatusIndexCount` /
   `StatusCount` to the live count, and clears the temporary migration keys.
3. `unpause()` and resume. The function requires admin authorization and is
   idempotent if re-run after completion.

No migration is required for correctness before then: a pre-upgrade invoice
without a position is located once by a compatibility scan on its next
transition and seeded in the map. Calling `migrate_status_index` removes that
one-time scan and any stale historical rows.

**ABI change:** `migrate_status_index(status: InvoiceStatus, page_size: u32)
-> (u32, bool)` is a new admin-authenticated entry point. Existing read
signatures (`get_by_status(status, page, page_size)`) and their result shape
are unchanged; only ordering under a departure follows the already-documented
swap-and-pop compaction.

### Storage Key Count

**Approximately 6 + (7 × number_of_invoices)** keys per active invoice:
- 5 instance keys
- 1 invoice entry
- 1 issuer index entry (+1 for count if first)
- 1 buyer index entry (+1 for count if first)
- 1 status index entry
- 1 status position (removed when the invoice leaves that status)

Transitions reuse the single status slot/position rather than adding a row per
historical status, so key count stays bounded over the invoice lifecycle.

---

## Contract: escrow_contract

### Instance Storage

| DataKey | Type | Description | Set During |
|---------|------|-------------|------------|
| `Admin` | `Address` | Contract admin | `initialize()` |
| `PoolContract` | `Address` | Authorised pool contract | `initialize()` |
| `InvoiceContract` | `Address` | Invoice contract address | `initialize()` |
| `UsdcAsset` | `Address` | USDC token contract address | `initialize()` |
| `Paused` | `bool` | Emergency circuit-breaker flag, via `clearledger_pause::PauseState::Paused` (issue #716). Absent means not paused. | `pause()` / `unpause()` |

### Persistent Storage

#### Escrow Lock Entry

| DataKey | Type | Description |
|---------|------|-------------|
| `Locked(BytesN<32>)` | `EscrowRecord` | Lock record keyed by invoice ID |

```
EscrowRecord {
    invoice_id: BytesN<32>,
    amount:     u128,      // USDC stroops
    locked_at:  u64,       // Unix timestamp
    issuer:     Address,   // Lock-time issuer; sole recipient source of truth
}
```

#### History Entries

| DataKey | Type | Description |
|---------|------|-------------|
| `HistoryCount(BytesN<32>)` | `u32` | Monotonic next append index and cursor ceiling |
| `HistoryStart(BytesN<32>)` | `u32` | First retained event index after pruning |
| `HistoryEvent(BytesN<32>, u32)` | `EscrowEvent` | Event at its stable append index |
| `History(BytesN<32>)` | `Vec<EscrowEvent>` | Legacy pre-pagination vector; migrated on append or compaction |

History is stored per event so `get_history_page(invoice_id, cursor, limit)`
hydrates only the requested slice. The append index remains stable through
pruning; a cursor older than `HistoryStart` resumes at the first retained event.
At most `MAX_HISTORY_EVENTS` (32) are retained automatically, and admins may
lower that number with `compact_history`. Pruned history remains available in
the ledger event stream for indexers. The compatibility `get_history` view
returns all currently retained entries in one response.

#### History Retention and Compaction (issue #51)

| Parameter | Value | Meaning |
|-----------|-------|---------|
| `MAX_HISTORY_EVENTS` | 32 | Automatic on-chain retention ceiling per invoice |
| `MIN_HISTORY_EVENTS` | 1 | Smallest admin-selected retention |
| `MAX_COMPACT_PER_CALL` | 32 | Maximum entries removed by one compaction call |

`compact_history(invoice_id, keep) -> u32` requires admin authorization and
removes oldest indexed entries until `keep` remain or the per-call removal cap
is reached. Repeated calls are safe. Original ledger events are preserved for
indexers; see [ESCROW_HISTORY.md](./ESCROW_HISTORY.md).

**Approximately 2 + N persistent keys per invoice, or 3 + N after pruning** — one `Locked` record, `HistoryCount`, one `HistoryEvent` per retained event (`N`), and `HistoryStart` once pruning begins. Legacy `History` vectors are removed when migrated. History entries receive the same TTL extension policy as the lock.

#### Lifecycle

1. `lock()` validates that `amount` is in `1..=i128::MAX` and that no
   `Locked` record already exists for the invoice **before** transferring
   USDC. Invalid amounts fail with `InvalidAmount`; a duplicate invoice fails
   with `AlreadyLocked` and leaves the original `amount`/`issuer` unchanged.
   On success it creates an `EscrowRecord` and transfers USDC from pool → escrow.
2. `release_to_issuer()` or `release_to_pool()` transfers USDC out and
   **removes** the `Locked` key (caller must re-lock for a new funding cycle).
   `release_to_issuer()` treats `EscrowRecord.issuer` as the source of truth:
   a caller-supplied address must equal it, and the transfer always goes to the
   stored value.
3. `handle_default()` returns locked USDC to the pool and **removes** the key.
4. On removal, the TTL-granted entries also expire organically.


---

## Contract: pool_contract

### Instance Storage

| DataKey | Type | Description | Set During |
|---------|------|-------------|------------|
| `Admin` | `Address` | Contract admin | `initialize()` |
| `InvoiceContract` | `Address` | Invoice contract address | `initialize()` |
| `EscrowContract` | `Address` | Escrow contract address | `initialize()` |
| `FundingAsset` | `Address` | Asset this pool instance funds invoices with | `initialize()` |
| `TotalShares` | `u128` | Total LP shares outstanding | `initialize() = 0` |
| `TotalDeposits` | `u128` | Total USDC principal deposited | `initialize() = 0` |
| `TotalFunded` | `u128` | Total USDC deployed to invoices | `initialize() = 0` |
| `TotalYieldDistributed` | `u128` | Cumulative yield distributed | `initialize() = 0` |
| `ActiveInvoiceCount` | `u32` | Currently funded invoices | `initialize() = 0` |
| `MaxUtilizationBps` | `u32` | Max utilization cap (bps) | `initialize() = 8500` |
| `ProtocolFeeBps` | `u32` | Protocol fee in basis points (max 2000 bps = 20%) | `initialize() = 0` |
| `TreasuryAddress` | `Address` | Protocol treasury destination for fee cuts | `initialize() = treasury (may equal admin)` |
| `MinInitialDeposit` | `u128` | Minimum first deposit an empty pool accepts, in `FundingAsset` stroops | `initialize()` |
| `ShareName` | `String` | SEP-41 `name()` of this pool's LP share token | `initialize()` |
| `ShareSymbol` | `String` | SEP-41 `symbol()` of this pool's LP share token | `initialize()` |
| `ShareDecimals` | `u32` | SEP-41 `decimals()` of this pool's LP share token (falls back to 7 when unset) | `initialize()` |

### Persistent Storage

#### LP Position Keys

| DataKey | Type | Description |
|---------|------|-------------|
| `LPShares(Address)` | `u128` | Share balance per LP address |
| `LPDepositCount(Address)` | `u32` | Number of deposits made by this LP |
| `LPYieldEarned(Address)` | `u128` | Cumulative yield earned (updated on withdraw) |
| `LPInitialDeposit(Address)` | `u128` | Total principal deposited by LP (tracked for yield calculation) |

#### SEP-41 Share Transfers

See [SEP-41 LP Share Token: Design and Behavior](SEP41_DESIGN.md) for the full
method table, authorization matrix, allowance expiry semantics, and known
deviations.

LP shares move between addresses through two public entry points that share
one balance-movement path (`move_shares`) and emit the same standard
`transfer(from, to, amount)` event:

- `transfer(from, to, amount)` — the standard SEP-41 entry point. Generic
  `soroban_sdk::token::Client` consumers call this directly against the pool
  address.
- `transfer_from(spender, from, to, amount)` — moves shares against an
  `approve` grant, debiting the allowance.
- `transfer_shares(from, to, amount)` — **non-standard** legacy alias of
  `transfer`, kept for integrators that predate the SEP-41 surface. New code
  must use `transfer`.

Transfers only move balances: `TotalShares`, `TotalDeposits`, `TotalFunded`,
yield accounting, and share price are untouched. Self-transfers are no-ops.
The recipient's `LPShares` entry is created on first credit, exactly as a
deposit would create it.

#### SEP-41 Allowance Keys

| DataKey | Type | Description |
|---------|------|-------------|
| `Allowance(Address, Address)` | `ShareAllowance { amount: i128, expiration_ledger: u32 }` | Shares `from` let `spender` move, until `expiration_ledger`. Removed when fully spent or revoked with `approve(..., amount = 0)`; read back as `0` once expired. |

#### Funded Invoice

| DataKey | Type | Description |
|---------|------|-------------|
| `FundedInvoice(BytesN<32>)` | `u128` | Funded amount keyed by invoice ID |

### Protocol Fee & Yield Distribution Math

When repayments occur via `receive_repayment` or `receive_repayment_with_refund`, gross yield is computed as:
$$\text{yield\_amount} = \text{amount} - \text{funded\_amount} - \text{refund}$$

With a configured protocol fee (`fee_bps > 0`):
$$\text{protocol\_cut} = \frac{\text{yield\_amount} \times \text{fee\_bps}}{10\,000}$$
$$\text{lp\_yield} = \text{yield\_amount} - \text{protocol\_cut}$$

- **Treasury Transfer:** If `protocol_cut > 0`, the pool transfers `protocol_cut` USDC directly to `TreasuryAddress`. At the default `fee_bps == 0`, no transfer is made.
- **LP Accounting:** Only `lp_yield` (not the full gross yield) is added to `TotalDeposits` and `TotalYieldDistributed`.
- **Admin Configuration:** The fee defaults to 0 bps at initialization and can be modified up to 2000 bps (20%) along with the treasury address by the contract admin via `set_protocol_fee(fee_bps, treasury)`.

### Storage Key Count

**Approximately 8 instance + (5 × number_of_LPs) + (1 × number_of_funded_invoices).**

In practice most pools will have:
- 8 instance keys (constant)
- 5 keys per active LP
- 1 key per funded invoice (removed on repayment/default)

---

## Contract: pool_factory

### Instance Storage

| DataKey | Type | Description | Set During |
|---------|------|-------------|------------|
| `Admin` | `Address` | Contract admin | `initialize()` |
| `AssetCount` | `u32` | Number of registered assets | `register_asset()` / `register_existing_pool()` |
| `AssetIndex(u32)` | `Address` | Ordered index of registered assets | `register_asset()` / `register_existing_pool()` |
| `PoolForAsset(Address)` | `Address` | The pool contract deployed/tracked for an asset | `register_asset()` / `register_existing_pool()` |

### Storage Key Count

**Approximately 2 + (2 × number_of_assets).**

Each registered asset requires two keys: one for its position in the list (`AssetIndex`) and one mapping its address to a pool (`PoolForAsset`).

### Asset Enumeration and Pagination

`AssetIndex` is append-only and ordered by registration. Both registration
paths write the asset's pool mapping, append the asset at index `AssetCount`,
and then increment `AssetCount`, so a position never moves or gets reused.

Read the list with `get_assets_page(cursor, limit)`, which returns
`AssetPage { items: Vec<Address>, next_cursor: Option<u32> }`. `limit` must be
in `1..=50` (`MAX_ASSET_PAGE_LIMIT`), otherwise the call panics with
`InvalidLimit` (error 10); a cursor greater than `AssetCount` panics with
`InvalidCursor` (error 11). Start at cursor `0` and follow `next_cursor` until
it is `None`. A cursor equal to `AssetCount` returns an empty final page.

Because the index is append-only, a registration between two page reads is
returned at the end of the walk and cannot shift or duplicate an already-read
asset. The legacy unbounded `list_assets()` view is retained for ABI
compatibility and uses the same order, but new integrations should page.


## Gas / Budget Estimates

> **Note:** These are approximate and will vary with network conditions.
> Update after mainnet deployment.

| Operation | Read Entries | Write Entries | Relative Cost |
|-----------|-------------|---------------|---------------|
| `registry::register_issuer` | 1 | 2 (+ TTL) | Low |
| `invoice::create` | 5+ | 6+ (+ TTL entries) | Medium |
| `invoice::repay` | 3 | 3 (+ TTL + token transfer) | Medium-High |
| `pool::deposit` | 5 | 5 (+ TTL + token transfer) | Medium |
| `pool::fund_invoice` | 6+ | 5 (+ TTL + cross-contract calls) | High |
| `escrow::lock` | 2 | 2 (+ TTL + token transfer) | Medium |
| `escrow::compact_history` | 1 | 1 (+ TTL), at most `MAX_COMPACT_PER_CALL` evictions | Low-Medium |

See [LIMITATIONS.md](./LIMITATIONS.md) for current testnet budget constraints.

---

## Upgrade Path

The invoice and escrow contracts expose an admin-gated `upgrade(new_wasm_hash)`
entry point. First install the new Wasm on the network, then call `upgrade`
with its hash from the contract's stored admin address. The contract is updated
in place, so its address and storage remain unchanged. Other contracts still
require a redeploy if they do not expose this entry point. Protect the admin
with a multisig as described in [DEPLOYMENT.md](../DEPLOYMENT.md). See that
document for the redeployment rollback procedure.
