# Pool factory asset policy

Pool creation is an administrator-controlled operation. The factory requires the
initialized admin to authorize every `register_asset` and
`register_existing_pool` call, and it records one immutable pool per asset. A
caller without admin authorization cannot add an asset, while a duplicate is
rejected before the mapping or asset index changes.

The registration tests in `contracts/pool_factory/src/test.rs` cover unauthorized
registration, duplicate registration, invalid existing pools, escrow asset
mismatches, and preservation of lookup mappings after rejected requests. These
checks form the allowlist boundary for the current deployment model.
