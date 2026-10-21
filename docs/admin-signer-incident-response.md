# Compromised admin signer response

This runbook concerns the currently deployed registry, invoice, pool and escrow
contracts. It changes no authorization rules. It is a response plan, not a
Mainnet-readiness claim or a guarantee that compromised custody can be recovered.
Use public identifiers in incident records; never include seeds, private keys,
wallet recovery phrases, authentication tokens or unsigned secret material.

## Establish the incident record

Record the network/passphrase, observation timestamp and ledger, public contract
IDs, each contract's `get_admin` result, suspected public signer address, affected
transactions, public asset IDs and balances. Read admin state from the selected
network; do not assume `.deployed-addresses` or a previous deployment is current.
An account G-address, a signer key, and a contract C-address are different things.
If an admin is a C-address, investigate its custom authorization policy rather
than applying classic account multisig instructions to it.

Appoint a response lead and independent verifier. Preserve public transaction
hashes, event payloads, deployed WASM hashes and configuration history so the
team can distinguish signer compromise, incorrect wiring and code compromise.
Do not publish unconfirmed attribution or an unverified recovery deadline.

## Contain using available valid authority

1. Stop operator automation that signs, deploys, changes fees/wiring or resumes
   the service. Disable new financing and funding actions in operator-controlled
   interfaces; an interface change alone does not stop direct contract calls.
2. If legitimate signers can still satisfy the stored admin's authorization,
   invoke **pool.pause → invoice.pause → escrow.pause → registry.pause**. Include
   every deployed pool/escrow asset instance and any separately deployed factory
   in the incident inventory; this four-contract sequence is not a factory pause
   API. Verify each transaction's result and `paused` event before moving on.
3. Treat this as several independently confirmed transactions, not an atomic
   global pause. A partial failure leaves a partial pause. Track the exact state
   of every dependency and retries; a compromised signer may also attempt to
   unpause or alter configuration. Never infer containment from one event alone.
4. If the valid threshold cannot be met, do not try an authorization bypass.
   Escalate immediately through the project's established incident channels,
   suppress new user actions where possible, and describe the on-chain limitation.

Read-only queries remain available while paused. Pause blocks normal mutation,
including repayment/release paths; it is not a recovery withdrawal facility.
`unpause` is deliberately callable while paused but still requires admin auth.
Check `contracts/{pool,invoice,escrow,registry}/src/lib.rs` before preparing the
network-specific invocations. Use the configured Stellar CLI and a reviewed
signing process; this document intentionally contains no secret-bearing commands.

## Replace signers or change the stored admin

For a **classic admin account**, inspect its current signer weights, master-key
weight and low/medium/high thresholds. With sufficient uncompromised authority,
prepare a reviewed SetOptions transaction that installs replacement public signer
keys, removes the compromised signer's weight and establishes the intended
multisig thresholds. Signer/threshold changes require the account's applicable
high threshold. Verify the resulting signer set and thresholds on-chain and prove
that legitimate operators can still authorize the required actions. Avoid a
sequence that makes the account permanently unsignable. If only a signer changes
on the same admin account, the contract's stored admin address need not change.

Contract ownership changes are a distinct operation:

| Contract | Current mechanism / limitation |
| --- | --- |
| Registry | `transfer_ownership(new_admin)` requires current and new admin authorization. `transfer_admin` requires only current admin and is a lower-security escape hatch; use the dual-auth path when possible. |
| Invoice | `transfer_ownership(new_admin)` requires current and new admin authorization. |
| Pool | `transfer_ownership(new_admin)` requires current and new admin authorization. |
| Escrow | No public ownership transfer entry point exists in this checkout. `upgrade` requires the current admin; do not invent a `transfer_ownership` call. |

The three available ownership transfer methods are guarded by the pause check.
Do **not** blindly unpause compromised control just to rotate ownership. Prefer
securing the current admin account's signer set while contracts remain paused.
If that is impossible, prepare an incident-specific, independently reviewed plan
for any narrowly controlled resume/transfer window or redeployment. For a contract
admin C-address, follow that contract's `__check_auth` and recovery mechanisms;
classic account signer-weight changes do not apply to its contract storage.

Invoice and escrow expose `upgrade(new_wasm_hash)`; registry and pool do not in
this version. An upgrade is not a substitute for recovering valid authorization.
Use only audited WASM with a verified hash, explicit storage compatibility and a
migration plan. A signer incident by itself does not establish that an upgrade
is necessary.

## Decide whether to redeploy

Redeploy only after checking whether the current authority, contract code and
wiring remain trustworthy. Inventory every outstanding invoice, LP position,
escrow lock, asset balance, protocol-fee configuration and indexer checkpoint.
New contract IDs do not automatically migrate storage or unlock old escrow.
Changing frontend addresses or running `deploy.sh --fresh` does not move funds or
invalidate the compromised deployment. Any migration needs valid authority,
asset-specific settlement rules and reconciliation evidence. If the old authority
is irrecoverable, record the unavailable operations instead of promising recovery.

Maintain a public old→new contract address map and activation ledger when a
replacement is approved. Preserve indexer history and prevent users from mixing
old and new invoice/pool/escrow dependencies. Verify Testnet recovery rehearsal
before any separately approved production use; Testnet assets do not establish
real-value recovery or guaranteed financing/returns.

## Resume and communicate

After signer/wiring/code verification, resume dependencies in reverse order:
registry, escrow, invoice, then pool. Confirm each result; perform controlled
synthetic transactions and reconcile balances, locks and accounting before
re-enabling user-facing financing. Verify fee/treasury, utilization, allowlisted
asset and ownership settings against the approved record. Keep alerts for further
signer, ownership, pause and upgrade events.

Publish status updates using only public facts:

```text
Network and observation ledger:
Affected public contract IDs / asset IDs:
Confirmed impact and currently disabled actions:
Containment transaction hashes and confirmation state:
Verified replacement admin/signers or replacement contract IDs:
User actions currently requested (or none):
Next update time and established project contact channel:
```

Follow with a timeline and reconciled impact report. Do not ask users to share
wallet secrets or send funds to an unverified “recovery” address.

## References

- [Stellar signatures, signer weights and multisig thresholds](https://developers.stellar.org/docs/learn/fundamentals/transactions/signatures-multisig)
- Repository: `SECURITY.md`, `DEPLOYMENT.md`, `contracts/pause/src/lib.rs` and the
  four contract implementations listed above.
