# Persistent write TTL guard

Run `python3 scripts/check-persistent-ttl.py`. CI runs the same source check.
Each direct persistent `.set(&key, ...)` requires `.extend_ttl(&key, ...)`
after it in the same enclosing block. Diagnostics identify the file, line
and key. An extension of another key or in a different function cannot satisfy it.
The initial change renews supported-asset and status-count entries that lacked bumps.

This conservative source check is not a Rust control-flow proof or a ledger TTL
test. It can reject helper-based renewals and does not follow aliased storage
accessors. Keep writes direct and bind complex key expressions to local variables.
There is deliberately **no helper allowlist** in the initial implementation:
any future exception must name the exact file, function and key, document its
helper's TTL behavior and add a regression test before relaxing this rule.
Future work: AST-based alias/control-flow analysis and ledger-advance tests.
