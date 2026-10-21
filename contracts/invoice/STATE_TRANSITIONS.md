# Invoice Entry-Point Reference

This table follows the current implementation in `src/lib.rs` and event
definitions in `src/events.rs`. All token amounts are USDC stroops. A dash means
the invoice contract itself emits no event or transfers no tokens. Funding and
repayment token movements often happen in Pool/Escrow cross-contract calls, not
in the state-writing invoice entry point.

## Lifecycle Entry Points

| Entry point | Authorization | Allowed prior state -> result | Events | Token movement |
|---|---|---|---|---|
| `create` | Issuer | No invoice -> `Created` | `invoice_created` | None |
| `batch_create` | Issuer once for the batch | Each valid entry: no invoice -> `Created`; atomic on any invalid entry | Per-invoice `invoice_created`, then `batch_invoices_created` | None |
| `submit_attestation` | Permissionless caller; valid signature from active registered agent | Existing invoice; status unchanged; one attestation per invoice | `attestation_submitted` | None |
| `list_for_financing` | Invoice issuer | `Created` -> `Listed`; requires unexpired attestation and current registry verification | `invoice_listed` | None |
| `batch_list_for_financing` | Issuer of each successfully listed invoice, once per distinct issuer | Each eligible `Created` invoice -> `Listed`; individual failures are skipped | Per-invoice `invoice_listed`, then `batch_invoices_listed` | None |
| `cancel` | Invoice issuer | `Created` -> `Cancelled` | `invoice_cancelled` | None |
| `mark_funded` | Configured Pool contract | `Listed` -> `Funded` | `invoice_funded` | None in this method; Pool funding locks funds in Escrow and advances proceeds to issuer |
| `mark_shipped` | Invoice issuer | `Funded` -> `Active` | `invoice_shipped` | None |
| `confirm_delivery` | Issuer or buyer named as confirmer | `Active` -> `Active` after first confirmation; `Active` -> `Confirmed` after both | `delivery_confirmed`; also `both_confirmed` when the second party confirms (emitted first) | None |
| `repay_partial` | Invoice buyer | `Funded`, `Active`, or `Confirmed` stays there until balance reaches zero; then -> `Repaid` | `partial_repayment_received` until final payment; then `invoice_repaid` | Buyer -> Escrow per installment; on final payment Escrow -> Pool, and Pool accounts for yield/refund/fee |
| `repay` | Invoice buyer through `repay_partial` | Same states and result as `repay_partial`, settling the full remaining balance | Same as `repay_partial` | Same as `repay_partial` |
| `repay_early` | Invoice buyer | `Confirmed` before due date -> `Repaid` | `invoice_repaid` | Buyer -> Escrow -> Pool; Pool returns the unearned discount to buyer and splits earned yield |
| `trigger_default` | Permissionless; due date passed and Escrow grace period satisfied | `Funded`, `Active`, or `Confirmed` -> `Defaulted` | `invoice_defaulted` when this call performs the transition | Pool/Escrow default handling returns recoverable locked funds to Pool |
| `mark_defaulted` | Invoice's configured funding Pool | `Funded`, `Active`, or `Confirmed` -> `Defaulted`; already `Defaulted` is an idempotent no-op | `invoice_defaulted` on transition | None in this method; called by Pool default handling |
| `expire_listing` | Stored issuer or admin, supplied as `caller` | `Listed` after the expiry window -> `Expired` | `invoice_expired` | None |

## Configuration And Administration

| Entry point(s) | Authorization | State effect | Events | Token movement |
|---|---|---|---|---|
| `initialize` | Initial admin | Initializes admin, registry, counter | `contract_initialized` | None |
| `set_pool_contract`, `set_escrow_contract`, `set_agent_registry_contract` | Admin | Replaces configured contract address | Corresponding `*_contract_updated` event | None |
| `add_supported_asset`, `remove_supported_asset` | Admin | Adds/removes an asset accepted for new invoices | `supported_asset_added` / `supported_asset_removed` | None |
| `set_expiry_window` | Admin | Sets listing expiry window (maximum one year) | `expiry_window_set` | None |
| `transfer_ownership` | Current and incoming admin | Replaces admin | `ownership_transferred` | None |
| `pause`, `unpause` | Admin | Toggles state-changing-call circuit breaker | `paused`, `unpaused` | None |
| `upgrade` | Admin | Replaces current contract Wasm | `upgraded` | None |

## Read-Only Entry Points

Every method below is permissionless, valid regardless of invoice status, does
not change state, emits no event, and moves no tokens. Methods requiring an
invoice ID return or panic according to whether that invoice exists.

| Entry point(s) | Read result |
|---|---|
| `get_admin`, `get_registry_contract`, `get_pool_contract`, `get_agent_registry_contract`, `get_escrow_contract` | Contract administration/wiring addresses |
| `get_attestation` | Optional stored attestation for an invoice |
| `is_supported_asset`, `get_supported_asset_count` | Asset allowlist membership/count |
| `get_expiry_window` | Current listing-expiry window, or default |
| `get_status`, `has_status_membership` | Invoice status and status-index membership |
| `get_face_value`, `get_remaining_balance`, `get_repaid_amount`, `get_discount_bps`, `get_funding_terms`, `get_funding_asset` | Individual invoice financing/repayment values |
| `get` | Full invoice record |
| `get_by_status`, `get_by_issuer`, `get_by_buyer` | Paginated invoice records |
| `get_invoice_count_by_issuer`, `get_invoice_count_by_buyer`, `get_counts` | Invoice counts |
| `get_issuer`, `get_buyer`, `get_due_date` | Invoice participants and due date |

## Attestation Wire Format

`AttestationPayload` is XDR-encoded and the exact encoded bytes are hashed and
signed. Its field order is `domain_separator`, `invoice_id`, `risk_score`,
`evidence_hash`, `agent_id`, `nonce`, `expires_at`. `expires_at` is Unix seconds;
submission requires it to be strictly later than ledger time, and listing is
rejected once it is reached. Integrators must include this field when signing
new payloads. Already-stored attestations from before this field was introduced
have no expiry key and retain their previous listing behavior.