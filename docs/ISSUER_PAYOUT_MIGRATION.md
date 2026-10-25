# Issuer payout: compatibility constraint for issue #35

## Current behavior

Pool `fund_one` sends the discounted amount through `escrow.lock`, records the
pool's funded invoice, and marks the invoice funded. The issuer is **not paid**
by that path. Adding `release_to_issuer` immediately afterward is unsafe:
it transfers the proceeds but deletes `DataKey::Locked`. Later invoice
`repay` / `repay_partial` transfers buyer money into escrow and invokes
`release_to_pool`, which requires the now-deleted record and fails `NotFound`.
The buyer transfer would roll back as part of the transaction.

The added regression test documents that merely replenishing escrow's token
balance after issuer release does not restore settlement eligibility. It pins
existing behavior to prevent treating an added release call as a complete fix.
No payout or settlement runtime behavior changes in this initial patch.

## Required atomic migration

1. Separate the loan's outstanding principal/issuer identity from token custody.
   Preserve the existing record serialization for historical invoices, or add a
   versioned settlement record with explicit legacy fallback.
2. Add an authorized, one-time issuer disbursement operation that marks custody
   as paid while retaining settlement eligibility; protect duplicate payout and
   reentry before transferring tokens.
3. Route pool single/batch funding through that operation in the same atomic
   transaction as `lock`, pool accounting and `mark_funded`. Any payout error
   must roll back every state change and token movement.
4. Update repayment and default resolution to distinguish held collateral from
   already-disbursed principal. A default must not try to transfer a paid-out
   escrow balance, and cannot consume another invoice's buyer repayments.
5. Test with the real stellar asset contract: issuer balance increase on funding,
   partial/full repayment, default after payout, duplicate funding/payout, failed
   token transfer rollback, two simultaneous loans and legacy locked records.

**Remaining:** implement the coordinated custody/settlement migration and its
integration tests. This preparatory regression is not an implemented issuer
payout feature and should not be described as resolving the complete issue.
