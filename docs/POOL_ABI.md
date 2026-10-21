# Pool ABI Notes

This document records ABI-facing behavior that integrators need beyond the
generated Soroban contract spec. Contract error codes live in
[`contracts/pool/src/errors.rs`](../contracts/pool/src/errors.rs); they are
never renumbered.

## `preview_fund(invoice_id) -> FundingPreview`

Read-only quote for funding a listed invoice through a pool instance. It is the
preview counterpart of `fund_invoice` (and of each entry in
`batch_fund_invoice`) and runs the same checks in the same order:

1. `DataKey::FundedInvoice(invoice_id)` must not exist (`AlreadyFunded`).
2. The invoice must be `Listed` (`InvoiceNotListed`).
3. Issuer and buyer must still be verified in the registry
   (`IssuerNotVerified` / `BuyerNotVerified`).
4. The invoice funding asset must equal the pool's funding asset
   (`AssetMismatch`).
5. `funded_amount = face_value * (10_000 - discount_bps) / 10_000` must be
   non-zero (`InvalidAmount`) and must not overflow `u128` (`Overflow`).
6. `funded_amount` must not exceed available liquidity
   (`InsufficientLiquidity`).
7. Post-funding utilization must not exceed `max_utilization_bps`
   (`UtilizationCapExceeded`).

The discount multiplication, floor division, utilization calculation, and
`u128` overflow checks are shared with the write path (`evaluate_funding`), so
the quote cannot drift from what `fund_invoice` commits.

### `FundingPreview`

| Field | Meaning |
| --- | --- |
| `funded_amount` | Funding-asset base units `fund_invoice` would commit, using the same floor-rounded discount math. Zero when the quote stopped at `InvalidAmount`, `Overflow`, or an earlier eligibility failure. |
| `required_capacity` | Pool liquidity the funding would consume; equal to `funded_amount`. |
| `available_liquidity` | Current `total_deposits - total_funded`. Zero when an earlier eligibility failure stopped the quote before pool totals were read. |
| `utilization_bps_after` | Utilization after funding, in basis points. Zero when an earlier eligibility, liquidity, or overflow check stopped the quote. |
| `max_utilization_bps` | The pool's configured cap. Zero when an earlier eligibility failure stopped the quote before pool totals were read. |
| `eligible` | `true` exactly when `status == FundingStatus::Eligible`. |
| `status` | First failing check, or `Eligible`. |

### `FundingStatus`

`FundingStatus` is a contract type union with the variants `Eligible`,
`AlreadyFunded`, `InvoiceNotListed`, `IssuerNotVerified`, `BuyerNotVerified`,
`AssetMismatch`, `InvalidAmount`, `InsufficientLiquidity`,
`UtilizationCapExceeded`, and `Overflow`. Each non-`Eligible` variant describes
the same condition as the same-named `PoolError`, which is what a real funding
transaction would raise.

### Auth, state, and TTL

`preview_fund` requires no authorization. It performs only storage reads and
cross-contract view calls (`get_funding_terms`, `get_issuer`, `get_buyer`,
`get_funding_asset`, and registry `is_verified`). It writes no storage, emits no
events, and extends no TTL — including the pool's instance entry. It does not
check pause state, so callers that intend to submit `fund_invoice` should also
confirm the pool is unpaused.

The method panics only with `NotInitialized` when the pool has not been
initialized, or with the invoice contract's lookup error when `invoice_id` does
not exist. All predictable eligibility and capacity failures are returned as
`FundingStatus` values instead of reverting.

### Example

```rust
let preview = pool.preview_fund(&invoice_id);
if preview.eligible {
    // preview.funded_amount equals what fund_invoice(&invoice_id) would commit.
}
```
