# ClearLedger Soroban Contracts

**Invoice-finance infrastructure built with Soroban on Stellar.**

This Rust workspace implements ClearLedger’s on-chain coordination layer on [Soroban, Stellar’s smart-contract platform](https://developers.stellar.org/docs/build/smart-contracts/overview). The contracts manage participant verification, invoice lifecycle transitions, USDC liquidity pools, and escrow operations, with application and SDK integrations built around Stellar’s contract interfaces and events.

[Launch the Stellar Testnet app](https://clearledger-three.vercel.app) · [Application repository](https://github.com/Stellar-Clear/clearledger) · [Stellar Testnet deployments](#testnet-deployments) · [Soroban architecture](docs/NEW_DESIGN.md) · [Deployment guide](DEPLOYMENT.md) · [Security policy](SECURITY.md)

## Contract system

| Contract | Responsibility |
| --- | --- |
| `registry_contract` | Stores participant profiles and verification status for issuers and buyers. |
| `invoice_contract` | Creates invoice records and enforces lifecycle transitions, including delivery confirmation and repayment. |
| `pool_contract` | Accepts liquidity, issues transferable LP shares, funds eligible invoices, and accounts for repayment. |
| `escrow_contract` | Holds funds during the funding flow and supports configured release and recovery operations. |
| `pool_factory` | Supports deploying and tracking separate pool instances by asset. |

The system coordinates on-chain state and token transfers. It does not validate the underlying commercial documents or establish that an invoice is legitimate.

### Invoice lifecycle

```text
Created → Listed → Funded → Active → Confirmed → Repaid
                              └──────────────────→ Defaulted
```

Participant verification and invoice status are checked on-chain at relevant transitions. The optional Underwrite agent registry integration is a separate dependency and must be configured for attestation-gated listing.

## Testnet deployments

ClearLedger’s USDC contracts are live on Stellar Testnet. These Soroban contract IDs connect the application stack to Stellar’s test network; addresses may change when contracts are redeployed. See the [deployment guide](DEPLOYMENT.md) for address lifecycle details.

<!-- START_DEPLOYED_ADDRESSES -->
| Contract | Address | Stellar Expert |
|----------|---------|---------------|
| registry_contract | `CCGHK4XJA23BVPTADPCOUV76G2KYAAAKQIKPF35FN5RFP2TT47TXWQNT` | [View](https://stellar.expert/explorer/testnet/contract/CCGHK4XJA23BVPTADPCOUV76G2KYAAAKQIKPF35FN5RFP2TT47TXWQNT) |
| invoice_contract | `CCZN4GM5O7D6RGI4P4FCKIB5HMRKDWOWY7M3YZ637XZFCGM7SXGIVAZ6` | [View](https://stellar.expert/explorer/testnet/contract/CCZN4GM5O7D6RGI4P4FCKIB5HMRKDWOWY7M3YZ637XZFCGM7SXGIVAZ6) |
| escrow_contract | `CC3EDR7B64Y5KZEJETATNVUWYGPHGUAR2V2QYTUCDOZJMYY3XEVAIGQB` | [View](https://stellar.expert/explorer/testnet/contract/CC3EDR7B64Y5KZEJETATNVUWYGPHGUAR2V2QYTUCDOZJMYY3XEVAIGQB) |
| pool_contract | `CDBJEGCV3IENZJKCK3LYYOSA7B5NZSPELBE7FPFHUKLG74LRJTCQIZ45` | [View](https://stellar.expert/explorer/testnet/contract/CDBJEGCV3IENZJKCK3LYYOSA7B5NZSPELBE7FPFHUKLG74LRJTCQIZ45) |
<!-- END_DEPLOYED_ADDRESSES -->

## Build and contribute

The workspace pins Rust 1.85.0 and the `wasm32v1-none` target in `rust-toolchain.toml`. Install the toolchain and Stellar CLI, then run:

```bash
rustup toolchain install 1.85.0
rustup target add wasm32v1-none --toolchain 1.85.0
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace --release --target wasm32v1-none
```

See [Building and inspecting WASM artifacts](docs/developer-guide/wasm-artifacts.md) for artifact paths, local and on-chain hash comparison, contract spec inspection, test execution, and troubleshooting.

For a fresh testnet deployment, configure `.env` using `.env.example`, fund a testnet deployer, then run:

```bash
bash scripts/setup-testnet.sh
bash scripts/deploy.sh
```

The deployment script supports resumable deployments and updates the address table above. See [DEPLOYMENT.md](DEPLOYMENT.md) for configuration, wiring, verification, and rollback details. The default deployment targets the supported USDC workflow; XLM deployment is experimental and opt-in.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a change. Architecture and integration references include the [threat model](docs/THREAT_MODEL.md), [event catalog](docs/EVENTS.md), [storage schema](docs/STORAGE.md), [registry metadata privacy and data-minimization guidance](docs/METADATA_PRIVACY.md), and [known limitations](docs/LIMITATIONS.md).

## Network and security

These contracts are deployed on Stellar Testnet and have not been formally audited. The current implementation has known functional and trust-model limitations, including an issuer payout path that is not yet wired into invoice funding. Review [known limitations](docs/LIMITATIONS.md) and [SECURITY.md](SECURITY.md) before integrating. Do not use real funds or treat testnet assets as redeemable value.
