# Supported Stellar Testnet asset configuration

This repository's supported deployment path is the **Testnet USDC-style asset
workflow**, not Mainnet and not a promise that a token has monetary value.
`scripts/deploy.sh` explicitly passes `--network testnet`. XLM deployment is
experimental and opt-in in that script (`DEPLOY_XLM=true`); the PowerShell script
has different deployment behavior, so review it separately before use.

## Asset identity and configuration

A classic Stellar asset is identified by **code + issuing account**. Its Soroban
Stellar Asset Contract (SAC) has a separate **C-address**, derived for the selected
network. A G-address identifies an account, not the contract to invoke for token
transfers. A ticker alone is insufficient to identify an asset.

| Setting / record | Expected value | Where used |
| --- | --- | --- |
| `USDC_ASSET_CODE` | `USDC`, if that is the selected issued asset | Asset identity metadata in `.env.example`; not used to derive a SAC by deploy.sh. |
| Issuing account | Verified public G-address for the selected Testnet asset | Record alongside the asset code and network. The example issuer is a reference, not proof of the currently deployed asset. |
| `USDC_ISSUER` | **Testnet SAC C-address** despite the legacy variable name | Both deploy scripts pass it to escrow `usdc_asset`, pool `funding_asset`, and invoice `add_supported_asset`; verify.sh also passes it to `is_supported_asset`. |
| `XLM_ASSET` | Native asset's **Testnet SAC C-address**, not the word `native` | Experimental XLM escrow/pool configuration. |
| `DEPLOY_XLM` | `false` for the supported Bash USDC-only path | Bash deployment opt-in for experimental XLM contracts. |

Do not paste an issuing G-address into `USDC_ISSUER`: the contract parameters
require the token contract. There is currently no issuer-to-SAC conversion in
these deployment scripts. Resolve and verify the selected asset's SAC using the
installed Stellar CLI/RPC before deployment; do not infer it from an account
address or copy a Mainnet contract ID. `.env.example` intentionally leaves this
C-address blank rather than pretending an issuer account is a contract.

Maintain a public deployment record with:

```text
network: Test SDF Network ; September 2015
asset code: USDC
issuing account: <verified public G-address>
asset contract: <verified Testnet C-address>
decimals: <token decimals confirmed from contract; expected 7>
verification ledger: <ledger sequence and timestamp>
registry / invoice / escrow / pool: <public deployed C-addresses>
```

These placeholders must be replaced by verified public identifiers. No secret
seeds, signing keys or API credentials belong in this record.

## Precision, balances and trustlines

The contract types and deployment share configuration use **7 decimal places**:
one whole token is represented by 10,000,000 integer units. Invoice face values,
deposits, funding and repayments use integer token units, not floating-point
amounts. Pool shares also use a configured precision and are not the same asset
as USDC. Check the selected SAC's actual decimals before using these assumptions.
The label “USDC” alone does not establish issuer identity or interoperability.

For an issued classic asset, participating Stellar accounts must have the
appropriate trustline and satisfy the issuer's authorization/limit requirements
before receiving the asset. Friendbot funds **Testnet XLM** for reserves and fees;
it does not establish a USDC trustline or supply the issued token. Native XLM has
no issued-asset trustline. Soroban contract balances do not use a G-account's
classic trustline in the same way; verify recipient address type and token
behavior rather than treating every Address as a classic account.

## Deployment and verification checklist

1. Start from `.env.example`; record the network, code, issuer G-address and
   verified SAC C-address separately. Set the legacy `USDC_ISSUER` variable to
   that **C-address** and set `DEPLOY_XLM=false` for Bash.
2. Confirm the CLI's named `testnet` configuration points to the intended Testnet
   RPC/passphrase. Network-derived IDs can change when the network is reset.
3. Review `scripts/deploy.sh --help` and use its dry-run mode before live actions.
   Review existing `.deployed-addresses`: resume mode may retain old wiring.
4. After deployment, verify escrow `get_usdc_asset`, pool `get_funding_asset`
   (legacy alias `get_usdc_asset`) and invoice `is_supported_asset` all identify
   the same SAC. Verify the pool's escrow/invoice references too.
5. Check token decimals and test balances, then run a small synthetic funding and
   payout flow. Keep real assets and production credentials out of this setup.

Explorer references (replace placeholders with public identifiers):

- Issuer: `https://stellar.expert/explorer/testnet/account/<G-address>`
- Issued asset: `https://stellar.expert/explorer/testnet/asset/USDC-<G-address>`
- Contract: `https://stellar.expert/explorer/testnet/contract/<C-address>`

Explorers are supporting views, not substitutes for checking RPC results and
network identity. No fixed current deployment addresses are asserted by this
page. The relevant files are `.env.example`, `scripts/deploy.sh`,
`scripts/deploy.ps1`, `scripts/verify.sh`, `contracts/pool/src/types.rs`, and
`contracts/invoice/src/types.rs`.

## Stellar references

- [Stellar Asset Contract: account and contract balance behavior](https://developers.stellar.org/docs/tokens/stellar-asset-contract)
- [Verify issued-asset trustlines](https://developers.stellar.org/docs/build/guides/basics/verify-trustlines)
