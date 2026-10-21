# Pre-Mainnet Independent Audit Scope

> **Status:** Preparation inventory only. This is not an independent audit, security certification, or statement of Mainnet readiness. The contracts are currently intended for Stellar Testnet; do not use real-value assets.
>
> **Reviewed against:** repository source and documentation on 2026-10-19.

## Audit Objectives

An independent reviewer should assess the contract system end to end, including authorization, state transitions, asset movement, persistent storage, cross-contract calls, and deployment assumptions. Findings should identify the affected contract and entrypoint, preconditions, impact, and a reproducible test or transaction sequence.

## Code and Test Map

| Area | Primary files | Review focus |
|---|---|---|
| Registry and identity | [`contracts/registry/src/lib.rs`](../contracts/registry/src/lib.rs), [`contracts/registry/src/errors.rs`](../contracts/registry/src/errors.rs) | Registration consent, admin verification/revocation, metadata bounds, enumeration indexes, and pause behavior |
| Invoice lifecycle | [`contracts/invoice/src/lib.rs`](../contracts/invoice/src/lib.rs), [`contracts/invoice/src/types.rs`](../contracts/invoice/src/types.rs), [`contracts/invoice/src/errors.rs`](../contracts/invoice/src/errors.rs) | Attestation, role checks, supported assets, listing/funding/shipping/repayment/default transitions, pagination, and replay resistance |
| Pool accounting | [`contracts/pool/src/lib.rs`](../contracts/pool/src/lib.rs), [`contracts/pool/src/types.rs`](../contracts/pool/src/types.rs), [`contracts/pool/src/errors.rs`](../contracts/pool/src/errors.rs) | Share pricing, deposits/withdrawals, utilization, yield/fees, repayment/default accounting, token transfers, and authorization |
| Escrow | [`contracts/escrow/src/lib.rs`](../contracts/escrow/src/lib.rs), [`contracts/escrow/src/types.rs`](../contracts/escrow/src/types.rs) | Lock/release invariants, invoice uniqueness, recipient selection, default handling, and balance conservation |
| Pool factory | [`contracts/pool_factory/src/lib.rs`](../contracts/pool_factory/src/lib.rs) | Factory authority, deployed instance configuration, registry/asset references, and duplicate creation behavior |
| Shared controls | [`contracts/pause/src/lib.rs`](../contracts/pause/src/lib.rs), [`contracts/ttl/src/lib.rs`](../contracts/ttl/src/lib.rs) | Pause enforcement on every mutating entrypoint and storage extension thresholds across instance/persistent entries |
| Cross-contract behavior | [`contracts/test-utils/src/lib.rs`](../contracts/test-utils/src/lib.rs), [`tests/integration/tests/lifecycle.rs`](../tests/integration/tests/lifecycle.rs) | Call ordering, auth propagation, rollback on nested failure, and consistency of the complete invoice-to-pool-to-escrow lifecycle |
| Deployment and operations | [`scripts/deploy.sh`](../scripts/deploy.sh), [`scripts/verify.sh`](../scripts/verify.sh), [`DEPLOYMENT.md`](../DEPLOYMENT.md) | Testnet network selection, admin/key handling, WASM identity, configured references/assets, and recovery from partial deployment |

Contract unit tests are colocated in each contract's `src/test.rs`; workspace integration tests are under `tests/integration/`. CI is defined in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

## Trust Boundaries and Assumptions

- The configured admin is trusted with verification, contract configuration, and other admin-gated operations. The current deployment path uses a deployer address; a multi-signature admin is an operational requirement to evaluate before Mainnet.
- Contracts trust addresses stored during initialization or later admin-authorized wiring. A wrong registry, invoice, pool, escrow, token, or external agent-registry address can break authorization or direct funds incorrectly.
- Issuer, buyer, liquidity-provider, and admin authorization is based on Soroban `require_auth`; reviewers should verify that every state transition authenticates the intended party and cannot be replayed or front-run into an invalid state.
- Funding-asset behavior depends on the deployed Stellar asset contract and its issuer/precision. Testnet USDC configuration is not evidence that a production issuer, asset, or redemption path is safe.
- Agent attestations depend on the configured external agent-registry contract, key format, signature verification, and key-rotation policy. That dependency is outside this repository and must be reviewed with the integration.
- Ledger timestamps, transaction resource limits, storage TTL, and Soroban authorization semantics are protocol inputs, not wall-clock or application-server guarantees.

## Known Risks and Required Review Questions

| Risk or open question | Current context | Review evidence to request |
|---|---|---|
| Privileged admin compromise or error | Admin-gated configuration and verification are described in [`docs/THREAT_MODEL.md`](THREAT_MODEL.md); the deployment scripts initialize with the deployer key by default | Enumerate admin-only methods, assess blast radius, ownership transfer, key custody, recovery, and multisig operations |
| Incorrect cross-contract wiring | Invoice, pool, and escrow hold contract references and call one another | Verify every stored reference against deployed IDs; test wrong-contract and failing-contract calls for atomic rollback |
| Asset and decimal assumptions | The primary workflow is testnet USDC; XLM deployment is marked experimental in [`DEPLOYMENT.md`](../DEPLOYMENT.md) | Verify token interface, precision, transfer failure behavior, issuer trust, and amount conversion boundaries |
| Share/accounting edge cases | Pool calculations handle deposits, withdrawals, repayment, defaults, yield, and fees | Independently derive conservation invariants and test rounding, zero shares, overflow, utilization boundaries, and adversarial call ordering |
| Lifecycle races and replay | Invoice transitions and permissionless expiry/default actions can interact within ledger ordering | Exercise duplicate, stale, reordered, and same-ledger calls; confirm no double funding or double settlement |
| Persistent storage expiry | Shared thresholds live in [`contracts/ttl/src/lib.rs`](../contracts/ttl/src/lib.rs); contract code renews selected entries on reads and writes | Inventory every persistent key, identify all reads that must renew it, and test behavior before and after threshold crossings and expiry boundaries |
| Pause completeness | Pause helpers are shared across contracts | Confirm every value-moving or privileged mutating entrypoint rejects while paused and that read-only paths remain intentional |
| Resource exhaustion | Batch and paginated entrypoints have bounds; cross-contract operations have Soroban resource costs | Establish maximum transaction sizes and budgets for worst-case loops, batches, and nested invocations |
| Deployment verification and partial failure | Deployment is resumable and can stop after deployment but before initialization/wiring | Verify script behavior on missing IDs, mismatched WASM, wrong admin, stale references, failed RPC calls, and experimental XLM configuration |
| Test boundary coverage | Property tests exist in contract crates, but they are not a substitute for independent adversarial review | Check typed errors, no partial mutation, malformed XDR/ScVals, enum boundaries, and cross-contract rollback across all public entrypoints |

The existing limitations and threat model should be read alongside this inventory: [`docs/LIMITATIONS.md`](LIMITATIONS.md), [`docs/THREAT_MODEL.md`](THREAT_MODEL.md), and [`docs/STORAGE.md`](STORAGE.md).

## Suggested Auditor Deliverables

1. A finding report with severity, affected version/commit, entrypoint, prerequisites, impact, and reproduction steps.
2. A review of the trust boundaries above, including explicit exclusions and dependencies outside this repository.
3. An invariant and test matrix covering authorization, asset conservation, lifecycle transitions, arithmetic boundaries, TTL, pause behavior, and rollback.
4. A retest report for fixes, plus a clear statement of what was and was not reviewed. Audit completion must not be represented as Mainnet readiness without separate deployment, key-management, and operational sign-off.

## Reproducible Repository Checks

From the repository root, with the pinned Rust toolchain installed:

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo build --workspace --release --target wasm32v1-none
```

These checks validate repository behavior and buildability only; they do not replace independent review, formal verification, or operational testing on Stellar Testnet.
