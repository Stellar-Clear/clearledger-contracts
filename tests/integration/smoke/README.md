# Testnet wiring smoke checks

After a reviewed deployment, run from the repository root:

```sh
python3 tests/integration/smoke/deployment_wiring.py \
  --deployments deployments.json \
  --asset "$VERIFIED_TESTNET_ASSET_CONTRACT" \
  --source-account "$PUBLIC_TESTNET_SOURCE_ACCOUNT" \
  --agent-registry "$EXPECTED_AGENT_REGISTRY"
```

Omit `--agent-registry` only when no agent registry is expected; the script then
reports that check as skipped. `deploy.sh` writes `deployments.json` with registry,
invoice, escrow_usdc and pool_usdc IDs. Its optional agent registry is not currently
included in that output, so supply the expected address explicitly (or add an
`agent_registry` field to your public manifest). Use a SAC C-address for `--asset`,
not the issuing G-account currently suggested by the legacy USDC_ISSUER name.

The tool checks public StrKey types/checksums, registry readability, both directions
of invoice/pool/escrow wiring, funding-asset consistency, invoice asset allowlisting
and optional agent registry configuration. A mismatch, malformed/missing deployment
field, timeout, failed CLI call or non-JSON response fails the command. IDs in a
manifest alone are not proof of a correct deployment.

The installed Stellar CLI must support `contract invoke --send=no`. The tool pins
the public Testnet RPC and passphrase and always requests simulation without
submission. The source must be an existing public Testnet G-address; no private
key, seed or funded-value signing credential is accepted or needed. Some view
methods bump TTL: simulation does not persist those bumps. Never drop `--send=no`
just because a method is described as a read. No deployments, signing or balance
transfers are performed by this script.

This is an operator-run live smoke check, not an automatic PR-CI network dependency.
Native integration tests cannot establish live deployment addresses. These reads
do not prove token decimals, issuer identity, economic invariants, real-value
readiness, or successful payout; verify those separately on Testnet. A skipped
optional agent check is not a claim the invoice can be listed without attestation.
