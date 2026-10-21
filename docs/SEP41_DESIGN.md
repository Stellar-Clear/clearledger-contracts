# SEP-41 LP Share Token Design

## Overview
This document outlines the design and implementation path for migrating the Pool Contract's LP share accounting to a SEP-41 compliant fungible token. Currently, LP shares are tracked internally via `DataKey::LPShares(Address)` and `DataKey::TotalShares`. By adopting the SEP-41 standard, the LP share becomes a fully composable token that can be held in Stellar wallets, traded on DEXs, or used as collateral elsewhere.

## Mint/Redeem Ratio Math
The token layer changes how shares are represented (via SEP-41 interfaces) and stored, but **it does not change the core pricing formula**.

### Minting on Deposit
When an LP deposits USDC, the pool will mint new SEP-41 LP share tokens to their address based on the current pool valuation:
`shares_to_issue = usdc_amount * total_shares / total_deposits`
*(Note: If `total_shares == 0` or `total_deposits == 0`, `shares_to_issue = usdc_amount`)*

### Burning on Withdraw
When an LP withdraws USDC, they redeem their SEP-41 LP share tokens (which are then burned), receiving USDC proportionally:
`usdc_out = shares_to_burn * total_deposits / total_shares`

These exact formulas, already present in `contracts/pool/src/lib.rs`, will be preserved.

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

## Token Decimals
The LP share token's `decimals()` value must be carefully considered.

**Decision:** The LP share token's `decimals()` should match the funding asset's decimals for that specific pool instance. 
Since ClearLedger now operates under a `pool_factory` + per-asset model, each pool instance works with a single funding asset (e.g., USDC, XLM). By matching the funding asset's decimals, we ensure 1:1 parity in precision when calculating deposits/withdrawals, preventing awkward scaling shifts.
