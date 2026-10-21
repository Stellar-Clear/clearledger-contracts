# ADR 0002: Isolate Pool Accounting Per Funding Asset

- **Status:** Accepted
- **Date:** 2026-10-19

## Context

Pooling balances from different assets would require exchange-rate valuation,
conversion, and cross-asset loss and liquidity rules. The contracts instead
use the invoice's funding-asset address to select a pool, while each pool
instance stores its own token address, share supply, liquidity, risk settings,
treasury, and funded-invoice state.

## Decision

Maintain one independent pool instance per funding asset. A pool accepts and
returns only its configured token; it does not convert, net, or cross-collateralize
balances with another pool. The invoice contract allow-list and the factory's
asset-to-pool mapping are separate controls and both must be configured for an
asset to be usable end to end. Each pool must use an escrow initialized for the
same asset.

The supported deployment workflow is USDC on Testnet. XLM deployment is
experimental and opt-in; its presence in the scripts is not a claim that it is
production-supported. Contract amounts are token base units, and onboarding
must verify the asset contract interface and precision assumptions rather than
assuming all assets use USDC's seven decimal places.

## Consequences

- Liquidity, utilization limits, fees, and losses are isolated by asset.
- Cross-asset price discovery, swaps, and netting are out of scope.
- Indexers and applications must select the pool using the invoice's funding
  asset rather than treating one pool as protocol-wide.
- New asset support requires an allow-list entry, a matching escrow, a factory
  pool registration, and integration tests for the asset's contract behavior
  and amount precision.
- No deployment or configuration described here implies Mainnet readiness.

## References

- `contracts/pool/src/lib.rs`
- `contracts/pool_factory/src/lib.rs`
- `contracts/invoice/src/lib.rs`
- [Deployment guide](../../DEPLOYMENT.md)
- [Known limitations](../LIMITATIONS.md)