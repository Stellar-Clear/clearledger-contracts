# Invoice due-date policy

Invoice repayment is eligible only while `now < due_date`. At the exact due
timestamp (`now == due_date`) the invoice is no longer eligible for repayment
and permissionless default handling may proceed according to its lifecycle
rules. This strict boundary avoids two callers observing different outcomes in
the same ledger timestamp.
