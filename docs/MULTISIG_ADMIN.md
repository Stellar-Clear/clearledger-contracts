# Multisig Admin Deployment and Recovery Runbook

> **Testnet only.** This runbook uses Stellar Testnet, Friendbot, and
> testnet contract addresses. It makes no Mainnet guarantees. Testnet state
> can be reset, contract addresses can change, and no value should be treated
> as redeemable. Commands were checked against Stellar CLI `28.1.0`.

This runbook covers the operational side of making a Stellar account the
admin of the ClearLedger contracts:

- create a 3-of-5 Stellar account with weighted signers;
- initialize contracts with that account as `admin`;
- rotate signers without changing the contract admin address;
- execute pause, unpause, upgrade, and ownership actions;
- recover from a lost signer, a stuck pause, or a bad upgrade.

For the ordinary deployment flow, see [DEPLOYMENT.md](../DEPLOYMENT.md). The
deployment scripts are not multisig-aware; see
[Option A and Option B](#2-initialize-contracts-with-the-multisig-admin).

## Contract authority map

These are the admin-relevant entry points in this checkout.

| Contract | Initialize | Admin getter | Upgrade | Pause / unpause | Ownership change |
|---|---|---|---|---|---|
| `registry` | `initialize(admin)` | `get_admin()` | none | `pause()`, `unpause()` | `transfer_ownership(new_admin)` and `transfer_admin(new_admin)` |
| `invoice` | `initialize(admin, registry_contract)` | `get_admin() -> Option<Address>` | `upgrade(new_wasm_hash)` | `pause()`, `unpause()` | `transfer_ownership(new_admin)` |
| `pool` | `initialize(admin, invoice_contract, escrow_contract, funding_asset, registry_contract, treasury, min_initial_deposit, share_name, share_symbol, share_decimals)` | `get_admin()` | none | `pause()`, `unpause()` | `transfer_ownership(new_admin)` |
| `escrow` | `initialize(admin, pool_contract, usdc_asset)` | `get_admin()` | `upgrade(new_wasm_hash)` | `pause()`, `unpause()` | none |
| `pool_factory` | `initialize(admin)` | none | none | none | none |

Authorization details:

- `initialize` calls `admin.require_auth()`.
- `pause` and `unpause` require the stored admin. `unpause` is deliberately
  **not** guarded by the pause flag, so a paused contract can be resumed.
- `upgrade` requires the stored admin and is **not** guarded by the pause flag.
  An admin can upgrade while paused.
- `transfer_ownership` requires **both** the current admin and the new admin.
  It is pause-guarded, so unpause first.
- `registry.transfer_admin` requires only the current admin. It is the
  single-authorization escape hatch; use it only for deliberate emergency
  rotation.
- Invoice wiring and admin setters (`set_pool_contract`,
  `set_escrow_contract`, `set_agent_registry_contract`,
  `add_supported_asset`, `remove_supported_asset`, `set_expiry_window`) and
  pool `set_protocol_fee` require the stored admin and are pause-guarded.
- Only `invoice` and `escrow` expose `upgrade`. `pool`, `registry`, and
  `pool_factory` have no upgrade entry point.
- `escrow` and `pool_factory` have no ownership-transfer entry point. Their
  admin is fixed at initialization; signer rotation on the admin account is
  the only way to change who controls it.
- `pool_factory` has no public admin getter and no pause entry point.
- The pause flag has no public `is_paused` view in any contract. Verify the
  pause by its event or by probing a guarded call (see
  [Verify pause](#verify-pause)).
- `pool.set_max_utilization(admin, new_cap_bps)` takes an explicit `admin`
  argument and only calls `require_auth()` on that argument; it does not
  compare it to the stored admin. Do not treat it as part of the
  multisig-only surface.

## The signing model: make the admin the transaction source

Soroban authorization for the admin is easiest to satisfy when the admin
account is the **transaction source**. Simulation then records a
`source_account` authorization entry instead of an `Address` entry, and the
classic Stellar envelope signatures from the multisig signers authorize the
call.

If you use a different source (for example, the deployer), simulation records
an `Address` entry for the multisig `G...` account. `stellar tx sign` signs
envelopes, not arbitrary Soroban `Address` authorization entries, and a
multisig account has no single secret key. That path fails with an auth error
or `MissingSignerForAddress`.

For that reason every admin command below uses:

```bash
--source-account "$ADMIN_MULTISIG"
```

and follows this pipeline for Soroban calls:

1. `stellar contract invoke --build-only` to build the unsigned call;
2. `stellar tx simulate` to record Soroban resources and `source_account` auth;
3. `stellar tx sign` three times with three signer keys;
4. `stellar tx send`.

Do not build several admin transactions at once. They all consume the admin
account sequence number; submit them one at a time.

## Prerequisites

```bash
export NETWORK=testnet
export DEPLOYER_ACCOUNT=deployer
export USDC_ISSUER=GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5
```

You also need the deployed contract IDs, either from `.deployed-addresses` or
from `.env.deployed`:

```bash
export REGISTRY_ID=C...
export INVOICE_ID=C...
export ESCROW_ID=C...
export POOL_ID=C...
```

`funding_asset` is the Soroban token contract address, not the `G...` issuer
account. If `USDC_ISSUER` is the issuer account, derive the Stellar Asset
Contract address before initializing:

```bash
export FUNDING_ASSET=$(stellar contract id asset \
  --asset "USDC:${USDC_ISSUER}" \
  --network testnet)
```

If your environment already stores the `C...` asset contract address, use it
directly. Passing a `G...` issuer where a token contract is expected can let
initialization succeed while later token transfers fail.

For [Option A](#option-a-deploy-with-the-future-admin-bootstrap-key-then-harden),
`scripts/deploy.sh` passes `USDC_ISSUER` directly as the escrow/pool asset, so
set `USDC_ISSUER` to this `C...` value (not the `G...` issuer) in `.env`.

## 1. Create the 3-of-5 admin account

### 1.1 Generate and fund keys

The bootstrap key creates and funds the future admin account. The five signer
keys can be funded testnet accounts or unfunded ed25519 keys; funding them
makes verification easier.

```bash
stellar keys generate cl-admin-bootstrap --network testnet --fund
for n in 1 2 3 4 5; do
  stellar keys generate "cl-signer-$n" --network testnet --fund
done

export ADMIN_MULTISIG=$(stellar keys address cl-admin-bootstrap)
echo "$ADMIN_MULTISIG"
```

The admin account's `G...` address does not change when signers are added or
the master weight is disabled. That address is the value passed as `admin` to
every contract.

### 1.2 Add five signers with weight 1

`stellar tx new set-options` accepts one signer per transaction. Add each
signer in its own transaction. These transactions are still authorized by the
bootstrap master key, whose weight is still 1 and whose thresholds are still
the account defaults.

```bash
for n in 1 2 3 4 5; do
  SIGNER=$(stellar keys address "cl-signer-$n")
  stellar tx new set-options \
    --source-account cl-admin-bootstrap \
    --signer "$SIGNER" \
    --signer-weight 1 \
    --network testnet
done
```

### 1.3 Set thresholds and disable the master key

Do this only after all five signers have been added. The operation is atomic:

```bash
stellar tx new set-options \
  --source-account cl-admin-bootstrap \
  --master-weight 0 \
  --low-threshold 3 \
  --med-threshold 3 \
  --high-threshold 3 \
  --network testnet
```

Every admin operation now needs three signer weights. The bootstrap key alone
can no longer authorize anything. Retire or archive the bootstrap secret
according to your key policy; do not delete the only record of the account
address.

### 1.4 Verify the account

```bash
curl -s "https://horizon-testnet.stellar.org/accounts/$ADMIN_MULTISIG" \
  | jq '{
      thresholds,
      signers: [.signers[] | {key, weight}],
      balances: [.balances[] | {asset_type, balance}]
    }'
```

Expected:

- `thresholds.low_threshold`, `thresholds.med_threshold`, and
  `thresholds.high_threshold` are all `3`;
- the five `cl-signer-*` keys each have weight `1`;
- the bootstrap address has no positive-weight signer entry.

Also confirm the account has enough XLM for Soroban resource fees:

```bash
curl -s "https://horizon-testnet.stellar.org/accounts/$ADMIN_MULTISIG" \
  | jq '.balances[] | select(.asset_type == "native") | .balance'
```

If it is low, top it up with Friendbot
(`curl -s "https://friendbot.stellar.org?addr=$ADMIN_MULTISIG"`) or a testnet
payment of `100000000` stroops (10 XLM) from `$DEPLOYER_ACCOUNT`.

## 2. Initialize contracts with the multisig admin

There are two valid approaches. Choose one before deploying; `initialize` is
one-shot and reverts with `AlreadyInitialized` if called twice.

### Option A: deploy with the future admin bootstrap key, then harden

This is the least error-prone testnet path because it uses
[`scripts/deploy.sh`](../scripts/deploy.sh) unchanged. The admin address is the
bootstrap account's `G...` address; the account becomes 3-of-5 immediately
after deployment. It is single-key **during** deployment, so do it in a
maintenance window and harden immediately.

1. Put the bootstrap key alias in `.env`:

   ```bash
   DEPLOYER_ACCOUNT=cl-admin-bootstrap
   # Use the C... Stellar Asset Contract address derived above, not the G... issuer.
   USDC_ISSUER=<C... asset contract address>
   ```

2. Run the normal deployment:

   ```bash
   bash scripts/deploy.sh
   ```

   The script initializes each contract with `--admin "$DEPLOYER_ADDRESS"` and
   sets the pool treasury to the same address. Both are the future multisig
   `G...` address.

3. Complete steps [1.2](#12-add-five-signers-with-weight-1) and
   [1.3](#13-set-thresholds-and-disable-the-master-key) to turn that account
   into 3-of-5.

4. Run the [post-initialization checks](#post-initialization-checks).

### Option B: initialize directly with an already-hardened multisig

Use this when the multisig account already exists and its master key is
already disabled. `scripts/deploy.sh` cannot sign with a multisig source and
always initializes with the deployer as admin, so deploy the Wasm manually and
use the multisig ceremony for `initialize`.

Build once:

```bash
stellar contract build
```

Deploy helper:

```bash
deploy_wasm() {
  local wasm="$1"
  local out
  out=$(stellar contract deploy \
    --wasm "$wasm" \
    --source-account "$DEPLOYER_ACCOUNT" \
    --network testnet)
  printf '%s\n' "$out" | grep -Eo 'C[A-Z2-7]{55}' | tail -n1
}

REGISTRY_ID=$(deploy_wasm target/wasm32v1-none/release/clearledger_registry.wasm)
INVOICE_ID=$(deploy_wasm target/wasm32v1-none/release/clearledger_invoice.wasm)
ESCROW_ID=$(deploy_wasm target/wasm32v1-none/release/clearledger_escrow.wasm)
POOL_ID=$(deploy_wasm target/wasm32v1-none/release/clearledger_pool.wasm)
```

Admin-call helper for a hardened multisig:

```bash
sign_with_three() {
  local in="$1"
  local signer
  for signer in cl-signer-1 cl-signer-2 cl-signer-3; do
    stellar tx sign "$in" \
      --sign-with-key "$signer" \
      --network testnet > "$in.$signer.xdr"
    in="$in.$signer.xdr"
  done
  printf '%s\n' "$in"
}

admin_invoke() {
  local contract="$1"
  local label="$2"
  shift 2
  local unsigned="tx-$label.unsigned.xdr"
  local simulated="tx-$label.simulated.xdr"

  stellar contract invoke \
    --id "$contract" \
    --source-account "$ADMIN_MULTISIG" \
    --network testnet \
    --build-only \
    -- "$@" > "$unsigned"

  stellar tx simulate "$unsigned" \
    --source-account "$ADMIN_MULTISIG" \
    --network testnet > "$simulated"

  local signed
  signed=$(sign_with_three "$simulated")
  stellar tx send "$signed" --network testnet
}
```

Initialize in dependency order. Registry first, then invoice, then escrow, then
pool, then invoice wiring:

```bash
admin_invoke "$REGISTRY_ID" registry-init -- \
  initialize \
  --admin "$ADMIN_MULTISIG"

admin_invoke "$INVOICE_ID" invoice-init -- \
  initialize \
  --admin "$ADMIN_MULTISIG" \
  --registry_contract "$REGISTRY_ID"

admin_invoke "$ESCROW_ID" escrow-init -- \
  initialize \
  --admin "$ADMIN_MULTISIG" \
  --pool_contract "$POOL_ID" \
  --usdc_asset "$FUNDING_ASSET"

admin_invoke "$POOL_ID" pool-init -- \
  initialize \
  --admin "$ADMIN_MULTISIG" \
  --invoice_contract "$INVOICE_ID" \
  --escrow_contract "$ESCROW_ID" \
  --funding_asset "$FUNDING_ASSET" \
  --registry_contract "$REGISTRY_ID" \
  --treasury "$ADMIN_MULTISIG" \
  --min_initial_deposit 10000000 \
  --share_name '"ClearLedger USDC Pool Shares"' \
  --share_symbol '"TT-USDC"' \
  --share_decimals 7

admin_invoke "$INVOICE_ID" invoice-set-pool -- \
  set_pool_contract \
  --pool_contract "$POOL_ID"

admin_invoke "$INVOICE_ID" invoice-set-escrow -- \
  set_escrow_contract \
  --escrow_contract "$ESCROW_ID"

admin_invoke "$INVOICE_ID" invoice-add-asset -- \
  add_supported_asset \
  --asset "$FUNDING_ASSET"
```

If an agent registry is configured:

```bash
admin_invoke "$INVOICE_ID" invoice-set-agent -- \
  set_agent_registry_contract \
  --agent_registry_contract "$AGENT_REGISTRY_CONTRACT"
```

### Optional pool factory

`scripts/deploy.sh` does not deploy `pool_factory`. If you use it, deploy it
with `deploy_wasm .../clearledger_pool_factory.wasm` and initialize it with
`--admin "$ADMIN_MULTISIG"` through the same ceremony. Its admin is fixed for
the life of the contract; rotate signers on the admin account to change control.

### Post-initialization checks

Read-only calls can use any funded source; they do not need multisig
authorization:

```bash
for id in "$REGISTRY_ID" "$INVOICE_ID" "$ESCROW_ID" "$POOL_ID"; do
  echo "== $id"
  stellar contract invoke --id "$id" \
    --source-account "$DEPLOYER_ACCOUNT" \
    --network testnet -- get_admin
done
```

`invoice.get_admin` returns an `Option<Address>` and prints `null` when not
initialized; the other getters panic with `NotInitialized`. Run
`scripts/verify.sh` for the wiring checks (`get_pool_contract`,
`get_escrow_contract`, `is_supported_asset`); it reads contract IDs from
`.env`.

## 3. Rotate signers

Signer rotation happens on the Stellar account with `set-options`; it does not
call any contract function and does not change the contract admin address.
Keep at least the threshold number of signers valid throughout the rotation.

Add the replacement first, then remove the old signer. With 3-of-5, the
overlap is 6 signers with a threshold of 3.

Helper for account-level `set-options`:

```bash
signer_set_options() {
  local label="$1"
  shift
  local unsigned="tx-$label.xdr"
  stellar tx new set-options \
    --source-account "$ADMIN_MULTISIG" \
    --network testnet \
    --build-only \
    "$@" > "$unsigned"
  local signed
  signed=$(sign_with_three "$unsigned")
  stellar tx send "$signed" --network testnet
}
```

Add a replacement signer:

```bash
# If the replacement key does not exist yet:
stellar keys generate cl-signer-6 --network testnet --fund
NEW_SIGNER=$(stellar keys address cl-signer-6)

signer_set_options add-signer-6 \
  --signer "$NEW_SIGNER" \
  --signer-weight 1
```

Remove the old signer only after confirming the new signer can sign. This
example removes `cl-signer-5`, leaving the three keys used by
`sign_with_three` available:

```bash
OLD_SIGNER=$(stellar keys address cl-signer-5)
signer_set_options remove-signer-5 \
  --signer "$OLD_SIGNER" \
  --signer-weight 0
```

If you rotate one of the three keys used by `sign_with_three`, update that
helper to use the new key set.

`--signer-weight 0` deletes that signer. Each transaction contains one signer
change. Verify the result with the Horizon command in
[1.4](#14-verify-the-account) and by decoding a signed transaction:

```bash
signed=$(sign_with_three tx-add-signer-6.xdr)
stellar tx decode "$signed" | jq '.tx.signatures | length'
```

The decoded envelope should contain three distinct signatures, and the
on-chain signer list should show six signers after the add and five after the
remove.

If you change thresholds, do it in a separate `set-options` transaction and
verify the new threshold with the Horizon check. Do not lower the threshold
and disable the master key in the same window unless the final signer set is
already in place.

## 4. Execute admin actions

All examples below assume `ADMIN_MULTISIG` is the transaction source and that
`sign_with_three` and `admin_invoke` are defined as in
[Option B](#option-b-initialize-directly-with-an-already-hardened-multisig).

### Pause

Pause each contract independently. Pausing one contract does not pause the
others.

```bash
admin_invoke "$REGISTRY_ID" pause-registry -- pause
admin_invoke "$INVOICE_ID" pause-invoice -- pause
admin_invoke "$ESCROW_ID" pause-escrow -- pause
admin_invoke "$POOL_ID" pause-pool -- pause
```

`pause` itself is not guarded by the pause flag, so repeating it is harmless
but unnecessary. The factory has no pause entry point.

### Verify pause

There is no `is_paused` getter. A successful pause transaction emits a
`paused` event with the admin in `topic[1]`; see the
[event catalog](EVENTS.md#paused). To probe the state, simulate a guarded call.
If the contract is paused, it must fail with `Error(Contract, #1)`
(`ContractPaused`).

```bash
# Invoice: expect ContractPaused when paused.
stellar contract invoke --id "$INVOICE_ID" \
  --source-account "$ADMIN_MULTISIG" \
  --network testnet --send=no \
  -- add_supported_asset --asset "$FUNDING_ASSET"
```

Use the same `--send=no` pattern with a guarded call on the other contracts:
`transfer_admin` on registry and `set_protocol_fee` on pool. Any error other
than `Error(Contract, #1)` means the pause flag is not the cause. Read-only
calls such as `get_admin`, `get_counts`, `get_stats`, and `get_locked` remain
callable while paused.

### Unpause

`unpause` is deliberately not pause-guarded and requires the same admin
authorization:

```bash
admin_invoke "$REGISTRY_ID" unpause-registry -- unpause
admin_invoke "$INVOICE_ID" unpause-invoice -- unpause
admin_invoke "$ESCROW_ID" unpause-escrow -- unpause
admin_invoke "$POOL_ID" unpause-pool -- unpause
```

Re-run the probe and confirm that it no longer returns
`Error(Contract, #1)`.

### Upgrade

Only `invoice` and `escrow` expose `upgrade`. Uploading a Wasm is permissionless;
the `upgrade` call is admin-only. Upload first and keep the returned 64-character
hex hash:

```bash
stellar contract upload \
  --wasm target/wasm32v1-none/release/clearledger_invoice.wasm \
  --source-account "$DEPLOYER_ACCOUNT" \
  --network testnet

export NEW_INVOICE_WASM_HASH=<64-hex-hash-from-upload>
export NEW_ESCROW_WASM_HASH=<64-hex-hash-from-upload>
```

Upgrade invoice:

```bash
admin_invoke "$INVOICE_ID" upgrade-invoice -- \
  upgrade \
  --new_wasm_hash "$NEW_INVOICE_WASM_HASH"
```

Upgrade escrow:

```bash
admin_invoke "$ESCROW_ID" upgrade-escrow -- \
  upgrade \
  --new_wasm_hash "$NEW_ESCROW_WASM_HASH"
```

`upgrade` replaces the current contract Wasm immediately. It does not run a
constructor or migrate storage, and it is not pause-guarded. Review storage
layout changes and keep the previous Wasm and its hash before upgrading. An
`upgraded` event is emitted with the new hash.

`registry`, `pool`, and `pool_factory` have no `upgrade` entry point; changing
their code requires a fresh deployment and address migration.

### Ownership transfer

#### Registry emergency rotation: `transfer_admin`

This requires only the current admin and is the preferred single-auth rotation
path for the registry. It is pause-guarded, so unpause first.

```bash
export NEW_ADMIN=G...
admin_invoke "$REGISTRY_ID" registry-transfer-admin -- \
  transfer_admin \
  --new_admin "$NEW_ADMIN"
```

This does not require the new admin to sign. Use it only when that is the
intended security trade-off. The dual-auth `transfer_ownership` is safer for
planned transfers.

#### Dual-auth ownership transfer

`invoice.transfer_ownership` and `pool.transfer_ownership` require signatures
from **both** the current admin and `new_admin`. This is harder than the other
admin actions: the current admin can authorize through the transaction source,
but `new_admin` is a separate `Address` authorization entry.

Prepare and inspect the two required auth entries:

```bash
export NEW_ADMIN=G...
export NEW_ADMIN_ALIAS=cl-new-admin

stellar contract invoke \
  --id "$INVOICE_ID" \
  --source-account "$ADMIN_MULTISIG" \
  --network testnet \
  --build-only \
  -- transfer_ownership \
  --new_admin "$NEW_ADMIN" > transfer-invoice.unsigned.xdr

stellar tx simulate transfer-invoice.unsigned.xdr \
  --source-account "$ADMIN_MULTISIG" \
  --network testnet > transfer-invoice.simulated.xdr

stellar tx decode transfer-invoice.simulated.xdr \
  | jq '.tx.tx.operations[0].body.invoke_host_function.auth'
```

You should see one `source_account` credential (the current admin) and one
`address` credential for `new_admin`. Envelope signatures alone do not satisfy
the `new_admin` entry. Use a Soroban client or wallet that can add the new
admin's auth-entry signature, then sign the envelope with three current admin
signers and submit with `stellar tx send`. If `new_admin` is another multisig
account, a `stellar-cli`-only transfer is not possible.

A `stellar-cli`-only fallback on Testnet is possible only when `new_admin` is a
single-key account whose secret the CLI can access. It temporarily re-enables
the bootstrap key as a weight-3 signer, invokes directly as that key with
`new_admin` as a local alias, then immediately disables it again:

```bash
# Requires 3-of-5. The bootstrap key alone has weight 3 until the last step.
signer_set_options master-weight-3 --master-weight 3

stellar contract invoke \
  --id "$INVOICE_ID" \
  --source-account cl-admin-bootstrap \
  --network testnet \
  -- transfer_ownership \
  --new_admin "$NEW_ADMIN_ALIAS"

signer_set_options master-weight-0 --master-weight 0
```

The direct invoke signs the envelope with the bootstrap key (weight 3) and
signs the `new_admin` `Address` auth entry with `cl-new-admin`. Do not use this
fallback on Mainnet. It creates a window in which one key can authorize admin
actions. If the final step fails, the bootstrap key is still weight 3; retry it
immediately or complete the transfer and re-harden. For `pool`, repeat with
`$POOL_ID`.

For `registry`, `transfer_ownership` has the same dual-auth requirement; use
`transfer_admin` when only the current admin needs to authorize.

`escrow` and `pool_factory` have no ownership-transfer function. To move
control, deploy a new instance with a new admin.

## 5. Emergency recovery

### Lost or compromised signer, quorum still reachable

1. Generate a replacement key.
2. Add it with weight 1 using [signer rotation](#3-rotate-signers).
3. Remove the lost or compromised signer with weight 0.
4. Verify the Horizon signer list and decode a three-signature transaction.

Rotate immediately if a signer is compromised. If an attacker can also gather
three signer weights, treat the admin account as compromised: do not rely on
pause or ownership transfer. Deploy fresh contracts under a new admin account
and move users to them.

### Lost quorum

Stellar classic accounts have no social recovery and no admin override. If
fewer than three signer weights remain available, the admin account cannot
authorize:

- `pause` or `unpause`;
- `upgrade`;
- `transfer_ownership` or `transfer_admin`;
- invoice wiring or admin setters.

There is no contract-level recovery function that bypasses the stored admin.
The operational fallback is a fresh deployment with a new multisig admin:

```bash
# With a new, funded bootstrap key in .env:
DEPLOYER_ACCOUNT=cl-admin-bootstrap-2
bash scripts/deploy.sh --fresh
```

Then update `.env.deployed`, the frontend environment, and the address table.
If the old contracts are not paused, users may still be able to interact with
them; if they are paused and the admin is lost, funds and state in those
contracts cannot be moved through the protocol. Treat this as a testnet
incident and document the abandoned addresses.

### Stuck pause

If the admin still has quorum, `unpause` is unguarded and is the recovery path:

```bash
admin_invoke "$INVOICE_ID" unpause-invoice -- unpause
```

Repeat for `registry`, `escrow`, and `pool`. If quorum is lost, there is no
force-unpause or timelock. Deploy fresh contracts and migrate. Do not assume
that a new deployment can recover funds locked in the old paused contracts.

### Bad upgrade or rollback

There is no automatic rollback and no upgrade timelock. If the upgraded code
still exposes `upgrade` and still satisfies the admin auth check:

1. Upload the previous Wasm (or rebuild the exact previous commit) and keep its
   hash.
2. Call `upgrade` with the old hash using the same admin ceremony.
3. Verify `get_admin`, wiring, pause state, and a read-only call.

```bash
export OLD_INVOICE_WASM_HASH=<previous-64-hex-hash>
admin_invoke "$INVOICE_ID" rollback-invoice -- \
  upgrade \
  --new_wasm_hash "$OLD_INVOICE_WASM_HASH"
```

If the bad upgrade removed `upgrade`, changed the admin storage key, or made
the contract unresponsive, on-chain rollback is not possible. Deploy fresh
contracts with `--fresh` and migrate.

### Full redeploy rollback

Use the [DEPLOYMENT.md rollback procedure](../DEPLOYMENT.md#rollback):

1. Pause old contracts if the admin is still available.
2. Deploy with `bash scripts/deploy.sh --fresh`.
3. Update `.env.deployed` and the frontend.
4. Notify LPs and integrators to withdraw from old pools and use the new
   addresses.

`--fresh` only changes local saved addresses; it does not delete or upgrade
old on-chain contracts.

## 6. Verification and failure modes

Before sending, confirm the envelope has three distinct signer signatures:

```bash
stellar tx decode "$signed" | jq '.tx.signatures | length'
```

### Common failures

| Symptom | Likely cause | Check or fix |
|---|---|---|
| `Error(Contract, #1)` | Contract is paused (`ContractPaused`) | Unpause with the admin, or probe a different guarded call |
| `Error(Auth, InvalidAction)` | Fewer than three valid signer weights, wrong source, or an unsigned `Address` auth entry | Use `--source-account "$ADMIN_MULTISIG"`, sign with three distinct signers, and check the envelope |
| `MissingSignerForAddress` | The call needs auth from the multisig `G...` as an `Address`, but the CLI has no secret for a multisig account | Make the admin the transaction source, or use a Soroban signer that can add that auth entry |
| `Error(Contract, #4)` on registry | Registry not initialized | Run `initialize` or check `get_admin` |
| `Error(Contract, #20)` on invoice | Invoice not initialized | Run `initialize` or check `get_admin` |
| `Error(Contract, #6)` on escrow | Escrow not initialized | Run `initialize` or check `get_admin` |
| `Error(Contract, #2)` on pool or factory | Not initialized | Run `initialize` |
| `Error(Contract, #1)` on an `initialize` call | `AlreadyInitialized` | Do not initialize again; use `get_admin` and wiring checks |
| Token transfer fails after successful initialization | `funding_asset` is the `G...` issuer, not the `C...` token contract | Derive the SAC address with `stellar contract id asset` |
| Friendbot returns `400` | Account already funded | Top up with a payment from a funded testnet account |
| `txBadSeq` | Another admin transaction consumed the sequence number | Rebuild from a fresh `--build-only` call; do not reuse old XDR |
| `txInsufficientBalance` | Admin account cannot pay resource fees | Fund the admin account with XLM |

### Final checklist

- [ ] Admin account shows thresholds `3/3/3`, five signers of weight `1`, and
      no positive-weight bootstrap key.
- [ ] `get_admin()` on registry, invoice, escrow, and pool returns the multisig
      `G...` address; invoice wiring checks pass.
- [ ] Pause probes return `Error(Contract, #1)` while paused and stop returning
      it after unpause.
- [ ] The upgrade hash and previous Wasm hash are recorded, and the `upgraded`
      event is visible.
- [ ] Signer backup and `.env.deployed` are current; no secrets are logged.

## 7. Known gaps and non-goals

- This is Testnet-only. Multisig is enforced by the Stellar account, not by the
  contracts; the contracts only see the admin address and call `require_auth()`.
- `scripts/deploy.sh` hardcodes the deployer as admin and is not multisig-aware.
  Use [Option A](#option-a-deploy-with-the-future-admin-bootstrap-key-then-harden)
  or [Option B](#option-b-initialize-directly-with-an-already-hardened-multisig).
- There is no pause-state getter, upgrade timelock, or contract-level recovery
  or admin override.
- `stellar-cli` cannot sign an `Address` authorization entry for a multisig
  account; dual-auth transfers to another multisig require a Soroban-capable
  signer or wallet.
- `pool.set_max_utilization` accepts an explicit `admin` argument and does not
  compare it to the stored admin; see the [authority map](#contract-authority-map).
- Testnet state may reset; do not treat testnet contracts or addresses as
  durable production infrastructure.

Related references: [deployment guide](../DEPLOYMENT.md), [event catalog](EVENTS.md),
[known limitations](LIMITATIONS.md), and [security policy](../SECURITY.md).
