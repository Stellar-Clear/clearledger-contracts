# LP-Governed Invoice Funding: Design Proposal

> **Status:** Design proposal — no implementation yet (issue #718)
> **Scope:** `contracts/pool/src/lib.rs`, `docs/STORAGE.md`

This document is the concrete design the README's governance roadmap asks for
("The goal is LP-governed capital allocation…"). It covers the LP token staking
mechanism, the quorum calculation, the voting window, and how the new
vote-gated check composes with `fund_invoice`'s existing permissionless
eligibility rules.

**Nothing here is implemented.** The tests named in [Testing
plan](#testing-plan-n/a-until-implementation) are deferred to the follow-up
implementation issue that this proposal scopes.

---

## Goals

- Replace "whoever calls first with liquidity gets funded" with "invoices LP
  capital actually wants to back are funded."
- Keep the **admin** limited to the emergency pause capability, consistent with
  the README roadmap: the admin holds no funding-control lever, no veto, and no
  vote-override.
- Preserve today's guarantees: an invoice can only be funded while it is
  `Listed`, only if the pool has liquidity and utilization headroom, and only
  once.

## Non-goals

- Changing the discount/pricing math. `discount_bps` stays an issuer choice at
  `list_for_financing` time. The utilization-derived suggestion view
  (`get_suggested_discount_bps`, issue #719) is read-only guidance and is
  deliberately orthogonal to governance.
- Multi-pool governance or cross-pool vote aggregation.
- Replacing the invoice contract's own `Listed` status with a proposal state.
  Governance *gates* funding; it does not create invoices.

---

## LP token staking mechanism

LPs already hold SEP-41 pool shares (`DataKey::LPShares(Address)`), whose value
tracks the pool. Voting power should therefore be denominated in shares.

**Stake**

```
pub fn stake_for_voting(env: Env, lp: Address, shares: u128) -> u128
pub fn unstake_from_voting(env: Env, lp: Address, shares: u128) -> u128
```

- `stake_for_voting` moves `shares` from `LPShares(lp)` into
  `VotedShares(lp)`, returning the new staked balance. LP auth required.
- Staked shares stay in `total_supply` (share-price math is untouched) but are
  **frozen** for `transfer` / `transfer_from` / `withdraw` / `transfer_shares`,
  since a share pledged to an active proposal must not be moved.
- `unstake_from_voting` is only allowed when the LP has no outstanding
  proposals in their voting window (otherwise their power could be withdrawn
  after voting, invalidating the tally).
- Staked shares earn yield normally: staking is a voting lock, not a depeg.
  Yield accrues through `LPYieldEarned` as usual.

This keeps a single share supply, so no new pricing invariants are introduced.

## Quorum threshold calculation

Quorum is measured in **staked shares**, against the pool's `TotalShares` at
the moment the proposal opens:

```
required_quorum = TotalShares * QUORUM_BPS / 10_000
```

## Interaction with `fund_invoice`'s permissionless checks

`fund_invoice` is currently permissionless with a fixed eligibility sequence:
the invoice must be `Listed`, not already funded by this pool, the utilization
cap must not be exceeded, and there must be sufficient available liquidity. The
vote gate **composes** with all of it rather than replacing it:

```
fn fund_invoice(env, invoice_id) -> bool {
    require_not_paused(&env);                    // admin breaker (#714)
    Self::require_initialized(&env);

    // existing checks, unchanged
    ... AlreadyFunded (#16) / InvoiceNotListed (#8) ...
    ... utilization cap (#12), available liquidity ...

    // new governance gate, evaluated last
    Self::require_proposal_passed(&env, invoice_id)?;
    ...
}
```

Two properties matter:

- **No check is weakened.** Governance is strictly *additional*: every invoice
  that passes today still must pass the same guards, and passing a vote does
  not let the pool fund an unlisted, unfunded, or over-cap invoice.
- **The gate is a vote result, not an admin decision.** The admin has no
  `override_proposal` / `force_fund` entry point. Per the roadmap the admin's
  only lever is `pause()` / `unpause()`; per issue #714 pause itself guards
  every other mutating path via `require_not_paused`, so a paused pool has
  staking, voting, and funding disabled together.

Anyone may still *call* `fund_invoice`; the call simply reverts unless the
governed conditions hold. Keeping the entry point permissionless preserves
integration compatibility for existing callers and indexers.

## Proposed storage layout

Following `docs/STORAGE.md` conventions: keyed per-address / per-invoice state
goes in `persistent()` with per-entry TTL bumps, singleton configuration stays
in `instance()`.

| DataKey | Type | Durability | Description | Set During |
|---------|------|------------|-------------|------------|
| `VotedShares(Address)` | `u128` | persistent | Shares an LP has pledged to governance. Frozen for transfer/withdraw while proposals are live. | `stake_for_voting` / `unstake_from_voting` |
| `Proposal(BytesN<32>)` | `Proposal` | persistent | Per-invoice vote state: `opens_at_ledger`, `closes_at_ledger`, `required_quorum`, `for_votes`, `against_votes`, `passed: Option<bool>`. | On proposal open |
| `ProposalVote(BytesN<32>, Address)` | `bool` | persistent | One LP's vote for a proposal; overwritten on re-cast. | `cast_vote` |
| `QuorumBps` | `u32` | instance | Participation floor in basis points (proposed 2000). | `initialize` |
| `VotingWindowLedgers` | `u32` | instance | Proposal duration in ledgers (proposed 17,280). | `initialize` |

Per `docs/STORAGE.md`, the two new `instance()` keys are written by
`initialize` and extended by the existing `extend_instance_ttl()`. As in
`contracts/pool/src/types.rs`, new `DataKey` variants must be **appended after**
the existing ones (never inserted earlier) so deployed discriminants stay
stable.

## Proposed entry points

```
fn stake_for_voting(env: Env, lp: Address, shares: u128) -> u128
fn unstake_from_voting(env: Env, lp: Address, shares: u128) -> u128
fn cast_vote(env: Env, invoice_id: BytesN<32>, support: bool)
fn get_proposal(env: Env, invoice_id: BytesN<32>) -> Option<Proposal>
fn get_voting_power(env: Env, lp: Address) -> u128
```

The three mutating entry points are `require_not_paused`-guarded, consistent with
the pause convention in issues #713/#714/#715. Errors would extend `PoolError`
with typed variants (e.g. `NoStakedShares`, `VotingWindowClosed`,
`ProposalNotPassed`) rather than bare `unwrap()`s, per issue #441's typed-error
convention.

## Testing plan (N/A until implementation)

Deferred to the follow-up implementation issue. When scoped, cover:

- stake/unstake round-trip; unstake blocked while a proposal is live; transfers
  and withdrawals blocked for staked shares
- quorum boundary math (exactly at, just below, just above `required_quorum`)
- vote re-casting and window boundary ledgers (first/last valid ledger)
- `fund_invoice` rejects a failed proposal, accepts a passed one, and still
  enforces every pre-existing check for a passed proposal
- a paused pool rejects staking, voting, and funding together
- property test: the sum of distinct LP votes equals the recorded tally, and
  `staked + unpledged == LPShares` is conserved across stake/unstake sequences

## Open questions for review

1. Should a proposal open on the `invoice_listed` event (requires pool-side
   event subscription) or on an explicit permissionless `open_proposal(invoice_id)`
   call? The latter is simpler and avoids cross-contract event plumbing, at the
   cost of needing someone to call it.
2. Should quorum be relative to `TotalShares` (supply) or to total staked
   shares? Supply-relative resists the "everyone stakes to inflate quorum"
   failure mode; staked-relative is simpler but gameable.
3. Should a failed proposal block funding permanently, or may the issuer relist
   with different terms and get a fresh proposal?
with `QUORUM_BPS = 2000` (20% of share supply) as the proposed constant. A
proposal passes when both conditions hold at the close of its window:

1. `for_votes >= required_quorum` (participation floor), and
2. `for_votes > against_votes` (simple majority of cast staked shares).

Rationale for the two-part rule: quorum alone can be gamed by a single whale
who stakes and votes for everything, while a bare majority lets a handful of
shares outvote a broad LP base. Requiring both means a proposal needs genuine
breadth *and* a real preference. 20% is the proposed starting point; it should
be an instance-level constant, not an admin-settable knob, so governance
parameters are not admin-controlled.

## Voting window

- A proposal opens when an invoice is listed for financing and closes after
  `VOTING_WINDOW_LEDGERS` (proposed: 17,280 ledgers ≈ 24 h at 5 s/ledger).
- Window boundaries are stored per proposal (`opens_at_ledger`,
  `closes_at_ledger`) so the close is deterministic and does not depend on who
  calls first.
- `fund_invoice` becomes callable only after the window closes and the proposal
  passed. There is no "early funding" escape hatch, which is what keeps the
  vote meaningful.
- Votes can only be cast while `opens_at_ledger <= current <= closes_at_ledger`.
  Re-casting overwrites the LP's prior vote for that proposal.