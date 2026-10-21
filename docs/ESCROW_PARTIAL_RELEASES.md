# Partial escrow releases to the pool

`release_to_pool(invoice_id, repayment_amount)` retains the lock when the
release is smaller than its outstanding amount. `get_locked` then reports the
residual. The issuer and original `locked_at` remain unchanged, so default
recovery does not restart its grace period. History and the existing release
event record each transfer's actual amount. Authorization and pause checks are
unchanged; zero amounts remain invalid.

For a sequence of releases no larger than the outstanding lock:

`sum(released amounts) + get_locked(invoice_id) == initial lock`

A release equal to the residual closes the lock. Existing settlement behavior
for releases above the residual is retained: the configured pool may settle an
amount including buyer-funded yield above the original advance, provided the
asset transfer succeeds. This change does not separately attribute externally
deposited buyer repayments, reconcile pooled escrow balances, or change the
invoice/pool repayment workflow. The conservation property applies to partial
releases of the original lock, not to additional tokens deposited externally.

The lock record and ABI retain their existing shape. Records already deleted
by older partial releases cannot be reconstructed by this patch.
