# ADR 0001: Use a Factory to Register Asset Pools

- **Status:** Accepted
- **Date:** 2026-10-19

## Context

Each pool is initialized for one funding asset and references one invoice
contract and one escrow contract. Deploying pools manually leaves asset-to-pool
discovery and duplicate deployment prevention to operators. The invoice
contract separately maintains an allow-list of funding assets, so that list
alone does not prove a usable pool exists for an asset.

## Decision

Use `pool_factory` as the registry and deployment boundary for asset pools.
`register_asset` deploys and initializes a pool deterministically from the asset
address, takes the factory admin as the pool admin, and records the mapping only
after initialization succeeds. An asset can be registered only once. For
migration, `register_existing_pool` accepts an already initialized pool only
when its configured funding asset matches. Read-only lookup uses
`get_pool_for_asset`; aggregate stats query the registered instances.

The factory derives the registry from the invoice contract rather than accepting
a second registry address, keeping invoice and pool verification aligned.
Invoices still require explicit allow-listing in the invoice contract, and each
asset must have a corresponding registered pool and correctly configured
escrow.

## Consequences

- Asset lookup and duplicate prevention have one canonical owner.
- Pool addresses are deterministic for a given factory and asset, independent
  of registration order.
- The factory admin controls new pool deployment and becomes the new pool's
  admin and initial treasury; admin key management remains a trust assumption.
- Registration is not a pool-upgrade or replacement mechanism. A registered
  asset cannot be registered again through either path.
- Adding an invoice asset without deploying/registering its pool and matching
  escrow can leave invoices that cannot be funded.

## References

- `contracts/pool_factory/src/lib.rs`
- `contracts/invoice/src/lib.rs` (`add_supported_asset` and pool routing)
- [Pool factory migration guide](../POOL_FACTORY_MIGRATION.md)