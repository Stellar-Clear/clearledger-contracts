# Event versioning: initial funding event migration

Existing event topics and payloads remain unchanged. The invoice and pool
funding paths now additionally emit:

| Topic | Indexed ID | Data |
| --- | --- | --- |
| `invoice_funded` (legacy) | invoice ID | funded amount (`u128`) |
| `invoice_funded_v1` | invoice ID | `(1u32, funded_amount: u128)` |

Indexers must select **one** format and filter by emitting contract address.
Both the invoice and pool emit funding events today; the versioned companion
preserves that behavior and does not indicate a second funding transfer.
Do not sum legacy and versioned events, and do not count the invoice and pool
observations as separate payments. The version is an explicit integer so a
new decoder can reject unsupported versions before reading the payload.

The companion is emitted in the same transaction as the legacy event. A reverted
transaction does not commit either event. No historical event data is rewritten.
Focused helper tests assert both topic shapes, emitter addresses and payloads.
The versioned helper is called before the unchanged legacy helper, keeping the
legacy funding event last for existing single-event readers and preserving direct
legacy emitter XDR fixtures. Consumers relying on an exact event count must accept
the added companion event;
transaction resource usage also increases by one event per helper invocation.

## Remaining scope

This focused initial change versions the funding event shared by two contracts.
Other legacy events are **not** migrated here. For a future incompatible payload
use a new versioned topic and retain the legacy publisher during a documented
consumer transition; never silently change a legacy payload under its old topic.
