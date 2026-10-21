# Pool sequence conservation: initial coverage

`prop_lp_operation_sequences_conserve_shares_and_deposits` generates 10
shrinkable cases of 1–19 operations over two LPs: deposits, share transfers,
withdrawals and admin fee configuration. After **every prefix**, it verifies
that participant share balances sum to supply, deposits match the reference
model, available liquidity matches deposits, pool token balance equals the
unfunded deposit book, and participant/pool token balances conserve the initial
sum. It also checks malformed negative transfers fail without balance/supply
changes after each prefix. The existing utilization/funding state machine now
also checks share-supply conservation while randomizing deposits, funding,
repayment, defaults, withdrawals, share transfers, utilization-cap updates and
fee updates. No new contract interface or behavior is introduced.

## Trust boundary and remaining scope

Both properties use the existing initialized pool fixture, mocked authorized
callers and a mock token. The unfunded reference model keeps shares at par;
the settlement model exercises yield and default accounting. Changing the
protocol fee must not itself move tokens or change share supply. Randomized
real-token failure and unauthorized-caller sequences remain outside this
coverage; targeted negative-transfer checks and existing authorization tests
cover those boundaries. Run `cargo test -p clearledger-pool` and
`cargo fmt --all --check` before merging.
