# Invoice status-index storage and pagination

The current status index uses membership markers for idempotent updates and
swap-and-pop removal when an invoice leaves a status. A repeated transition to
the already-current status does not append another row. Moving between statuses
removes the old array row and old membership marker, so the nine status variants
do not accumulate nine historical rows per invoice: normally one current-status
row and one membership marker remain. Legacy inconsistent rows are not migrated
by this change.

`StatusIndexCount` is the current array length, not a historical transition count.
Removal swaps the final row into the removed position, then deletes the trailing
row. No separate public compaction endpoint is necessary.

## Pagination consistency

`get_by_status(status, page, page_size)` uses zero-based offsets, not stable
cursors. With no state changes, successive pages preserve the current array
order. A concurrent transition can reorder rows through swap-and-pop; clients
should restart pagination and deduplicate by invoice ID when refreshing after a
transition. Do not persist an offset as a durable cursor or assume insertion
order. Start-at-end and overflowing offsets return empty pages.

The regression tests exercise repeated and replayed private-helper transitions,
check that old slots disappear and counts remain bounded, and verify page
boundaries after a swap. They preserve the existing compaction implementation
rather than introducing a competing append-only layout.
