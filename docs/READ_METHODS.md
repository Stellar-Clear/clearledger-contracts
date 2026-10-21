# Public contract reads and cost expectations

This inventory describes the current native contract implementations, not a
measured RPC latency or fee guarantee. “Bounded” refers to storage operations or
loop bounds; serialized values and host execution still consume resources.
Sources: `contracts/{registry,invoice,pool,escrow,pool_factory}/src/lib.rs`.

## Registry

| Methods | Storage / cost expectation |
| --- | --- |
| `get_admin` | One instance lookup; also renews instance TTL. |
| `get_profile`, `is_verified`, `get_verification_status` | Profile-key lookups; response size follows metadata. Existing entries may renew persistent TTL. Missing-profile behavior differs: `get_profile` rejects, verification views can report unregistered/false. |
| `get_profile_count` | Bounded role count lookup; does not enumerate profiles. |
| `list_profiles(role, start, limit)` | At most `limit` index slots, capped at 50. Missing/expired slots are skipped; returned length can be smaller than the scanned window. May renew count/index TTL. |

For registry pagination advance `start` by the requested limit, not the number of
returned addresses. A short page can contain index gaps. Keep role and ledger
context consistent and stop at the role count boundary; refresh counts when
restarting after registrations or other mutations.

## Invoice

| Methods | Storage / cost expectation |
| --- | --- |
| `get_admin`, `get_registry_contract`, `get_pool_contract`, `get_agent_registry_contract`, `get_escrow_contract`, `get_expiry_window`, `get_supported_asset_count` | Bounded instance/config lookups. Optional wiring can be absent; do not assume every deployment is fully configured. |
| `get_attestation`, `is_supported_asset` | Bounded keyed persistent lookups. |
| `get`, `get_status`, `get_face_value`, `get_remaining_balance`, `get_repaid_amount`, `get_discount_bps`, `get_funding_terms`, `get_funding_asset`, `get_issuer`, `get_buyer`, `get_due_date` | Bounded invoice-record lookup/projection; missing IDs reject. Multiple field queries may reread the same record, so prefer `get` when several fields are required. |
| `get_invoice_count_by_issuer`, `get_invoice_count_by_buyer` | Bounded index count lookups. |
| `get_counts` | Fixed set of status counters; no full invoice scan. |
| `has_status_membership` | Membership helper; see implementation's legacy-index fallback before assuming constant cost for older state. |
| `get_by_status`, `get_by_issuer`, `get_by_buyer` | Requested index window plus record hydration, capped by `MAX_PAGE_SIZE` (50). Status results additionally filter current status. Oversized page sizes reject; overflowing/out-of-range offsets return empty pages. |

`list_for_financing` is **a mutation**, not a read despite its name. It must not
be used as an enumeration API. Batch and status mutation costs are separate from
these read bounds. Status pages use offsets and swap-and-pop compaction, not a
stable cursor: transitions can reorder rows. Restart/deduplicate by invoice ID
when refreshing across mutations. Neither page size nor a constant number of
storage accesses guarantees a transaction fits a network resource limit.

## Pool

| Methods | Storage / cost expectation |
| --- | --- |
| `get_funding_asset`, legacy `get_usdc_asset`, `get_admin`, `get_fee_bps`, `get_treasury_address`, `get_invoice_contract`, `get_escrow_contract`, `get_protocol_fee_bps`, `get_treasury` | Bounded configuration lookups. Legacy aliases are not extra asset identities. |
| `name`, `symbol`, `decimals` | Bounded metadata lookups; name/symbol response size follows configured strings. These describe pool shares, not the funding asset. |
| `balance`, `allowance` | Bounded share/allowance lookups. Allowance semantics depend on the current ledger sequence and expiration. |
| `preview_deposit` | Bounded totals reads and checked arithmetic; a quote is not an execution guarantee. |
| `get_stats`, `get_utilization_rate`, `get_suggested_discount_bps` | Bounded totals/configuration reads and calculations; no all-invoice or all-LP traversal. |
| `get_lp_position` | Bounded LP keys plus totals, including TTL renewal for existing LP records. |
| `get_funded_amount` | Bounded invoice-keyed lookup; an absent entry can return `None`. |

Pool amounts and share amounts are different accounting quantities. Keep integer
precision explicit, inspect token/share decimals, and avoid floating-point
rounding in clients. No read predicts guaranteed yield or financing availability.

## Escrow and factory

| Contract / methods | Storage / cost expectation |
| --- | --- |
| Escrow `get_usdc_asset`, `get_admin`, `get_pool_contract` | Bounded instance configuration reads. |
| Escrow `get_locked`, `get_locked_at` | Bounded record lookups, returning zero for absent locks. |
| Escrow `get_history` | Returns the entire per-invoice history vector. **Not paginated**; cost and response size grow with history. |
| Factory `get_pool_for_asset` | Bounded asset mapping lookup. |
| Factory `list_assets` | Enumerates the entire asset index; **unbounded by a caller-supplied page size**. |
| Factory `get_aggregate_stats` | Enumerates assets and invokes each available pool's `get_stats`; cost scales with configured assets and cross-contract calls. Failed pool calls are skipped, so the result is not a proof every pool was healthy. |

Avoid calling the unpaginated factory/history views as a polling primitive at
large scale. Use an event-backed indexer for incremental history and maintain a
verified list of pool IDs when per-pool polling is sufficient. The current API
has no hidden pagination parameter for these methods; do not invent one.

## Stellar RPC client guidance

Use Testnet explicitly for development; contract IDs are network-specific
C-addresses and user accounts can be G-addresses or contract-auth C-addresses.
Use RPC simulation for read planning. Some views renew TTL, so “read” does not
necessarily mean a simulation has no ledger writes. The installed Stellar CLI
supports `contract invoke --send=no` to return simulation results without
submitting; confirm support with your pinned CLI's help before automating it.

Choose small pages (for example 10) and increase only after observing simulation
resource use. Do not translate the implementation's 50-entry cap into a claim
that a 50-entry request is always safe. Handle rejected simulation, missing or
archived storage, network resets and ledger changes explicitly. Pin requests to
a consistent observation period where supported, record ledger identifiers and
invalidate cached pages after relevant events. Ledger sequence controls allowance
expiry/storage lifetime; Unix timestamp controls business deadlines. They are
not interchangeable clocks.

No CPU/memory baseline was generated for this documentation. Quantitative
limits require simulation of the deployed WASM on the target network and fixture
sizes, not estimates from native unit-test timing.
