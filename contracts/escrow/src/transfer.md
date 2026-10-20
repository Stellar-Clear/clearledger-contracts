
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

- [ ] Admin key migrated to multi-sig
- [ ] Emergency pause mechanism implemented
- [ ] Security audit completed
- [ ] Issuer release wiring (Issue #56) resolved

When mainnet support is added, the same `deploy.sh` script
will work with `--network public` by updating `.env`.

## Contract Verification

After deployment, verify all contracts are live:
