# Changelog

All notable changes to this project will be documented in this file.


The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `registry`: `get_profile_count(role)` view, returning how many profiles are
  registered for `Role::Issuer` / `Role::Buyer` in O(1) from a single stored
  counter (#841)
- `registry`: `list_profiles(role, start, limit)` view, returning a page of
  registered addresses for a role in registration order, so indexers and admin
  dashboards no longer have to replay `issuer_registered` / `buyer_registered`
  events from genesis. `limit` is capped at 50 (mirroring the batch cap) and a
  larger page panics with the new `RegistryError::PageSizeExceeded` (#8) (#841)
- `registry`: per-role enumeration index, written by every registration path
  (`register_issuer`, `register_buyer`, `batch_register_issuers`,
  `batch_register_buyers`) as `DataKey::ProfileIndex(Role, u32)` plus
  `DataKey::ProfileCount(Role)`, TTL-extended like a profile entry. Skipped
  (already-registered) batch entries are never indexed twice (#841)
- `pool`: SEP-41 `approve`, `allowance` and `transfer_from`, so third
  parties can move a share holder's allowance under the spender's own
  auth (#756)
- `pool`: SEP-41 metadata `decimals`, `name` and `symbol`, all three supplied
  to `initialize` and stored per instance, so a pool funding an asset with
  other than 7 decimals reports the precision its shares actually have
  (#757)
- `pool`: `allowance_approved` event emitted on every successful
  `approve` (#756)
- `pool`: `get_funded_amount(invoice_id)` read-only view returning
  `Option<u128>` for the pool's active funding of an invoice: `Some(amount)`
  while funded, `None` before funding and after repayment/default (#839)
- `pool`: `protocol_fee_updated` event assertion coverage for
  `set_protocol_fee` (#767)
- Deployment scripts now pass the full `pool.initialize` argument list
  (`funding_asset`, `registry_contract`, `treasury`, `min_initial_deposit`,
  `share_name`, `share_symbol`, `share_decimals`)

### Changed

- `pool.initialize` accepts three new arguments: `share_name`, `share_symbol`
  and `share_decimals`

### Testing

- `pool.deposit` gained regression coverage pinning the behaviour the
  internal `mint` helper refactor relies on: the `lp_deposited` event
  payload is unchanged and `TotalShares` stays in step with LP balances
  (#758)

## [0.1.0] - 2025-01-01

### Added

- Registry contract for verified issuer/buyer management
- Invoice contract with full lifecycle state machine
- Escrow contract for USDC custody during funding
- Pool contract with share-based LP accounting
- Deployment scripts (`setup-testnet.sh`, `deploy.sh`, `verify.sh`)
- CI workflow with formatting, clippy, build, and test checks
- Security policy (SECURITY.md)
- Code of Conduct (CODE_OF_CONDUCT.md)
- Contributing guidelines (CONTRIBUTING.md)

### Changed

- `pool.fund_invoice` is now permissionless (previously
  required admin auth)

### Known Issues

- Issuer release not wired into `fund_invoice` (Issue #56)
- No emergency pause mechanism
- Admin key is a single signer (multi-sig planned)

## Security Researchers

We thank the following researchers for responsibly disclosing
vulnerabilities:

_(No disclosures yet)_
