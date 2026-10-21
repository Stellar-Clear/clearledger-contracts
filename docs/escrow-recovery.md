# Escrow admin recovery policy

Admin-triggered escrow recovery is authorized by the escrow admin and is only valid for an existing, unresolved lock. A resolved lock cannot be recovered a second time, and a caller without admin authorization is rejected. Recovery must preserve the configured pool and asset relationship; clients should inspect the resulting event before updating an off-chain view.

Use the Testnet contract and fixture tests when rehearsing recovery. This procedure does not authorize arbitrary invoice or pool state changes.
