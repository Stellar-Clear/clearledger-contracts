# Threat Model

> **Updated:** 2025-07-25
> **Applies to:** All four Soroban smart contracts (registry, invoice, escrow, pool)

This document describes the trust assumptions, authentication gates, and
attack surfaces of the ClearLedger protocol. It is intended for security
researchers, integrators, and contributors evaluating the protocol's risk
profile.

---

## Table of Contents

- [Trust Model](#trust-model)
- [Authentication Gates](#authentication-gates)
- [Attack Vectors](#attack-vectors)
- [Centralization Risks](#centralization-risks)
- [Defence-in-Depth](#defence-in-depth)
- [Out-of-Scope](#out-of-scope)

---

## Trust Model

### Roles

| Role | Description | Trust Level |
|------|-------------|-------------|
| **Admin** | Deployer address that calls `initialize()` on each contract. Controls profile management, default triggers, and configuration. | **High** — single point of failure |
| **Issuer** | A verified SME that creates invoices and requests funding. | Low — only self-authenticates |
| **Buyer** | A verified counterparty obligated to repay an invoice at maturity. | Low — only self-authenticates |
| **LP** | Liquidity provider that deposits USDC into a pool. | Low — only self-authenticates |
| **Anonymous caller** | Any Stellar account that can call permissionless functions. | None — no auth required |

### Trust Boundaries

```
  ┌─────────────────────────────────────────────────────┐
  │                  Contract Layer                      │
  │  ┌──────────┐  ┌───────────┐  ┌─────────┐  ┌────┐  │
  │  │ Registry │  │  Invoice  │  │ Escrow  │  │Pool│  │
  │  └────┬─────┘  └─────┬─────┘  └────┬────┘  └─┬──┘  │
  │       │              │              │          │     │
  │       └──────────────┴──────────────┴──────────┘     │
  │                 Trust boundary                        │
  │     (contracts trust each other's registered addr)    │
  └───────────────────────────────────────────────────────┘
            ▲                          ▲
            │                          │
   Verified via registry         Self-authenticated
   (issuer / buyer)              (LP deposits / withdrawals)
```

**Key trust assumption:** All contracts trust the addresses stored during
`initialize()` — if any stored reference is wrong, the caller loses funds
irrevocably.

---

## Authentication Gates

Every state-changing function enforces one of these auth patterns:

### Pattern A — Self-Auth (`address.require_auth()`)

The caller authenticates only themselves. No admin or third party is involved.

| Function | Contract | Who Auths |
|----------|----------|-----------|
| `register_issuer` | Registry | The issuer address |
| `register_buyer` | Registry | The buyer address |
| `update_metadata` | Registry | The profile address |
| `create` | Invoice | The issuer |
| `list_for_financing` | Invoice | The issuer |
| `mark_shipped` | Invoice | The issuer |
| `confirm_delivery` | Invoice | The confirmer (issuer _or_ buyer) |
| `repay` | Invoice | The buyer |
| `check_auth` | Invoice | The supplied address |
| `deposit` | Pool | The LP |
| `withdraw` | Pool | The LP |

### Pattern B — Admin-Auth (`admin.require_auth()`)

Only the stored admin address may call this function.

| Function | Contract | Why |
|----------|----------|-----|
| `revoke` | Registry | Prevents unauthorised removal of profiles |
| `verify_profile` | Registry | Controls verification status |
| `batch_register_issuers` | Registry | Batch operations gated by admin |
| `set_pool_contract` | Invoice | Wiring a new pool changes fund flows |
| `trigger_default` | Invoice | Declaring a default is a state change with fund consequences |
| `set_expiry_window` | Invoice | Configuration parameter |
| `expire_listing` | Invoice | Admin fallback path |
| `fund_invoice` | Pool | (*currently*) Capital allocation from pool |
| `set_max_utilization` | Pool | Risk parameter |
| `transfer_ownership` | All | Changes the admin key |

### Pattern C — Cross-Contract Auth (`contract.require_auth()`)

One contract authenticates as the caller of another contract's function.

| Function | Contract | Authenticated Caller | Why |
|----------|----------|---------------------|-----|
| `mark_funded` | Invoice | The pool contract | Prevents faking funding records |
| `receive_repayment` | Pool | The invoice contract | Prevents faking repayment events |
| `receive_repayment_with_refund` | Pool | The invoice contract | Same as above |
| `handle_default` | Pool | The invoice contract | Prevents premature default recovery |
| `lock` | Escrow | The pool contract | Only the pool can lock funds |
| `release_to_issuer` | Escrow | The pool contract | Only the pool can release to issuer |
| `release_to_pool` | Escrow | The pool contract | Only the pool can reclaim escrowed funds |

### Pattern D — Dual-Auth (two `require_auth()` calls)

Two distinct parties must both sign.

| Function | Contract | Signers |
|----------|----------|---------|
| `initialize` | All | The incoming admin (prevents assigning an uncontrolled address) |
| `transfer_ownership` | All | Current admin + new admin (prevents accidental transfers) |

### Pattern E — Permissionless (no auth)

Any account can call these.

| Function | Contract | Notes |
|----------|----------|-------|
| `get_profile`, `is_verified`, `get_verification_status`, `get_admin`, `get_profile_count`, `list_profiles` | Registry | Read-only views |
| `get`, `get_status`, `get_face_value`, `get_funded_amount`, `get_discount_bps`, `get_funding_asset`, `get_issuer`, `get_by_status`, `get_by_issuer`, `get_by_buyer`, `get_invoice_count_by_issuer`, `get_invoice_count_by_buyer`, `get_counts`, `get_expiry_window` | Invoice | Read-only views |
| `get_locked`, `get_history`, `get_history_page` | Escrow | Read-only views; retained history is bounded |
| `get_stats`, `get_lp_position`, `get_utilization_rate`, `get_usdc_asset` | Pool | Read-only views |
| `trigger_default` | Invoice | Callable by any account once `now >= due_date` |
| `expire_listing` | Invoice | Also callable by issuer (Pattern A) |

---

## Attack Vectors

### 1. Admin Key Compromise

**Risk:** The admin key controls profile revocations, defaults, pool funding,
and ownership transfer. Compromise of this key has a high blast radius.

**Mitigation:**
- `transfer_ownership` requires dual auth (current + new admin) — a stolen key
  cannot transfer ownership to the attacker without the new key also signing.
- The escrow `handle_default` has an alternative caller (pool contract), so
  admin compromise does not grant unilateral fund recovery.
- **Planned:** Multi-sig admin (see [Roadmap section in README](../README.md#known-centralization-risks--roadmap)).

### 2. Cross-Contract Spoofing

**Risk:** A malicious contract deployed at the same address as a legitimate
contract could fake `require_auth()` calls.

**Mitigation:**
- Contract addresses are hardcoded during `initialize()` and cannot be changed
  without admin auth.
- The invoice contract checks the pool address on every `mark_funded` call,
  and the pool checks the invoice address on `receive_repayment` and `handle_default`.

### 3. Dust-Attack on Pool Shares

**Risk:** After the pool accrues yield, the share price rises above 1.0.
A tiny deposit can round down to 0 shares while the USDC is still transferred
into the pool, effectively donating the depositor's funds to existing LPs.

**Mitigation:** The `deposit()` function rejects any deposit that would mint
0 shares with `PoolError::MinimumDeposit (#14)`. The check runs before the
token transfer, so no funds leave the depositor on the rejection path.

### 4. Replay of State Transitions

**Risk:** An attacker could try to call state-transition functions multiple
times on the same invoice (e.g., double-funding).

**Mitigation:** Every state transition in the invoice contract checks the
current status before proceeding. `list_for_financing` only works on `Created`
invoices; `mark_funded` only on `Listed`; `mark_shipped` only on `Funded`;
`confirm_delivery` only on `Active`; `repay` only on `Confirmed`; and
`trigger_default` only on `Funded`/`Active`/`Confirmed`. Once the status
advances, the previous function cannot be called again.

Additionally, escrow's `lock()` function rejects re-locking an already-locked
invoice with `EscrowError::AlreadyLocked`.

### 5. Integer Over/Underflow

**Risk:** Arithmetic errors in amount calculations could lead to incorrect
fund allocations.

**Mitigation:**
- All contracts use Rust `u128` for amounts. The release profile enables
  `overflow-checks = true` (see [Cargo.toml](../Cargo.toml)).
- `receive_repayment` validates `amount >= funded_amount`.
- `receive_repayment_with_refund` validates `refund <= amount - funded_amount`.
- `withdraw` validates `shares <= lp_shares` and `usdc_to_return <= available`.
- `handle_default` in the pool uses `saturating_sub` for deposit reduction.
- The invoice's `repay` functions use `saturating_sub` for discount calculations.

### 6. Front-Running on Invoice Expiry

**Risk:** An attacker could front-run an `expire_listing` call to fund an
invoice that would otherwise expire.

**Mitigation:** `expire_listing` only succeeds when `current_time > listed_at
+ expiry_window`. If the expiry window has passed, anyone can call
`expire_listing`, but a funder could still call `fund_invoice` in the same
ledger (before `expire_listing`). This is an accepted race condition given
the current permissionless funding model.

### 7. Utilization Cap Bypass

**Risk:** An admin could set `max_utilization_bps` above 10000 (100%) to
allow over-leveraging the pool.

**Mitigation:** `set_max_utilization` validates `new_cap_bps <= 10000` and
rejects values above 10000.

### 8. Batch Registration Overflow

**Risk:** An attacker could submit a very large batch of issuers to exhaust
gas or storage.

**Mitigation:** `batch_register_issuers` limits entries to `≤ 50` and
panics with `RegistryError::BatchSizeExceeded` otherwise.

---

## Centralization Risks

| Risk | Current State | Planned Mitigation |
|------|---------------|--------------------|
| Single admin key | Admin is a single Stellar address | Multi-sig (3-of-5) before mainnet |
| Admin-gated `fund_invoice` | Pool funding requires admin auth | LP-governed allocation (see [README](../README.md#known-centralization-risks--roadmap)) |
| No emergency pause | No circuit breaker exists | `admin_pause() / admin_unpause()` before mainnet |
| Centralised registry | Admin controls all registrations/revocations | On-chain governance (longer-term) |
| Single oracle for verification | Registry is the sole source of truth for identity | Multi-sig registry admin |

### Blast Radius: Admin Key

If the admin key is compromised or malicious:

| What They Can Do | What They Cannot Do |
|-----------------|---------------------|
| Revoke any profile | Transfer admin to a non-consenting address (dual auth) |
| Register fraudulent profiles | Directly withdraw LP funds |
| Trigger false defaults | Bypass the utilization cap (>10000 bps) |
| Set a new pool contract | Mint unbacked LP shares |
| Change pool utilization cap (≤10000 bps) | Release escrowed funds to themselves (only pool contract can) |
| Expire listings | |

---

## Defence-in-Depth

### Separation of Concerns

- **Registry** has no access to any funds.
- **Escrow** holds USDC but cannot release it without the pool contract.
- **Pool** manages accounting but does not hold USDC directly (it holds a
  balance from deposits, but escrow holds funded amounts).
- **Invoice** is a pure state machine with no fund custody.

### TTL Management

Every `persistent().set()` is immediately followed by `extend_ttl()` with
`live_until_ledger` = 100 and `seq` = 2_000_000. Instance storage is also
extended on every state-changing call via `extend_instance_ttl()`.

See [STORAGE.md](./STORAGE.md) for the full storage key schema and TTL
parameters.

### No External Dependencies Beyond Stellar

The contracts depend only on `soroban-sdk` and each other. There are no
oracle integrations, bridge contracts, or off-chain relayers whose compromise
could affect on-chain state.

---

## Out-of-Scope

The following are explicitly outside this threat model:

- **Stellar protocol-level attacks** (consensus, validator compromise, etc.)
  — report to [Stellar Development Foundation](https://stellar.org).
- **Freighter / wallet vulnerabilities** — report to the respective wallet team.
- **Frontend phishing or social engineering** — users are responsible for
  verifying transaction details before signing.
- **Off-chain indexer downtime** — the contracts function independently of
  the indexer; missed events do not affect fund safety.
- **Testnet-only risks** (Friendbot dust, ephemeral validators) — see
  [LIMITATIONS.md](./LIMITATIONS.md).

---

## Reporting

Report vulnerabilities through this repository’s private security reporting channel. See [SECURITY.md](../SECURITY.md) for our disclosure policy and response timeline.
