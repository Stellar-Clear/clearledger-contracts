# Admin event audit

The reviewed admin operations already publish events. This change adds a
regression check instead of introducing duplicate runtime emissions.
Run `python3 scripts/check-admin-events.py`; CI runs the same command.
The checked-in matrix pins each public method to its event helper and verifies
that helper actually calls the Soroban event publisher.

| Contract | Entry point | Event helper |
| --- | --- | --- |
| registry | `verify_profile` | `profile_verified` |
| registry | `revoke` | `address_revoked` |
| registry | `reinstate` | `address_reinstated` |
| registry | `transfer_ownership` | `ownership_transferred` |
| registry | `transfer_admin` | `admin_transferred` |
| registry | `pause` | `paused` |
| registry | `unpause` | `unpaused` |
| invoice | `set_pool_contract` | `pool_contract_updated` |
| invoice | `set_agent_registry_contract` | `agent_registry_contract_updated` |
| invoice | `set_escrow_contract` | `escrow_contract_updated` |
| invoice | `set_expiry_window` | `expiry_window_set` |
| invoice | `add_supported_asset` | `supported_asset_added` |
| invoice | `remove_supported_asset` | `supported_asset_removed` |
| invoice | `transfer_ownership` | `ownership_transferred` |
| invoice | `pause` | `paused` |
| invoice | `unpause` | `unpaused` |
| pool | `set_max_utilization` | `max_utilization_updated` |
| pool | `set_protocol_fee` | `protocol_fee_updated` |
| pool | `transfer_ownership` | `ownership_transferred` |
| pool | `pause` | `paused` |
| pool | `unpause` | `unpaused` |
| escrow | `pause` | `paused` |
| escrow | `unpause` | `unpaused` |

## Event data notes

- Registry verification includes the subject address in topics and the new
  verification status in data. Revoke/reinstate identify the subject address.
- Pool utilization includes old/new basis points; protocol fee includes old/new
  basis points and the treasury address.
- Invoice contract wiring includes old/new addresses in topics; its initial
  assignment uses the new address for both values. Expiry records the window.
- Pause/unpause identify the authorizing admin. Ownership topic/data shapes
  differ by contract; consumers must use the corresponding `events.rs` schema.
- Existing no-op branches may return before emitting (for example already-revoked
  profiles and already-supported assets). The matrix does not promise one event
  for every invocation or add events for operations that do not change state.

## Scope

This is a source regression guard for the listed methods, not a functional
payload/authentication assertion or proof that every control-flow path emits.
It detects removal of named calls and publisher bodies without treating an
unrelated event elsewhere as coverage. Reviewers must update the matrix for new
admin setters and review authorization, event fields, failed-transaction rollback
and successful execution in contract tests. Runtime event schemas remain unchanged.
