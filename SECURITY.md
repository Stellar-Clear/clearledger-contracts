# Security Policy

## Supported Versions

ClearLedger is currently in active development and deployed on Stellar testnet only.
No mainnet deployment exists. Smart contracts have not undergone a formal audit.

| Version | Supported |
|-------|---------|
| 0.1.x (testnet) | ✅ |
| mainnet | ❌ Not deployed |

## Scope

The following are in scope for security reports:

- Smart contract logic errors in `contracts/` (registry, invoice, escrow, pool)
- Authentication bypass or unauthorized fund access in Soroban contracts
- Integer overflow or underflow in amount calculations
- Incorrect state transitions in the invoice lifecycle
- USDC drainage vulnerabilities in the pool or escrow contracts
- Frontend issues that could cause users to sign unintended transactions

The following are out of scope:

- Stellar protocol-level issues (report to SDF directly)
- Freighter wallet vulnerabilities (report to Freighter team)
- Testnet-only issues with no mainnet impact path
- UI cosmetic bugs

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities through the private reporting option in this repository’s Security tab.

Include in your report:
- Description of the vulnerability
- Steps to reproduce
- Affected contract or component
- Potential impact
- Suggested fix if you have one

## Response Timeline

| Action | Target Time |
|------|-----------|
| Acknowledgement | 48 hours |
| Initial assessment | 5 business days |
| Fix or mitigation | 14 business days |

## Disclosure Policy

ClearLedger follows coordinated disclosure. We ask that you give us reasonable
time to assess and address a vulnerability before public disclosure.

We will credit researchers who report valid vulnerabilities in our
[CHANGELOG.md](./CHANGELOG.md) unless they prefer to remain anonymous.

## Audit Status

ClearLedger smart contracts have not been formally audited.
Users interact with testnet contracts at their own risk.
A security audit is planned prior to any mainnet deployment.

## Further Reading

- [Threat Model](./docs/THREAT_MODEL.md) — detailed trust assumptions, auth gates, and attack vectors
- [Pre-Mainnet Audit Scope](./docs/MAINNET_AUDIT_SCOPE.md) — code map, known-risk inventory, and independent-audit questions; not an audit or readiness claim
- [Storage Schema](./docs/STORAGE.md) — on-chain data layout and TTL management
- [Registry Metadata Privacy](./docs/METADATA_PRIVACY.md) — public visibility, prohibited personal data, and deletion limits
- [Limitations](./docs/LIMITATIONS.md) — known gaps, testnet constraints, and unhandled edge cases

## Important Notice

ClearLedger is experimental software. Do not deposit real funds.
All contracts operate on Stellar testnet with test USDC only.

## Admin signer incidents

See the [admin signer response runbook](docs/admin-signer-incident-response.md)
for dependency pause sequencing, signer/ownership replacement limits, redeployment
considerations and public-only user communications.
