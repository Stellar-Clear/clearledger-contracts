# Deployment Guide

This document covers prerequisites, deployment flow, contract
wiring order, verification, and rollback for the ClearLedger
smart contracts on Stellar.

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust | 1.85.0 | `rustup toolchain install 1.85.0` |
| wasm32v1-none target | — | `rustup target add wasm32v1-none --toolchain 1.85.0` |
| Stellar CLI | latest | See [Stellar CLI docs](https://github.com/stellar/stellar-cli) |
| Python | 3 | Used by deployment metadata checks (`python3`) |
| jq | latest | Required by `scripts/maintainer/update-readme-addresses.sh`, run automatically at the end of `deploy.sh`. See [jq docs](https://jqlang.github.io/jq/download/) |

The repo ships `rust-toolchain.toml` pinning channel `1.85.0` and
target `wasm32v1-none`. `rustup` picks it up automatically.

For exact commands to build release WASM, compare local and deployed
hashes, inspect contract specs, and run the tests, see the
[WASM artifact contributor guide](docs/developer-guide/wasm-artifacts.md).

### Environment Variables

Copy `.env.example` to `.env` and fill in:

| Variable | Description |
|----------|-------------|
| `DEPLOYER_ACCOUNT` | Key alias for the deployer (default: `deployer`) |
| `USDC_ISSUER` | Stellar address of the USDC issuer on testnet |
| `XLM_ASSET` | Native XLM SAC contract address. Testnet: `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`. See [Native XLM](docs/NATIVE_XLM.md). |
| `DEPLOY_XLM` | Set to `true` to opt in to experimental XLM contracts; defaults to the supported USDC workflow |

## Native XLM via the Stellar Asset Contract

Native XLM is supported through its Stellar Asset Contract (SAC), not through an
issuer account. `XLM_ASSET` must be the network-specific SAC `C...` address, and
the pool's `funding_asset`, the XLM escrow's asset, and the invoice allow-list
entry must all use that same address. XLM uses 7 decimals (stroops), and the
native SAC has no issuer or administrator.

The canonical Testnet address is:

```
CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
```

Do not use the `native` keyword or a `XLM:G...` asset identifier in a Soroban
`Address` field. For address derivation, verification commands, factory
initialization requirements, and current limitations, see
[docs/NATIVE_XLM.md](docs/NATIVE_XLM.md).

XLM deployment remains opt-in (`DEPLOY_XLM=true`) and the frontend does not yet
expose an XLM pool. The bundled scripts deploy a dedicated XLM pool directly;
routing an XLM pool through `pool_factory::register_asset` is supported but
currently manual.

### Signing-Key and Log Handling

The scripts accept a Stellar CLI key alias in `DEPLOYER_ACCOUNT`; they do not
accept or write secret seeds. Keep signing keys in the Stellar CLI key store,
never in `.env`, deployment metadata, or command-line arguments. The Bash
scripts use a restrictive `umask` for generated local state and neither script
variant enables command tracing. On CLI deployment or initialization failures,
raw CLI output is withheld because it can contain signed transaction material;
the scripts print a safe failure summary instead. Do not enable PowerShell
transcription or shell tracing in environments that capture private command
input.

## Testnet Deployment

### 1. Create and Fund Deployer

```bash
bash scripts/setup-testnet.sh
```

This creates a key named `deployer` and funds it via Friendbot.
Wait ~10 seconds after running before proceeding.

### 2. Deploy Contracts

```bash
bash scripts/deploy.sh
```

The script is **idempotent** — re-running skips already-deployed
contracts. Deployed addresses are saved to `.deployed-addresses`.

#### Flags

| Flag | Behavior |
|------|----------|
| *(none)* | Resume mode — skips already-deployed contracts |
| `--fresh` | Ignore saved addresses and redeploy everything |
| `--resume` | Explicit resume (same as default) |
| `--dry-run` | Print what would be deployed without executing |
| `--only <contract>` | Redeploy a single contract (hotfix mode) |
| `--help` | Show usage |

#### Single-Contract Hotfix Redeploys

To hotfix a single contract without rerunning the full pipeline, pass
`--only <contract>` where `<contract>` is one of:
`registry`, `invoice`, `escrow_usdc`, `pool_usdc`, `escrow_xlm`, `pool_xlm`.

```bash
# Redeploy only the invoice contract, keeping all other addresses intact:
bash scripts/deploy.sh --only invoice
```

When `--only` is passed, the script:
1. Clears only the saved address and init flag for the targeted contract.
2. Loads all other saved addresses from `.deployed-addresses` for cross-contract references.
3. Runs `deploy_contract` + `invoke_init` only for the specified contract.
4. Generates `deployments.json` and updates `README.md` at the end (same as a full deploy).

### 3. Post-Deploy Wiring

After `deploy.sh` completes, copy the generated contract IDs
into the ClearLedger frontend:

```bash
cat .env.deployed
```

Paste the values into `clearledger-app/.env.local`.

## Contract Wiring Order

The deploy script initializes contracts in a specific order
because each contract references others:

```
1. registry_contract
   └─ no dependencies

2. invoice_contract
   └─ needs: registry_contract

3. escrow_contract (USDC)
   └─ needs: pool_contract, USDC asset

4. pool_contract (USDC)
   └─ needs: invoice_contract, escrow_contract, USDC asset

5. escrow_contract (XLM) [EXPERIMENTAL]
   └─ needs: pool_contract, native XLM SAC address

6. pool_contract (XLM) [EXPERIMENTAL]
   └─ needs: invoice_contract, escrow_contract, native XLM SAC address

7. Wire pool into invoice
   └─ invoice.set_pool_contract(pool_usdc)

8. Wire escrow into invoice
   └─ invoice.set_escrow_contract(escrow_usdc)
   Without this, repay / repay_partial / repay_early panic with
   `InvoiceError::NotFound` when reading `DataKey::EscrowContract`.

9. Allow-list funding assets
   └─ invoice.add_supported_asset(usdc)
   └─ invoice.add_supported_asset(xlm)
   Without this, `create` rejects every invoice with `UnsupportedAsset`.

10. Wire agent registry (optional — skipped when `AGENT_REGISTRY_CONTRACT`
    is unset)
    └─ invoice.set_agent_registry_contract(agent_registry)
    Without this, `submit_attestation` panics and `list_for_financing`
    can never unlock.
```

The registry must be deployed first because all other contracts
call `is_verified()` on it during initialization.

## Multi-sig Admin

Each protocol contract with an `admin` parameter accepts a Stellar account
address as its admin. A classic Stellar account can require multiple signers
for authorization by configuring its signer weights and thresholds; Soroban
then checks the account's configured threshold when the admin authorizes a
contract call. Configure this before
deploying contracts and pass the multisig account's `G...` address as `admin`
to each `initialize()` call. Keep the individual signer secret keys separate.
The bundled deployment scripts currently set admin to the deployer's address;
for a multisig deployment, initialize each contract with the multisig address
and submit those initialization transactions with the multisig threshold met.

### Testnet Example: 3-of-5

Create and fund five testnet signer accounts, then use Stellar Laboratory's
Testnet transaction builder to submit `Set Options` operations from the admin
account with these settings:

| Signer | Weight |
|--------|--------|
| Signer 1 | 1 |
| Signer 2 | 1 |
| Signer 3 | 1 |
| Signer 4 | 1 |
| Signer 5 | 1 |

Set the low, medium, and high thresholds to `3`. Each `Set Options` transaction
must be authorized by the account's current threshold. Set the account's master
signer weight to `0` so its own key does not count as one of the five signers.
The initial signer/threshold update must still be authorized using the
account's current settings. After setup,
record the admin account's `G...` address and use it as the admin for each
contract initialization. When invoking manually or adapting deployment
automation, pass this address as `--admin` instead of the deployer address and
submit the initialization transaction with the configured threshold. Subsequent
admin-only invocations, including contract upgrades, must likewise carry
authorization satisfying the 3-of-5 threshold.

## Testnet Pause and Recovery Runbook

This procedure applies to the current Testnet deployment only. It is an
operator response guide, not evidence of Mainnet readiness. The `pause` and
`unpause` entry points are available on the registry, invoice, escrow, and pool
contracts. Each contract has an independent pause flag; pausing one contract
does not pause the others. Read-only methods remain available while paused,
but guarded state changes fail with `ContractPaused`.

The manual commands below use the contract IDs from `.env` (see
`.env.example`) and the admin methods implemented in
`contracts/{registry,invoice,escrow,pool}/src/lib.rs`. `scripts/deploy.sh`
deploys contracts but does not provide an incident pause workflow.
`.github/workflows/ci.yml` runs build and test jobs only; it does not deploy,
pause, or resume contracts. An authorized Testnet operator must run this
procedure.

### Contain an Incident

1. Confirm the affected contract IDs in `.env` and check their admins with the
   existing read-only `get_admin` calls. Stop deploy/redeploy jobs while
   investigating; do not use `--fresh` as an incident response.
2. Use the configured admin signer (or the configured multi-sig threshold) to
   pause the affected contracts. For a broad protocol incident, pause each
   deployed pool first to stop new funding and LP activity, then pause the
   invoice, escrow, and registry contracts as needed. The experimental XLM
   contracts are optional and should only be included when deployed.

   ```bash
   stellar contract invoke --id "$POOL_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- pause
   stellar contract invoke --id "$POOL_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- pause
   stellar contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- pause
   stellar contract invoke --id "$ESCROW_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- pause
   stellar contract invoke --id "$ESCROW_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- pause
   stellar contract invoke --id "$REGISTRY_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- pause
   ```

   Omit an optional ID when its contract is not deployed. The signer used by
   `--source` must satisfy the contract admin's authorization policy; for a
   multi-sig admin, submit with the required threshold rather than replacing
   the admin with a single signer.

3. Confirm each transaction succeeds and inspect that contract's events on
   [Stellar Expert Testnet](https://stellar.expert/explorer/testnet) for the
   `paused` event and its admin. The current contracts do not expose a
   read-only pause-status method, so `get_admin`, `get_stats`, or `get_counts`
   alone cannot confirm pause state. Use those existing views to capture
   contract configuration and balances while investigating. Do not submit a
   real state-changing transaction just to test whether a contract is paused.
4. Record the incident time, affected contract IDs, transaction hashes,
   observed event(s), relevant ledger/state observations, and the operator who
   authorized the pause. Tell integrators that affected state-changing calls
   will fail until the contracts are resumed.

### Recover

1. Resolve the underlying incident and verify the affected contract wiring and
   state using the existing read-only queries in [Contract Verification](#contract-verification).
   Check the relevant pool `get_stats`, invoice `get_counts`, escrow lock
   records, and registry state before resuming. Re-running `scripts/verify.sh`
   is an additional deployment/wiring check; it does not report pause state.
2. Obtain the operator/multi-sig approval to resume, then call `unpause` on
   each contract that was paused. Use the same contract IDs, network, admin
   signer, and authorization threshold as for `pause`, replacing the final
   `pause` argument with `unpause`.
3. Confirm successful transaction inclusion and inspect each contract's
   `unpaused` event on Stellar Expert Testnet. Start with a small, controlled
   operation and monitor contract events and balances before restoring normal
   traffic. Do not describe Testnet assets as having Mainnet or real-value
   status.

## Agent Registry Wiring

`AGENT_REGISTRY_CONTRACT` in `.env.example` refers to the agent-registry
contract from the separate `underwrite-contract` repo. `deploy.sh` and
`deploy.ps1` wire it automatically **when** `AGENT_REGISTRY_CONTRACT` is
set in `.env`; otherwise the step is skipped with a clear warning.

To wire it manually (or re-wire it after rotating the agent registry):

1. Deploy the agent-registry contract from the `underwrite-contract` repo.
2. Set `AGENT_REGISTRY_CONTRACT` in your `.env` to its address.
3. Call `invoice.set_agent_registry_contract` with that address:

   ```bash
   stellar contract invoke \
     --id "$INVOICE_CONTRACT_ID" \
     --source "$DEPLOYER_ACCOUNT" \
     --network "$STELLAR_NETWORK" \
     -- set_agent_registry_contract \
        --agent_registry_contract "$AGENT_REGISTRY_CONTRACT"
   ```

This step is only required if agent-attested invoice submission is used;
skip it otherwise.

## Mainnet Deployment

Mainnet deployment is not yet supported. Before mainnet:

- [ ] Admin configured as a multi-sig account (see [Multi-sig Admin](#multi-sig-admin))
- [ ] Emergency pause mechanism implemented
- [ ] Security audit completed
- [ ] Issuer release wiring (Issue #56) resolved

When mainnet support is added, the same `deploy.sh` script
will work with `--network public` by updating `.env`.

## Contract Verification

After deployment, verify all contracts are live:

```bash
bash scripts/verify.sh
```

Or on Windows (PowerShell):

```powershell
powershell ./scripts/verify.ps1
```

This checks that each deployed contract responds, its WASM hash matches the
local release artifact, admin addresses match `DEPLOYER_ACCOUNT`, cross-contract
references and configured assets match `.env`, and the invoice asset allow-list
is correct. Experimental XLM contracts are checked only when
`DEPLOY_XLM=true`. Build the same release artifacts that were deployed before
running verification; a different local build is expected to fail the hash
check.

You can also verify on [Stellar Expert Testnet](https://stellar.expert/explorer/testnet).
To confirm the deployed bytecode matches your local build, compare hashes as
described in the [WASM artifact contributor guide](docs/developer-guide/wasm-artifacts.md#compare-wasm-hashes).

### Deployment Metadata Consistency

`deploy.sh` validates the generated `deployments.json`, `.env.deployed`, and
`.deployed-addresses` against each other and the README address table before it
reports completion. CI also checks that the README markers and required address
rows are present; locally generated files are compared whenever they exist.
Run the same checks manually with:

```bash
python3 scripts/check_deployment_metadata.py
python3 -m unittest scripts.test_deployment_metadata
```

## Contract Address Lifecycle & Rotation Policy

Testnet contract addresses are ephemeral and may be rotated at any time during development. When rotated, old addresses will drift silently and are no longer maintained.

### Rotation Conditions

Deployed addresses are saved in `.deployed-addresses`:

```
registry=<CONTRACT_ID>
invoice=<CONTRACT_ID>
escrow_usdc=<CONTRACT_ID>
pool_usdc=<CONTRACT_ID>
escrow_xlm=<CONTRACT_ID> (EXPERIMENTAL)
pool_xlm=<CONTRACT_ID> (EXPERIMENTAL)
```

- **Default mode** (resume): Reuses addresses found in `.deployed-addresses` and skips already-deployed contracts.
- **Fresh mode** (`--fresh`): Removes `.deployed-addresses` and starts clean, forcing a complete rotation of all contract addresses on-chain.

### Integrator Expectations

The deployment scripts write local, gitignored `deployments.json` metadata. The Bash deployment pipeline passes that metadata to `scripts/maintainer/update-readme-addresses.sh`, which refreshes both each contract ID and its individual Stellar Expert Testnet link in `README.md`. The PowerShell deployment also refreshes the README from its deployment metadata. These links target Testnet and do not imply Mainnet support.

> [!NOTE]
> **Single-Contract Hotfix Redeploys:** Use `bash scripts/deploy.sh --only <contract>` (e.g. `--only invoice`) to redeploy a single contract without re-running the full pipeline. The script will automatically update `deployments.json` and `README.md` with the new address. See the [Single-Contract Hotfix](#single-contract-hotfix-redeploys) section under **Flags** above for full details.

Integrators and contributors should:
1. Treat testnet addresses as volatile.
2. Regularly pull the latest changes from the `main` branch to synchronize with the current testnet environment.
3. Check `README.md` for the current canonical testnet addresses rather than hardcoding them in local environments.

## Rollback

There is no automated rollback. To redeploy from scratch:

```bash
bash scripts/deploy.sh --fresh
```

This ignores all saved addresses and deploys fresh contracts.
The old contract addresses will still exist on-chain but are
no longer referenced by the application.

To manually invalidate old deployments:
1. Revoke all verified issuers/buyers from the old registry
2. Pause LP deposits on old pool contracts (when pause is implemented)
3. Deploy new contracts with `--fresh`
4. Update `clearledger-app/.env.local` with new addresses
5. Notify LPs to withdraw from old pools and deposit into new ones

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `stellar CLI not found` | Install Stellar CLI or add to PATH |
| `DEPLOYER_ACCOUNT not set` | Copy `.env.example` to `.env` and fill in values |
| Deploy hangs waiting for contract | Check network connection; testnet may be slow |
| `400` from Friendbot | Account likely already funded — this is normal |
| `contract not found` for `XLM_ASSET` | Re-derive the SAC address with `stellar contract id asset --network testnet --asset native`; use the `C...` address, never a `G...` issuer. See [docs/NATIVE_XLM.md](docs/NATIVE_XLM.md). |
| Contract not found on verify | Wait longer after deploy; re-run `verify.sh` |
