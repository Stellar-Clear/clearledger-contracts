# Native XLM via the Stellar Asset Contract

This document defines how ClearLedger supports the native Stellar asset (XLM)
through the Stellar Asset Contract (SAC), what must be configured before an XLM
pool is deployed, and how to verify the setup.

Native XLM support is implemented at the contract layer. The deployment scripts
still treat XLM as opt-in and the frontend does not yet expose an XLM pool, so
`DEPLOY_XLM` defaults to `false`.

## Canonical native SAC addresses

The native asset has no issuer. Its SAC address is derived from the network
passphrase, so it is different on every network. Use the contract address, not
the string `native` or a `XLM:G...` asset identifier, wherever a Soroban
`Address` is required.

| Network | Native XLM SAC address | Status |
| --- | --- | --- |
| Testnet | `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC` | Deployed; supported target |
| Futurenet | `CB64D3G7SM2RTH6JSGG34DDTFTQ5CFDKVDZJZSODMCX4NJ2HV2KN7OHT` | Reference only |
| Mainnet | `CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA` | Reference only; mainnet deployment is not supported yet |

There is no issuer account and no SAC administrator for the native asset. The
SAC's `admin()` entry point is not usable, and native XLM cannot be minted,
burned, clawed back, or frozen through issuer flags. Do not model XLM as an
issued asset with an issuer address.

The native SAC is already deployed on Testnet (and Mainnet). Deploying it again
with `stellar contract asset deploy --asset native` fails because the instance
already exists; obtaining its address with `stellar contract id asset` is the
correct operation.

## Testnet setup and verification

Set the Testnet SAC address in `.env` (the value in `.env.example` is correct):

```bash
XLM_ASSET=CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
DEPLOY_XLM=true
```

Derive the address independently before using it:

```bash
stellar contract id asset --network testnet --asset native
# CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC
```

Verify that the contract exists, exposes the SAC interface, and reports the
expected precision:

```bash
stellar contract info interface \
  --id "$XLM_ASSET" \
  --network testnet

stellar contract invoke \
  --id "$XLM_ASSET" \
  --source "$DEPLOYER_ACCOUNT" \
  --network testnet \
  --send=no \
  -- decimals
# 7

stellar contract invoke \
  --id "$XLM_ASSET" \
  --source "$DEPLOYER_ACCOUNT" \
  --network testnet \
  --send=no \
  -- symbol
# "native"
```

XLM uses 7 decimal places (1 XLM = 10,000,000 stroops). Every amount passed to
a pool, escrow, or SAC call is denominated in stroops.

## Pool initialization requirements

A pool instance supports exactly one funding asset. For native XLM, that asset
is the network's native SAC contract address.

| Requirement | Value for native XLM |
| --- | --- |
| `funding_asset` | The canonical SAC `C...` address for the network |
| Asset precision | 7 decimals (`stroops`) |
| Issuer / SAC admin | Not applicable; native XLM has neither |
| Escrow asset | The same SAC address passed to `escrow.initialize` (its parameter is historically named `usdc_asset`) |
| LP share decimals | `7`, so shares use the same base unit as XLM |
| Minimum initial deposit | `10_000_000` stroops (1 XLM) for factory-deployed pools; direct deployments may choose a different value in stroops |
| Invoice allow-list | The same SAC address must be added with `invoice.add_supported_asset` |
| Address collisions | `funding_asset` must differ from the admin, invoice, escrow, and registry addresses |

The escrow must be initialized before the pool because `pool.initialize`
cross-checks `escrow.get_usdc_asset()` against `funding_asset`. At the token
level, contract-held XLM balances live in contract data entries; no trustline is
needed for a contract address. Classic account holders still need a spendable
XLM balance.

### Deploying through the pool factory

With the factory, the escrow for XLM must already be initialized with the SAC
address. Then register the asset:

```bash
stellar contract invoke \
  --id "$POOL_FACTORY_CONTRACT_ID" \
  --source "$DEPLOYER_ACCOUNT" \
  --network testnet \
  -- register_asset \
     --asset "$XLM_ASSET" \
     --pool_wasm_hash "$POOL_WASM_HASH" \
     --invoice_contract "$INVOICE_CONTRACT_ID" \
     --escrow_contract "$ESCROW_XLM_CONTRACT_ID"
```

`POOL_WASM_HASH` is the 32-byte hex hash printed by `stellar contract upload`.
`register_asset` deploys a new pool instance, initializes it, and records the
asset-to-pool mapping. The factory supplies its defaults: share name
`ClearLedger Pool Shares`, symbol `TT-POOL`, 7 share decimals, and a
`10_000_000` stroop minimum initial deposit. It also uses the factory admin as
the pool admin and treasury, and reads the registry address back from the
invoice contract. If different share metadata or a different minimum deposit is
required, initialize a pool directly and adopt it with
`register_existing_pool`.

An asset can only be registered once. Re-registering the same SAC address fails
with `AssetAlreadyRegistered`; there is no replacement semantics.

### Direct initialization

If a pool needs custom share metadata or a different minimum deposit, initialize
the escrow and pool directly, then optionally adopt the pool with
`register_existing_pool`. Initialize the escrow with the same SAC address first:

```bash
stellar contract invoke \
  --id "$ESCROW_XLM_CONTRACT_ID" \
  --source "$DEPLOYER_ACCOUNT" \
  --network testnet \
  -- initialize \
     --admin "$DEPLOYER_ADDRESS" \
     --pool_contract "$POOL_XLM_CONTRACT_ID" \
     --usdc_asset "$XLM_ASSET"
```

Then initialize the pool:

```bash
stellar contract invoke \
  --id "$POOL_XLM_CONTRACT_ID" \
  --source "$DEPLOYER_ACCOUNT" \
  --network testnet \
  -- initialize \
     --admin "$DEPLOYER_ADDRESS" \
     --invoice_contract "$INVOICE_CONTRACT_ID" \
     --escrow_contract "$ESCROW_XLM_CONTRACT_ID" \
     --funding_asset "$XLM_ASSET" \
     --registry_contract "$REGISTRY_CONTRACT_ID" \
     --treasury "$DEPLOYER_ADDRESS" \
     --min_initial_deposit 10000000 \
     --share_name '"ClearLedger XLM Pool Shares"' \
     --share_symbol '"TT-XLM"' \
     --share_decimals 7
```

### Deploying with the bundled scripts

Setting `DEPLOY_XLM=true` makes `scripts/deploy.sh` (and `deploy.ps1`) deploy a
dedicated XLM escrow and pool directly, then allow-list `XLM_ASSET` on the
invoice contract. The scripts pass `--funding_asset "$XLM_ASSET"` and
`--share_decimals 7`; they do not currently route the XLM pool through the
factory.

To exercise the opt-in path:

```bash
DEPLOY_XLM=true bash scripts/deploy.sh
```

Re-run verification afterwards:

```bash
bash scripts/verify.sh
```

### Post-deployment checks

Confirm the pool is bound to the SAC address and uses 7-decimal shares:

```bash
stellar contract invoke --id "$POOL_XLM_CONTRACT_ID" \
  --source "$DEPLOYER_ACCOUNT" --network testnet \
  -- get_funding_asset
# CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC

stellar contract invoke --id "$POOL_XLM_CONTRACT_ID" \
  --source "$DEPLOYER_ACCOUNT" --network testnet \
  -- decimals
# 7
```

Confirm the invoice contract will accept the native SAC as a funding asset:

```bash
stellar contract invoke --id "$INVOICE_CONTRACT_ID" \
  --source "$DEPLOYER_ACCOUNT" --network testnet \
  -- is_supported_asset --asset "$XLM_ASSET"
# true
```

## Limitations

- XLM deployment is opt-in and remains experimental; the frontend does not
  support selecting or displaying the XLM pool.
- The factory does not validate that an `Address` is a SAC. It will deploy a
  pool for any address, including a `G...` issuer account. A pool funded with a
  non-SAC address deploys successfully but token transfers fail later, so pass
  the canonical `C...` SAC address only.
- Each pool funds a single asset. There are no multi-asset pools or swaps
  between XLM and USDC.
- Native XLM cannot be administered through the SAC: no issuer flags, minting,
  burning, or clawback path is available.
- Mainnet deployment is not supported yet. The Mainnet address in the table is
  for reference and must not be used as a Testnet configuration.
- Testnet state is ephemeral; re-derive the SAC address and re-check the
  deployment after a network reset.
