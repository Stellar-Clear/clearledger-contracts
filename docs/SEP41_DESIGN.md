# SEP-41 LP Share Token: Design and Behavior

> **Testnet only.** This document describes the current `contracts/pool`
> implementation as deployed on Stellar Testnet. It is not a Mainnet
> specification, compatibility guarantee, or security audit. Do not treat
> Testnet LP shares as redeemable value.

This document covers the SEP-41 surface implemented directly by
`PoolContract` in `contracts/pool/src/lib.rs`: the methods it exposes, how
share metadata is configured, which callers must authorize each action, how
allowances expire, and where the implementation differs from the SEP-41 token
interface.

## Overview

The pool does not deploy a companion token contract. LP shares are the pool's
own `DataKey::LPShares(Address)` balances, and `PoolContract` implements the
SEP-41 entry points directly. `DataKey::TotalShares` is the token's total
supply.

This keeps issuance and redemption in the same execution context as deposits
and withdrawals:

- `deposit()` mints shares through the internal `_mint` helper and emits
  `lp_deposited`.
- `withdraw()` burns shares through the internal `_burn` helper and emits
  `lp_withdrawn`.
- `transfer()` and `transfer_from()` move existing balances without minting,
  burning, or changing pool accounting.

There are **no public `mint`, `burn`, or `burn_from` entry points**. Supply only
changes through `deposit()` and `withdraw()`.

### Previewing a Withdrawal
`preview_withdraw(shares)` is the read-only ABI method for the burn formula above. It returns a `WithdrawPreview`:

| Field | Meaning |
| --- | --- |
| `usdc_out` | USDC stroops `withdraw` would pay for `shares`, using the same floor division as the write path. |
| `available_liquidity` | Pool USDC currently available to withdraw (`total_deposits - total_funded`). |
| `liquidity_ok` | `true` when `available_liquidity >= usdc_out`; otherwise a real withdrawal of those shares would fail with `InsufficientLiquidity`. |

The preview requires no authorization and performs no storage writes, event emission, or TTL extension. It quotes pool-wide share math only; callers should also check the LP's share balance (`balance` / `get_lp_position`) and the pool's pause state before submitting a withdrawal. `withdraw` and `preview_withdraw` share one conversion helper (`usdc_for_withdraw`) so their rounding and dust guard cannot drift.

## Interface Boundary Decision

### Recommendation
**Direct Implementation:** The `PoolContract` should implement the SEP-41 interface directly, rather than deploying a separate companion token contract.

### Rationale
- **Simplicity & Gas Savings:** A single contract avoids the complexity of cross-contract calls between the pool and its companion token contract. Minting and burning shares during deposits and withdrawals happens instantly within the same execution context.
- **State Re-use:** We already store `DataKey::LPShares(Address)` and `DataKey::TotalShares`. Implementing SEP-41 directly allows us to reuse these exact storage keys for `balance` and `total_supply`, bridging the gap seamlessly without duplicating state.
- **Standard Precedent:** Other Soroban vault and liquidity pool patterns (such as standard AMMs) typically implement the token interface directly on the pool contract.


## Supported interface

`PoolContract` implements the following SEP-41 methods from
`contracts/pool/src/lib.rs`.

| Method | Signature | Behavior | Authorization |
|---|---|---|---|
| `allowance` | `allowance(from: Address, spender: Address) -> i128` | Returns the remaining live grant, or `0` if absent or expired. | None |
| `approve` | `approve(from: Address, spender: Address, amount: i128, expiration_ledger: u32)` | Overwrites the `(from, spender)` grant; `amount = 0` revokes it. | `from` |
| `balance` | `balance(id: Address) -> i128` | Returns `DataKey::LPShares(id)` cast to `i128`, or `0` when absent. | None |
| `decimals` | `decimals() -> u32` | Returns the instance's configured share decimals, or `7` when unset. | None |
| `name` | `name() -> String` | Returns the share-token name configured at `initialize`. | None |
| `symbol` | `symbol() -> String` | Returns the share-token symbol configured at `initialize`. | None |
| `total_supply` | `total_supply() -> i128` | Returns `DataKey::TotalShares` cast to `i128`, or `0` when unset. | None |
| `transfer` | `transfer(from: Address, to: Address, amount: i128)` | Moves shares from `from` to `to`. Standard SEP-41 transfer entry point. | `from` |
| `transfer_from` | `transfer_from(spender: Address, from: Address, to: Address, amount: i128)` | Moves shares using `spender`'s live allowance and decrements it. | `spender` |

The pool also keeps a non-standard legacy alias:

| Method | Signature | Behavior | Authorization |
|---|---|---|---|
| `transfer_shares` | `transfer_shares(from: Address, to: Address, amount: i128)` | Same checks, movement, and event as `transfer`. Kept for pre-SEP-41 integrators. New code must use `transfer`. | `from` |

### Not implemented

- **`burn` / `burn_from`**: absent from the public interface. Shares can only be
  burned by the owner calling `withdraw()`. A generic SEP-41 client that calls
  `burn` or `burn_from` against the pool address will fail because no such
  entry point exists.
- **Public `mint`**: absent by design; SEP-41 does not define a minting method
  and issuance is restricted to `deposit()`.
- **Admin share movement**: the admin has no override to transfer, burn, or
  mint another holder's shares. Pausing blocks state-changing share operations
  for everyone, but it cannot move balances.

## Metadata: `decimals`, `name`, `symbol`

All three values are per-instance configuration written once by
`initialize()` in `contracts/pool/src/lib.rs`:

```rust
initialize(
    env,
    admin,
    invoice_contract,
    escrow_contract,
    funding_asset,
    registry_contract,
    treasury,
    min_initial_deposit,
    share_name: String,
    share_symbol: String,
    share_decimals: u32,
)
```

| Value | Storage key | Read behavior | Validation / mutability |
|---|---|---|---|
| `decimals` | `DataKey::ShareDecimals` | Returns the stored `u32`; falls back to `DEFAULT_SHARE_DECIMALS = 7` when the key is absent. Does **not** panic before initialization. | No range check and no comparison against the funding asset. Immutable after `initialize`. |
| `name` | `DataKey::ShareName` | Returns the stored `String`; panics with `PoolError::NotInitialized` (`#2`) when absent. | Rejects an empty string at `initialize` with `PoolError::InvalidConfiguration` (`#15`). Immutable after `initialize`. |
| `symbol` | `DataKey::ShareSymbol` | Returns the stored `String`; panics with `PoolError::NotInitialized` (`#2`) when absent. | Rejects an empty string at `initialize` with `PoolError::InvalidConfiguration` (`#15`). Immutable after `initialize`. |

### Decimal semantics

Shares are issued in the **funding asset's base units**. On the first deposit,
shares are minted 1:1 with the deposited base units; later deposits use the
proportional formula below. `decimals()` is presentation metadata only and does
not scale any arithmetic.

`docs/SEP41_DESIGN.md` requires the configured `share_decimals` to match the
funding asset's own decimals, but `initialize()` does **not** verify this. Under
the `pool_factory` model, each deploy is responsible for passing the correct
value. A pool funding an asset with non-7-decimal precision that omits
`share_decimals` will report the `7` fallback and wallets will render its share
balances at the wrong precision.

`name` and `symbol` are likewise set per instance so two pools funding
different assets do not render identical share tokens. They have no setters;
changing them requires a new deployment.

## Authorization matrix

| Entry point | Caller that must authorize | Notes |
|---|---|---|
| `initialize` | `admin` | Also cross-checks that `escrow_contract.get_usdc_asset()` equals `funding_asset`; escrow must be initialized first. |
| `deposit` | `lp` | Pulls the funding asset from `lp`. |
| `withdraw` | `lp` | Burns `lp`'s shares and pays the funding asset to `lp`. |
| `transfer` | `from` | Standard SEP-41 transfer; recipient does not authorize. |
| `transfer_shares` | `from` | Legacy alias with the same auth rule as `transfer`. |
| `approve` | `from` | The share owner grants the spender. |
| `transfer_from` | `spender` | The owner already authorized by calling `approve`; the owner does not sign the spend. |
| `allowance` | None | Read-only. |
| `balance` | None | Read-only. |
| `total_supply` | None | Read-only. |
| `decimals` / `name` / `symbol` | None | Read-only metadata. |
| `pause` / `unpause` | stored `admin` | `pause` blocks state-changing pool methods; reads remain available. `unpause` is intentionally not pause-guarded. |
| `set_max_utilization` / `set_protocol_fee` | stored `admin` | Pool configuration, not share-token methods. |

While the pool is paused, `transfer`, `transfer_shares`, `approve`, and
`transfer_from` revert through `clearledger_pause::require_not_paused` with
`ContractPaused`. `balance`, `allowance`, `total_supply`, and metadata reads do
not take the pause guard.

## Share movement semantics

`transfer` and `transfer_from` share `move_shares` in
`contracts/pool/src/lib.rs`, so they enforce the same balance rules:

1. The sender must have a `DataKey::LPShares(from)` entry. A missing entry,
   including after all shares were previously moved out, panics with
   `PoolError::NoShares` (`#6`) rather than `InsufficientBalance`.
2. `amount` must be positive. Zero and negative amounts panic with
   `PoolError::InvalidAmount` (`#4`).
3. The sender must hold at least `amount`; otherwise the call panics with
   `PoolError::InsufficientBalance` (`#23`).
4. A self-transfer (`from == to`) leaves balances unchanged. `transfer` still
   succeeds and emits the transfer event.
5. A transfer to a new address creates that address's `LPShares` entry.
6. Transfers never change `TotalShares`, `TotalDeposits`, `TotalFunded`, yield
   accounting, or the share price.

`transfer_from` additionally requires a live allowance of at least `amount`
(see below). It decrements the allowance before calling `move_shares`.
Soroban invocation atomicity means that if a later balance check panics, the
allowance decrement is rolled back with the rest of the transaction.

Self-transfers through `transfer_from` are a known quirk: the allowance is
decremented, `move_shares` returns early because `from == to`, and a transfer
event is still emitted. No shares change hands.

## Allowance expiry semantics

Allowances are stored per `(owner, spender)` pair in persistent storage:

```text
DataKey::Allowance(from: Address, spender: Address)
    -> ShareAllowance { amount: i128, expiration_ledger: u32 }
```

`ShareAllowance` is defined in `contracts/pool/src/types.rs`; the read and
write logic is `approve`, `allowance`, `transfer_from`, and `live_allowance` in
`contracts/pool/src/lib.rs`.

### Ledger sequence, not timestamp

`expiration_ledger` is a **Stellar ledger sequence number**, compared against
`env.ledger().sequence()`. It is not a Unix timestamp and there is no
time-based allowance variant.

The grant is live through its expiration ledger and dead from the next ledger:

```text
live when expiration_ledger >= current ledger sequence
expired when expiration_ledger <  current ledger sequence
```

Consequences:

- `allowance()` returns the full remaining amount at
  `expiration_ledger == current sequence`.
- At `expiration_ledger + 1`, `allowance()` returns `0` without any cleanup
  transaction.
- `transfer_from` against an expired grant panics with
  `PoolError::InsufficientAllowance` (`#24`), the same error as never having a
  grant.

### `approve` rules

- A second `approve` **overwrites** the previous grant; it does not add to it.
- A non-zero `amount` requires `expiration_ledger > current sequence`.
  `expiration_ledger <= current sequence` panics with
  `PoolError::InvalidExpiration` (`#25`). This is slightly stricter than the
  SEP-41 text, which permits an expiry equal to the current ledger.
- A negative `amount` panics with `PoolError::InvalidAmount` (`#4`).
- `amount = 0` is the revoke signal. It removes the storage entry, accepts any
  `expiration_ledger` value (including `0` or a past ledger), and emits
  `allowance_approved` with the supplied expiry.
- Every successful `approve`, including a revoke, emits
  `allowance_approved`.
- The allowance entry's storage TTL is extended on write. Expiry is enforced by
  contract logic; storage TTL and `expiration_ledger` are independent. If the
  persistent entry lapses from rent before `expiration_ledger`, the grant is
  effectively lost and reads `0`.

### `transfer_from` rules

- Only `spender` authorizes.
- `amount` must be positive; zero or negative panics with
  `PoolError::InvalidAmount` (`#4`) before any allowance is consumed.
- A missing, expired, or insufficient allowance panics with
  `PoolError::InsufficientAllowance` (`#24`).
- A partial spend stores the reduced amount and keeps the original expiration
  ledger.
- Spending the exact remaining amount removes the allowance entry.
- Allowance checks happen before the share balance checks, but the whole call
  is atomic.

### Allowance lifecycle example

Assume the current ledger sequence is `100`:

| Action | Result |
|---|---|
| `approve(owner, spender, 500, 200)` | Grant of `500` shares through ledger `200`. |
| `allowance(owner, spender)` at sequence `200` | `500` (last live ledger). |
| `allowance(owner, spender)` at sequence `201` | `0` (expired). |
| `transfer_from(spender, owner, to, 1)` at sequence `201` | Panics with `InsufficientAllowance` (`#24`). |
| `approve(owner, spender, 0, 0)` at any sequence | Revokes the grant immediately. |
| `approve(owner, spender, 1, 100)` at sequence `100` | Panics with `InvalidExpiration` (`#25`). |

## Errors

Share-token failures use `PoolError` from `contracts/pool/src/errors.rs`:

| Error | Code | Share-token use |
|---|---|---|
| `NotInitialized` | `#2` | `name`, `symbol`, `transfer`, `transfer_shares`, `approve`, and `transfer_from` before initialization. |
| `InvalidAmount` | `#4` | Zero or negative `transfer` / `transfer_shares` / `transfer_from`; negative `approve`; zero `deposit` or `withdraw`; first deposit below `min_initial_deposit`. |
| `NoShares` | `#6` | Sender has no `LPShares` entry. |
| `InsufficientShares` | `#7` | `withdraw` amount exceeds the caller's balance. |
| `InsufficientBalance` | `#23` | `transfer` / `transfer_from` amount exceeds the sender's balance. |
| `InsufficientAllowance` | `#24` | No live allowance, or the live allowance is below the requested amount. |
| `InvalidExpiration` | `#25` | Non-zero `approve` with `expiration_ledger <= current sequence`. |
| `MinimumDeposit` | `#14` | Pool-level dust guard for deposits and withdrawals, not a token-interface error. |
| `InvalidConfiguration` | `#15` | Empty `share_name` or `share_symbol` at `initialize`. |

`ContractPaused` comes from the shared pause crate rather than `PoolError`.

## Events and indexer notes

The pool emits share events from `contracts/pool/src/events.rs`.

| Event | Topics | Data | Emitted by |
|---|---|---|---|
| `transfer` | `("transfer", from)` | `(to, amount)` | `transfer`, `transfer_shares`, `transfer_from` |
| `allowance_approved` | `("allowance_approved", from)` | `(spender, amount, expiration_ledger)` | `approve` |
| `lp_deposited` | `("lp_deposited", lp)` | `(usdc_amount, shares_issued)` | `deposit` |
| `lp_withdrawn` | `("lp_withdrawn", lp)` | `(usdc_amount, shares_burned)` | `withdraw` |

Indexers must not assume the canonical SEP-41 event encoding:

- SEP-41's standard transfer event is topics `("transfer", from, to)` with data
  `amount`. This pool publishes `to` and `amount` together in the data field.
- SEP-41's standard approval event is named `approve` with topics
  `("approve", from, spender)` and data `(amount, expiration_ledger)`. This pool
  emits `allowance_approved` with a different topic/data layout.
- Mint and burn events are not emitted. `deposit` and `withdraw` use
  `lp_deposited` and `lp_withdrawn` instead, so a token indexer that derives
  supply only from `mint` / `burn` events will miss supply changes.

See `docs/EVENTS.md` for the catalog-level description.

## Mint and redeem math

The token surface does not change the pool's pricing formula.

### Minting on deposit

```text
shares_to_issue = usdc_amount * total_shares / total_deposits
```

If `total_shares == 0` or `total_deposits == 0`, shares are issued 1:1 with
`usdc_amount` in funding-asset base units. An empty pool rejects a first deposit
below its configured `min_initial_deposit` with `InvalidAmount` (`#4`).
Division truncates; on a non-empty pool a deposit that would mint zero shares
is rejected with `MinimumDeposit` (`#14`) rather than burning value. See
`shares_for_deposit` and `deposit` in `contracts/pool/src/lib.rs`.

### Burning on withdraw

```text
usdc_out = shares_to_burn * total_deposits / total_shares
```

A withdrawal that would return zero USDC is rejected with `MinimumDeposit`
(`#14`). See `withdraw` in `contracts/pool/src/lib.rs`.

## Known deviations and limitations

These are current Testnet implementation facts, not Mainnet guarantees.

1. **No public `burn` or `burn_from`.** Shares are burned only by `withdraw`.
2. **No public `mint`.** Issuance is restricted to `deposit`.
3. **Non-standard event encoding.** Transfer and approval events do not use the
   canonical SEP-41 topic/data layout, and no `mint` / `burn` events are
   emitted (see above).
4. **`approve` rejects `expiration_ledger == current sequence`.** The SEP-41
   text permits equality for a non-zero grant; this implementation rejects it.
   Reads treat equality as live.
5. **Expired allowances are not pruned on read.** `allowance()` returns `0`,
   but the persistent entry remains until it is overwritten, fully spent,
   revoked, or lapses from storage rent.
6. **Storage TTL and semantic expiry are independent.** An allowance whose
   persistent entry lapses from rent before `expiration_ledger` is lost.
7. **Zero-amount transfers are rejected.** `transfer`, `transfer_shares`, and
   `transfer_from` return `InvalidAmount` instead of performing a no-op.
8. **Self `transfer_from` consumes allowance without moving shares.** The
   allowance is decremented and a transfer event is emitted even though
   `move_shares` returns early for `from == to`.
9. **`decimals` is not validated against the funding asset.** The deployer must
   pass the correct `share_decimals`; unset instances fall back to `7`.
10. **Metadata is immutable.** There is no setter for `name`, `symbol`, or
    `decimals`; a correction requires a new pool deployment.
11. **`balance` and `total_supply` cast `u128` to `i128`.** There is no explicit
    bound check; a supply above `i128::MAX` would be misreported.
12. **Pause scope.** `pause()` blocks `transfer`, `transfer_shares`, `approve`,
    and `transfer_from`, but not read-only views or `unpause`.

## Tests

The behavior above is pinned by tests in `contracts/pool/src/test.rs`,
including:

- `test_sep41_balance_and_total_supply`
- `test_approve_sets_allowance_until_expiration`
- `test_transfer_from_decrements_allowance`
- `test_transfer_from_fails_once_allowance_exhausted`
- `test_allowance_reads_zero_after_expiration`
- `test_transfer_from_rejects_expired_allowance`
- `test_approve_with_zero_amount_revokes_allowance`
- `test_approve_with_expiration_in_the_past_panics`
- `test_transfer_from_rejects_spender_without_grant`
- `test_approve_emits_allowance_approved_event`
- `test_share_metadata_comes_from_initialize`
- `test_share_metadata_is_scoped_to_the_pool_instance`
- `test_share_metadata_unreadable_before_initialize`
- `test_sep41_generic_token_client_transfer_moves_shares`
- `test_sep41_generic_token_client_transfer_requires_from_auth`
- `test_transfer_insufficient_balance_panics_via_generic_client`
- `test_transfer_from_address_without_shares_panics`
- `test_transfer_zero_amount_panics_via_generic_client`
- `test_transfer_negative_amount_panics_via_generic_client`
- `test_transfer_and_transfer_from_emit_standard_transfer_event`
- `test_transfer_matches_legacy_transfer_shares`
- `test_deposit_mints_through_shared_helper_without_changing_behaviour`
- `test_share_supply_stays_consistent_across_mint_and_burn`

Related references:

- Storage keys: `docs/STORAGE.md` (`ShareName`, `ShareSymbol`, `ShareDecimals`,
  `Allowance`).
- Event catalog: `docs/EVENTS.md`.
- Design rationale and factory scoping: `docs/NEW_DESIGN.md`,
  `docs/POOL_FACTORY_MIGRATION.md`.
