# Threat Model

> **Updated:** 2026-10-20
> **Applies to:** `registry`, `invoice`, `escrow`, `pool`, and `pool_factory`
> contracts, plus the shared `clearledger-pause` helper.
> **Network:** Stellar **Testnet only**. There are no Mainnet guarantees, no
> formal audit, and no production-tested governance process.

This document describes the trust assumptions, authentication gates, and
attack surfaces of the ClearLedger protocol as implemented in this repository.
It is intended for security researchers, integrators, and contributors
evaluating the protocol's risk profile.

Everything below is written against the testnet deployment model: assets have
no real-world value, admin keys are ordinary Stellar accounts (not multisigs),
and the protocol must not be used with real funds.

---

## Table of Contents

- [Trust Model](#trust-model)
- [Authentication Gates](#authentication-gates)
- [Funding Authorization](#funding-authorization)
- [Ownership Transfer](#ownership-transfer)
- [Pause Controls](#pause-controls)
- [Upgrade Powers](#upgrade-powers)
- [Attack Vectors](#attack-vectors)
- [Centralization Risks](#centralization-risks)
- [Defence-in-Depth](#defence-in-depth)
- [Out-of-Scope](#out-of-scope)
- [Reporting](#reporting)

---

## Trust Model

### Roles

Each deployed contract stores its **own** admin in instance storage. There is
no protocol-wide admin and no shared ownership registry. Admins are the
addresses that signed the corresponding `initialize()` call.

| Role | Description | Trust Level |
|------|-------------|-------------|
| **Registry admin** | Verifies, revokes, and reinstates profiles; runs batch registration; pauses the registry; can transfer registry ownership. | **High** — controls identity truth for the protocol |
| **Invoice admin** | Wires the invoice contract to pool/escrow/agent-registry contracts, manages supported assets and the listing-expiry window, pauses the invoice contract, upgrades it, and transfers its ownership. | **High** — can rewire repayment flows and replace code |
| **Escrow admin** | Pauses the escrow contract and upgrades its code. There is no ownership-transfer entry point. | **High** — upgrade power reaches all locked funds |
| **Pool admin** | Pauses the pool, sets the protocol fee/treasury, changes the utilization cap, and transfers pool ownership. | **High** — can halt LP withdrawals and change risk parameters |
| **Pool-factory admin** | Deploys/registers pool instances. Becomes the new pool's admin and initial treasury. | **High** — chooses the Wasm and admin for every pool it deploys |
| **Issuer** | A registry profile that creates invoices, lists them, marks shipment, and can expire its own stale listing. Profiles start unverified until an admin verifies them. | Low — self-authenticates |
| **Buyer** | A registry profile obligated to repay; can confirm delivery and repay. | Low — self-authenticates |
| **LP** | Deposits the funding asset and receives SEP-41 transferable shares; can withdraw, transfer, approve, and transfer-from. | Low — self-authenticates |
| **Underwrite agent** | Off-chain signer registered in the external agent-registry contract. Its signed attestation unlocks listing. | Medium — trusted per-invoice risk attestation |
| **Anonymous caller** | Any Stellar account calling a permissionless entry point. | None — no auth required |

### Trust Boundaries

```
  ┌──────────────────────────────────────────────────────────────┐
  │                     Contract layer                           │
  │  ┌──────────┐ ┌─────────┐ ┌────────┐ ┌──────┐ ┌───────────┐  │
  │  │ Registry │ │ Invoice │ │ Escrow │ │ Pool │ │PoolFactory│  │
  │  └────┬─────┘ └────┬────┘ └───┬────┘ └──┬───┘ └─────┬─────┘  │
  │       │            │          │         │           │        │
  │       └────────────┴──────────┴─────────┴───────────┘        │
  │          Each contract trusts its stored wiring addresses     │
  └──────────────────────────────────────────────────────────────┘
            ▲                                   ▲
            │                                   │
   Registry profiles / agent          Self-authenticated LPs,
   attestations gate listing          issuers, and buyers
```

**Key trust assumptions:**

- Each contract trusts the addresses written by its `initialize()` call. A
  wrong wiring address at initialization can make funds unrecoverable.
- The invoice contract's pool, escrow, and agent-registry references can be
  changed later by the **invoice admin** (`set_pool_contract`,
  `set_escrow_contract`, `set_agent_registry_contract` in
  `contracts/invoice/src/lib.rs:193`, `:366`, `:280`). The pool's contract
  references, the escrow's pool/asset references, and the factory's asset
  mappings have no post-initialization setters.
- A pool deployed through `register_asset` is initialized with the factory
  admin as both its admin and initial treasury
  (`contracts/pool_factory/src/lib.rs:127-159`).

---

## Authentication Gates

Every state-changing function enforces one of the following patterns.

### Pattern A — Self-auth (`address.require_auth()`)

The caller authenticates only themselves. No admin is involved.

| Function | Contract | Who Auths | Notes |
|----------|----------|-----------|-------|
| `register_issuer`, `register_buyer` | Registry | The profile address | New profiles start unverified |
| `update_profile`, `update_metadata` | Registry | The profile address | Owner-only edits |
| `create`, `batch_create` | Invoice | The issuer | Once per issuer per batch |
| `list_for_financing`, `batch_list_for_financing` | Invoice | Each invoice's issuer | Once per distinct issuer |
| `cancel` | Invoice | The issuer | Only from `Created`; **not** pause-guarded |
| `mark_shipped` | Invoice | The issuer | `Funded` → `Active` |
| `confirm_delivery` | Invoice | The confirmer (issuer or buyer) | Both confirmations needed |
| `repay`, `repay_partial`, `repay_early` | Invoice | The buyer | `repay` pays the remaining balance |
| `expire_listing` | Invoice | `caller`, which must be the stored issuer **or** the invoice admin | Only after the expiry window |
| `deposit`, `withdraw` | Pool | The LP | LP position owner |
| `transfer`, `transfer_shares` | Pool | `from` | SEP-41 share transfers |
| `approve` | Pool | `from` | SEP-41 allowance |
| `transfer_from` | Pool | `spender` | Spends an unexpired allowance |

### Pattern B — Admin auth (`admin.require_auth()`)

Only the stored admin may call these (except the `set_max_utilization`
discrepancy noted below).

| Function | Contract | Why |
|----------|----------|-----|
| `batch_register_issuers`, `batch_register_buyers` | Registry | Bulk identity creation |
| `revoke`, `reinstate`, `verify_profile` | Registry | Controls verification status |
| `pause`, `unpause` | Registry | Circuit breaker |
| `transfer_ownership` | Registry | Also requires the new admin |
| `transfer_admin` | Registry | Single-signer escape hatch; see below |
| `upgrade` | Invoice, Escrow | Replaces contract Wasm |
| `set_pool_contract`, `set_escrow_contract`, `set_agent_registry_contract` | Invoice | Rewires cross-contract value flows |
| `add_supported_asset`, `remove_supported_asset` | Invoice | Controls which assets new invoices may use |
| `set_expiry_window` | Invoice | Listing-expiry configuration (≤ 31,536,000 s) |
| `pause`, `unpause` | Invoice, Escrow, Pool | Circuit breaker |
| `transfer_ownership` | Invoice, Pool | Also requires the new admin |
| `set_protocol_fee` | Pool | Fee (≤ 2000 bps) and treasury address |
| `set_max_utilization` | Pool | **See authorization discrepancy below** |
| `register_asset`, `register_existing_pool` | Pool factory | Deploys or records a pool |
| `compact_history` | Escrow | Prunes on-chain history; admin is accountable for retention |

**Pool authorization discrepancy (`set_max_utilization`).** The function
accepts an `admin: Address` argument and calls `admin.require_auth()` on that
argument, but never compares it to the stored `DataKey::Admin`
(`contracts/pool/src/lib.rs:1914-1926`). Any address that signs its own call
can therefore change the utilization cap; the stored admin is not actually
enforced. The test at `contracts/pool/src/test.rs:1530` only proves that an
*unsigned* non-admin fails, not that a signed non-admin is rejected. Treat the
cap as effectively caller-controlled until the contract is fixed.

### Pattern C — Cross-contract auth (`contract.require_auth()`)

One contract authenticates as the caller of another contract's function.

| Function | Contract | Authenticated Caller | Why |
|----------|----------|---------------------|-----|
| `mark_funded` | Invoice | The pool configured by `set_pool_contract` | Prevents faking `funding_pool` |
| `mark_defaulted` | Invoice | The invoice's recorded `funding_pool` | Persists default status |
| `receive_repayment`, `receive_repayment_with_refund` | Pool | The configured invoice contract | Prevents faking repayment accounting |
| `handle_default` | Pool | The configured invoice contract | Prevents unauthorised default recovery |
| `lock`, `release_to_pool` | Escrow | The configured pool contract | Only the pool can move escrowed funds |
| `release_to_issuer` | Escrow | The configured pool contract | Only the pool can release, and the recipient must match the issuer recorded at lock time |
| `handle_default` | Escrow | The configured pool contract **or** the escrow admin | The admin can force recovery after the 60-second lock grace period |

`mark_funded` additionally checks that its `pool_address` argument equals the
configured pool both before and after authenticating
(`contracts/invoice/src/lib.rs:1316-1332`). Escrow's `handle_default` allows
two callers, not just the pool (`contracts/escrow/src/lib.rs:386-406`).

### Pattern D — Dual auth (two `require_auth()` calls)

| Function | Contracts | Signers |
|----------|-----------|---------|
| `transfer_ownership` | Registry, Invoice, Pool | Current admin **and** `new_admin` |

Contrary to earlier versions of this document, `initialize()` is **not**
dual-auth. It requires only the incoming admin's signature and is one-shot
(`registry/src/lib.rs:57-64`, `invoice/src/lib.rs:97-111`,
`escrow/src/lib.rs:45-62`, `pool/src/lib.rs:108-224`,
`pool_factory/src/lib.rs:52-60`). The first caller to initialize a freshly
deployed instance chooses its admin.

### Pattern E — Permissionless (no auth)

| Function | Contract | Notes |
|----------|----------|-------|
| `fund_invoice`, `batch_fund_invoice` | Pool | Any caller can allocate pool capital to an eligible listed invoice |
| `trigger_default` | Invoice | Any caller once `now >= due_date` and status is `Funded`/`Active`/`Confirmed` |
| `submit_attestation` | Invoice | No caller auth; trust comes from the secp256k1 signature and agent registry |
| `get_profile`, `is_verified`, `get_verification_status`, `get_admin`, `get_profile_count`, `list_profiles` | Registry | Read-only |
| `list_profiles_page` | Registry | Read-only view |
| All `get_*`, `get_by_*`, `get_counts`, `balance`, `total_supply`, `allowance`, `get_attestation` | Invoice, Pool, Escrow | Read-only |
| `get_stats`, `get_lp_position`, `get_utilization_rate`, `get_usdc_asset` | Pool | Read-only views |
| `get_locked`, `get_locked_at`, `get_history`, `get_history_page` | Escrow | Read-only views; retained history is bounded and paginated |
| `get_funded_amount`, `get_expiry_window` | Invoice | Read-only views |
| `expire_listing` | Invoice | Also callable by issuer (Pattern A) |
| `get_pool_for_asset`, `get_aggregate_stats`, `list_assets` | Pool factory | Read-only |

Read-only views remain callable while a contract is paused.

---

## Funding Authorization

Funding is **permissionless** and is no longer admin-gated.

### `pool.fund_invoice` / `pool.batch_fund_invoice`

`fund_invoice` has no `require_auth` on its caller
(`contracts/pool/src/lib.rs:1148-1154`). `batch_fund_invoice` is equally
permissionless (`:1347-1364`). The caller does not choose the amount, the
recipient, or the discount; those come from the invoice's own listed terms.
The pool enforces, in order:

1. The invoice has not already been funded by this pool
   (`DataKey::FundedInvoice` guard, `:1173-1176`).
2. Invoice status is `Listed` (`:1180-1187`).
3. Issuer **and** buyer are still verified in the registry
   (`:1189-1212`). This is the last verification check before capital is
   committed; verification is not re-checked after funding.
4. The invoice's funding asset matches the pool's asset (`:1214-1224`).
5. `funded_amount = face_value * (10000 - discount_bps) / 10000` is non-zero
   (`:1229-1235`).
6. Available liquidity (`TotalDeposits - TotalFunded`) covers the amount
   (`:1237-1246`).
7. Post-funding utilization does not exceed `max_utilization_bps`
   (`:1248-1257`).

Pool state is committed before cross-contract calls, then the pool invokes
`escrow.lock` and `invoice.mark_funded` (`:1259-1295`). Both of those calls
authorize **the pool**, so a permissionless caller cannot fake the funding
record or redirect the tokens. `batch_fund_invoice` processes an ordered list,
caps the batch at 50, and stops at the first capacity shortfall
(`:1300-1364`); eligibility errors revert the whole batch.

The regression test `test_fund_invoice_is_permissionless`
(`contracts/pool/src/test.rs:1495`) clears all auths and asserts funding still
succeeds.

### Trust implications

- **Any account can allocate LP capital.** LPs have no on-chain veto over
  which eligible invoices are funded. The only on-chain brake is the pool
  admin's `pause` (and the pre-existing utilization/liquidity caps).
- **Callers choose priority when capacity is scarce.** Whoever submits first
  (or supplies the first entries of a batch) determines which eligible
  invoices consume the remaining liquidity.
- **Eligibility still depends on trusted inputs.** Terms come from the issuer,
  verification comes from the registry admin, and listing requires an
  Underwrite attestation. Permissionless funding does not remove those trust
  assumptions; it removes the admin from the allocation decision.
- **Expiry races remain possible.** Only the issuer or invoice admin can call
  `expire_listing`, but a funder can fund in the same ledger before the
  expiry transaction executes. See [Attack Vectors](#6-front-running-on-invoice-expiry).
- **Funding locks principal in escrow.** `fund_invoice` calls `escrow.lock`
  but does not call `escrow.release_to_issuer`; issuer payout is still not
  wired into the funding path. See
  [LIMITATIONS.md](./LIMITATIONS.md#issuer-release-not-wired-issue-56). The
  caller cannot bypass this, but it means a successful funding transaction
  does not by itself pay the issuer.

---

## Ownership Transfer

Ownership is **per contract**, not protocol-wide.

| Contract | Entry point | Auth | Notes |
|----------|-------------|------|-------|
| Registry | `transfer_ownership(new_admin)` | Current admin + new admin | `contracts/registry/src/lib.rs:747-755`; emits `ownership_transferred` |
| Registry | `transfer_admin(new_admin)` | Current admin only | `:787-794`; intentionally bypasses dual auth and emits `admin_transferred` |
| Invoice | `transfer_ownership(new_admin)` | Current admin + new admin | `contracts/invoice/src/lib.rs:2631-2643` |
| Pool | `transfer_ownership(new_admin)` | Current admin + new admin | `contracts/pool/src/lib.rs:384-393` |
| Escrow | none | — | Admin is fixed at `initialize`; there is no transfer or rotation entry point |
| Pool factory | none | — | Admin is fixed at `initialize`; there is no transfer or rotation entry point |

The dual-auth rule prevents the current admin from installing an address that
never consented. It does **not** protect against a compromised current admin
pairing with a compromised or attacker-controlled new admin. The registry's
`transfer_admin` is a deliberate single-signer escape hatch; integrators that
require dual-auth safety must never use it (`test_transfer_admin_bypasses_transfer_ownership_dual_auth`,
`contracts/registry/src/test.rs:1221`).

There is no atomic multi-contract ownership rotation. Each admin must rotate
its own contract, and escrow/factory admins cannot rotate at all. Losing those
keys permanently removes the ability to pause, upgrade, or administer those
instances.

Transfers on registry, invoice, and pool are pause-guarded and revert while
that contract is paused. Escrow and factory have no transfer path to guard.

---

## Pause Controls

Pause is implemented by the shared `clearledger-pause` crate
(`contracts/pause/src/lib.rs:63-87`). The flag lives in **each contract's own
instance storage** under `PauseState::Paused`; it is not a global flag shared
across the protocol. An absent flag means live. Pausing the registry does not
pause the invoice, escrow, pool, or factory contracts. The per-instance scope
is pinned by `test_pause_is_scoped_to_one_instance`
(`contracts/registry/src/test.rs:2523`,
`contracts/escrow/src/test.rs:1507`).

| Contract | `pause` / `unpause` auth | Guarded state-changing entry points | Not guarded while paused |
|----------|--------------------------|--------------------------------------|--------------------------|
| Registry | Stored admin | `initialize` (only meaningful before init), registration, updates, revoke/reinstate/verify, batch registration, `transfer_ownership`, `transfer_admin` | `pause`, `unpause`; read-only views |
| Invoice | Stored admin | `initialize`, wiring, supported assets, `create`/`batch_create`, listing, attestation, `mark_funded`, shipping, confirmation, repayment, defaults, expiry window, `expire_listing`, `transfer_ownership` | `cancel`, `upgrade`, `pause`, `unpause`; read-only views |
| Escrow | Stored admin | `initialize`, `lock`, `release_to_issuer`, `release_to_pool`, `handle_default` | `upgrade`, `pause`, `unpause`; read-only views |
| Pool | Stored admin | `initialize`, deposits, withdrawals, share transfers/approvals, funding, repayment, default handling, parameter changes, `transfer_ownership` | `pause`, `unpause`; read-only views |
| Pool factory | No pause function | — | All factory functions, including deployment |

`unpause` is intentionally never guarded by `require_not_paused`, so a paused
contract can always be resumed by its admin. `pause`/`unpause` emit `paused`
and `unpaused` events with the admin address
(`contracts/{registry,invoice,escrow,pool}/src/events.rs`).

**Semantics and risks:**

- Pause is an admin-only liveness control. It blocks deposits, withdrawals,
  repayments, funding, and default processing, so a malicious or compromised
  admin can freeze LP funds indefinitely. There is no timelock and no
  automatic expiry.
- Pause is not a complete code-change stop. `invoice.upgrade` and
  `escrow.upgrade` run while paused, and `invoice.cancel` also skips the
  guard. Pool factory deployment is not pausable at all.
- Because cross-contract calls execute the callee's guard, pausing one
  contract can make an operation initiated in another contract revert. For
  example, a paused pool or escrow causes the invoice repayment path to fail
  even if the invoice contract is live.
- A protocol-wide emergency stop requires every relevant admin to pause
  independently; no single role can halt the whole system.

---

## Upgrade Powers

Upgrade is **admin-only** and currently exists on two contracts:

| Contract | Function | Auth | Guarded by pause? | Effect |
|----------|----------|------|-------------------|--------|
| Invoice | `upgrade(new_wasm_hash)` | Stored admin | **No** | Calls `env.deployer().update_current_contract_wasm` (`contracts/invoice/src/lib.rs:136-148`) |
| Escrow | `upgrade(new_wasm_hash)` | Stored admin | **No** | Replaces escrow code (`contracts/escrow/src/lib.rs:125-137`) |
| Registry | none | — | — | No upgrade entry point |
| Pool | none | — | — | No upgrade entry point |
| Pool factory | none | — | — | No upgrade entry point, but the admin chooses the `pool_wasm_hash` used for new deployments (`contracts/pool_factory/src/lib.rs:127-159`) |

There is no dual auth, timelock, delay, or pause guard on either upgrade.
Once the stored admin signs, the new Wasm takes effect for subsequent calls
and can change any storage semantics, authorization rule, or fund-flow
behaviour. In particular:

- An invoice-admin upgrade can bypass every documented invoice invariant.
- An escrow-admin upgrade can move or release all currently locked funds,
  because the escrow contract holds those tokens.
- Pausing an invoice or escrow does not prevent its admin from upgrading it.

Registry, pool, and factory instances are not upgradeable through a contract
entry point, but the factory admin controls the code hash used for every new
pool it deploys. Upgrade events are emitted as `upgraded`
(`contracts/{invoice,escrow}/src/events.rs`).

---

## Attack Vectors

### 1. Admin Key Compromise

**Risk:** Each contract has a single admin key. Compromise of a registry,
invoice, escrow, pool, or factory admin gives that attacker the powers listed
in [Blast Radius](#blast-radius-per-admin-key). The highest-impact paths are
escrow upgrade (locked funds) and invoice upgrade/rewiring (repayment flows).

**Mitigation:**
- Registry/invoice/pool ownership transfer needs the new admin too, so a
  stolen key cannot unilaterally hand ownership to an attacker without that
  new key signing. Registry's `transfer_admin` is an explicit exception.
- On the pool factory the admin is the asset-registration trust boundary: it is
  copied into every newly deployed pool as that pool's admin and treasury. A
  factory ownership transfer therefore needs both signatures, and once it lands
  the old key can no longer register assets or move ownership again.
- Pool funding is permissionless, so compromising the pool admin does **not**
  let the attacker choose which invoices receive capital; they can only pause,
  change parameters, or transfer ownership.
- Escrow `handle_default` accepts either the pool or the escrow admin, but it
  always returns funds to the pool, never to the caller.
- **Gap:** There is no multisig, timelock, or dual control for pause or
  upgrade. Escrow and factory admins cannot be rotated at all.

### 2. Cross-Contract Spoofing and Wiring

**Risk:** A malicious contract could try to impersonate the pool, invoice, or
escrow in a cross-contract call, or an admin could rewire a contract to a
hostile address.

**Mitigation:**
- `mark_funded` checks the caller-supplied pool against the configured pool
  before and after `require_auth`, and only the configured pool can record
  funding (`contracts/invoice/src/lib.rs:1308-1332`).
- Pool repayment/default entry points require the configured invoice
  contract (`contracts/pool/src/lib.rs:1392-1396`, `:1447-1457`,
  `:1519-1523`).
- Escrow `lock`, `release_to_issuer`, and `release_to_pool` require the
  configured pool (`contracts/escrow/src/lib.rs:187-313`).
- Pool, escrow, and factory wiring has no post-initialization setter. Invoice
  wiring can be changed only by the invoice admin, which is a high-trust
  operation; a hostile escrow address can intercept future buyer repayments.

### 3. Dust Attack on Pool Shares

**Risk:** After yield accrues, a tiny deposit can round down to zero shares.

**Mitigation:** `deposit` rejects zero-share deposits with
`PoolError::MinimumDeposit` before transferring tokens
(`contracts/pool/src/lib.rs:565-630`).

### 4. Replay of State Transitions

**Risk:** An attacker could replay funding, repayment, or default calls.

**Mitigation:** Status gates and idempotency keys cover the transitions:
`list_for_financing` requires `Created`; `mark_funded` requires `Listed`;
`mark_shipped` requires `Funded`; `confirm_delivery` requires `Active`;
`repay`/`repay_early` require a repayable status; `trigger_default` requires
`Funded`/`Active`/`Confirmed` and a passed due date. A `FundedInvoice` entry
prevents double-funding, and escrow rejects an existing lock with
`AlreadyLocked`.

#### Attestation replay and domain separation

`submit_attestation` is permissionless, so the signed payload is the entire
trust boundary. The signed preimage is the XDR `AttestationPayload`
(`domain_separator`, `invoice_id`, `risk_score`, `evidence_hash`, `agent_id`,
`nonce`), hashed with keccak256 and signed with secp256k1. Verification
requires:

- the effective domain separator
  `SHA-256(tag || ledger.network_id() || contract_address.to_xdr())`, which
  binds the scheme, the Stellar network passphrase hash, and this exact
  invoice-contract deployment;
- the payload's `invoice_id` to equal the submitted invoice id;
- the recovered pubkey to equal the active agent's registered pubkey for the
  payload's `agent_id`; and
- no attestation to be stored yet for that invoice.

Consequences: a signature cannot be moved to another invoice, another
network, another invoice-contract deployment, another nonce, or another agent
identity. Changing any signed field invalidates the signature, and the
one-attestation-per-invoice storage check blocks byte-for-byte replay. The
nonce is signed but is **not** a per-agent replay ledger; distinct nonces do
not permit a second attestation for the same invoice. The full wire format,
migration note, and resource profile are in
[ATTESTATIONS.md](./ATTESTATIONS.md).

### 5. Integer Over/Underflow

**Risk:** Arithmetic errors in amounts or utilization.

**Mitigation:** Amounts are `u128` with `overflow-checks = true` in the
release profile. Funding scaling, utilization, repayments, withdrawals, and
default accounting use checked arithmetic or documented saturation; see
`contracts/pool/src/lib.rs:1229-1257` and `:1560-1577`.

### 6. Front-Running on Invoice Expiry

**Risk:** A funder can fund a listing in the same ledger as an expiry call.

**Mitigation:** `expire_listing` is **not** permissionless. The caller must
authenticate and be the stored issuer or the invoice admin, and the expiry
window must have elapsed (`contracts/invoice/src/lib.rs:2053-2101`). The race
is therefore between the issuer/admin and a funder, not an arbitrary
attacker. Once the window has elapsed both calls are valid, and whichever
executes first wins; this is accepted under the permissionless funding model.

### 7. Utilization Cap Bypass

**Risk:** An admin or caller sets an unsafe utilization cap, or the cap is
changed without authorization.

**Mitigation:** `set_max_utilization` rejects values above 10,000 bps
(`contracts/pool/src/lib.rs:1914-1919`). **However**, as noted under Pattern
B, the function authenticates a caller-supplied `admin` argument instead of
the stored admin, so any self-authenticating account can change the cap.
Until that is fixed, the cap is not an admin-only protection.

### 8. Batch Registration / Funding Overflow

**Risk:** A large batch exhausts gas or storage.

**Mitigation:** Registry batch registration, invoice `batch_create`,
`batch_list_for_financing`, and pool `batch_fund_invoice` cap input at 50
entries. Batch funding stops on liquidity/utilization exhaustion rather than
partially draining the pool.

### 9. Permissionless Capital Allocation

**Risk:** An attacker (or any LP) can direct available pool capital into
eligible invoices, potentially consuming liquidity ahead of other
participants.

**Mitigation:** Capital can only go to invoices that are listed, verified,
asset-matched, non-zero, within liquidity, and under the cap. The attacker
cannot choose the amount or recipient. There is no on-chain LP vote; this is
the accepted economic trade-off of the permissionless funding model and is
tracked for governance work in [NEW_DESIGN.md](./NEW_DESIGN.md).

### 10. Pause and Upgrade Liveness

**Risk:** An admin can freeze a contract indefinitely, or upgrade invoice/
escrow code while paused and change all invariants.

**Mitigation:** None on-chain beyond the requirement that the admin key signs.
These are documented centralization risks, not solved problems.

### 9. Hostile or Non-Conforming Funding Asset

**Risk:** A pool's configured funding asset is an external SAC contract. If
its `transfer` fails, or if a caller supplies an amount outside the token's
signed `i128` range, a careless pool could commit accounting before the token
call, mint shares without receiving funds, or hand the token a negative amount
through a truncating `as i128` cast.

**Mitigation:**
- `deposit` calls the funding asset's `transfer` before writing shares or
  totals. A failed SAC call reverts the whole invocation, so no shares are
  minted and no accounting changes survive.
- `fund_invoice` writes pool state before its cross-contract calls
  (checks-effects-interactions), but a failed `escrow::lock` transfer reverts
  the entire invocation together with the `FundedInvoice` entry, counters, and
  invoice status. A failed attempt can be retried; a successful one is still
  protected by the `AlreadyFunded` replay guard.
- Every amount passed to a funding-asset `transfer` is checked against
  `i128::MAX` first and rejected with `PoolError::Overflow` rather than being
  truncated to a negative value.
- The pool never reads the funding asset's `balance()`. Accounting is derived
  from the token transfer's success and the pool's own totals, so a malformed
  or lying `balance()` response cannot influence shares, deposits, or funded
  state.

**Trust boundary:** The configured funding asset is assumed to implement SAC /
SEP-41 semantics and to move exactly the requested amount when `transfer`
returns successfully. A token that returns success without moving funds is
outside the pool's ability to detect; the pool deliberately does not add a
post-transfer balance check because that would introduce a new untrusted
read on the hot path.

---

## Centralization Risks

| Risk | Current State | Planned Mitigation |
|------|---------------|--------------------|
| Per-contract single admin | Each contract has one Stellar account; no multisig | Multi-sig (3-of-5) before any Mainnet consideration |
| Permissionless funding without LP governance | Any caller can allocate eligible capital; LPs cannot vote | LP-governed allocation design in [NEW_DESIGN.md](./NEW_DESIGN.md) |
| Per-contract pause | Admin-only, no timelock, not global; invoice `cancel` and both upgrades ignore the flag | Multisig + timelocked operational process |
| Admin-only upgrade | Invoice and escrow admins can replace code immediately; no timelock | Timelocked upgrade + multisig |
| Escrow/factory admin not transferable | No ownership transfer entry point on either contract | Add explicit rotation or accept immutable admin |
| Centralized registry | Registry admin is the sole verifier and can revoke/reinstate | Multisig registry administration / governance |
| Factory controls new pools | Factory admin supplies the pool Wasm and becomes each new pool's admin and treasury | Review/verify Wasm hash and use a governed factory admin |
| Optional agent-registry dependency | Listing requires an attestation from an underwrite agent registry configured by the invoice admin | Multisig configuration + audited agent registry |

### Blast Radius Per Admin Key

| Admin | What They Can Do | What They Cannot Do |
|-------|------------------|---------------------|
| **Registry** | Register/verify/revoke/reinstate profiles; pause registry; transfer ownership (dual) or admin (single) | Move funds; pause/upgrade other contracts; bypass invoice/escrow/pool auth |
| **Invoice** | Rewire pool/escrow/agent-registry; add/remove assets; set expiry window; pause; expire any listed invoice after its window; upgrade code; transfer ownership (dual) | Directly withdraw pool deposits or escrow locks without an upgrade/rewire attack |
| **Escrow** | Pause escrow; upgrade code; force `handle_default` (funds go to pool); no ownership transfer | Send escrowed funds to an arbitrary address using existing entry points; change the stored pool/asset address |
| **Pool** | Pause pool (blocking deposits, withdrawals, repayment, funding); set fee/treasury (≤2000 bps); change utilization cap (currently any caller can too); transfer ownership (dual); no upgrade | Directly withdraw LP principal; mint unbacked shares; change funding eligibility |
| **Pool factory** | Deploy a pool with an arbitrary installed Wasm and itself as admin/treasury; register an existing matching pool; no pause/upgrade | Repoint an already-registered asset; upgrade existing pools; move funds |

Issuers, buyers, and LPs cannot pause, upgrade, or change parameters. An
issuer can only affect its own invoices; a buyer can only repay or confirm; an
LP can only move its own shares. The anonymous caller can trigger permissionless
funding/default/attestation but cannot choose amounts or recipients.

---

## Defence-in-Depth

### Separation of Concerns

- **Registry** holds no funds and cannot call value-moving entry points.
- **Escrow** holds locked USDC but requires the configured pool (or, for
  default recovery, the escrow admin) to move it.
- **Pool** holds LP deposits and repayment proceeds; LP shares are the only
  claim on those deposits.
- **Invoice** is primarily a state machine, but it does move buyer funds
  through escrow during repayment and invokes pool accounting. It does not
  custody funds.
- **Factory** holds no funds; it deploys and indexes pool instances.

### Issuer Provenance on Release

`lock()` records the issuer alongside the locked amount, and
`release_to_issuer()` treats that stored address as the source of truth. The
caller-supplied `issuer` parameter is only an assertion checked for equality;
the token transfer and `released_to_issuer` event always use the stored
address. A mismatch reverts with `InvalidRecipient` before any token transfer
or storage mutation, so the pool cannot redirect escrowed funds to an
arbitrary recipient and a replayed release finds no lock record.

### TTL Management

Every `persistent().set()` is followed by an `extend_ttl()` call with the
workspace thresholds, and instance storage is extended on state-changing
calls. See [STORAGE.md](./STORAGE.md) for the key schema and TTL parameters.

### External Dependencies

The contracts depend on `soroban-sdk`, the Stellar token contract for the
funding asset, and the optional Underwrite agent-registry contract used for
attestations. There are no bridges, oracles, or off-chain relayers in the
core fund flow. The testnet funding asset's issuer can freeze or mint according
to that asset's own rules; that risk is outside the protocol's control.

---

## Out-of-Scope

The following are explicitly outside this threat model:

- **Mainnet operation and guarantees.** This code is testnet-only and
  unaudited. Nothing here should be read as a Mainnet security claim.
- **Stellar protocol-level attacks** (consensus, validator compromise, etc.)
  — report to the Stellar Development Foundation.
- **Funding-asset issuer powers** (freeze, mint, clawback) and other token
  contract behaviour.
- **Freighter / wallet vulnerabilities** — report to the wallet team.
- **Frontend phishing or social engineering.**
- **Off-chain indexer downtime** — contracts function independently of the
  indexer; missed events do not affect fund safety.
- **Testnet-only risks** (Friendbot dust, ephemeral validators, state resets)
  — see [LIMITATIONS.md](./LIMITATIONS.md).

---

## Reporting

Report vulnerabilities through this repository's private security reporting
channel. See [SECURITY.md](../SECURITY.md) for the disclosure policy and
response timeline.
