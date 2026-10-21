# Ledger renewal boundaries

The `ledger_renewal` integration suite advances fresh native test environments to
one ledger above, exactly at, and one ledger below `TTL_THRESHOLD`. It invokes
pause on registry, invoice, escrow and pool and checks the instance TTL is renewed
at/below the threshold, while entries above it keep their remaining lifetime.

Persistent records for a profile, invoice, escrow lock and LP shares are seeded
with valid shapes independently of financing/token workflows. Public views must
still return the stored values at every boundary. Registry profile and LP-position
views renew their persistent records; invoice and escrow getters currently do not.
The test records those existing policies without introducing a runtime change.

Run `cargo test -p clearledger-integration-tests --test ledger_renewal`.
Ledger sequence changes do not imply timestamp changes. No network deployment or
real-value assets are used. This suite covers renewal while entries are live;
archived-state restoration requires separate coverage and RPC orchestration.
