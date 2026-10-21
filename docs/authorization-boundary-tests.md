# Cross-contract authorization regression coverage

The `authorization_boundary` integration test initializes registry, invoice,
escrow and pool under setup-only mock authorization, then clears all mocked
credentials before attempting privileged calls. It checks the host **Auth**
error category, so a later validation failure cannot masquerade as an auth check.

Coverage includes missing account credentials for pause/unpause on all four
contracts, unrelated-contract relay calls to those entry points, and direct and
relayed upgrades for the two contracts that expose upgrades (invoice and escrow).
The upgrade hash is deliberately uninstalled: rejection must happen at auth,
before deployment. Admin identities remain unchanged after rejected calls.

Run `cargo test -p clearledger-integration-tests --test authorization_boundary`.
These tests use native testutils and synthetic addresses; no Testnet/Mainnet
transactions or real wallet credentials are involved. This focused suite does not
replace per-entry-point business logic tests or real signed transaction tests.
