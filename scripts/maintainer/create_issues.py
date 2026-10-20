import subprocess
import sys

REPO = "ClearLedger/ClearLedger-contract"

issues = [
    # ── REGISTRY CONTRACT ─────────────────────────────────────────────────────────
    {
        "title": "feat(registry): add batch registration support for multiple issuers",
        "labels": "enhancement,good first issue,complexity:medium",
        "body": """## Summary
The current `register_issuer` function registers one address at a time. Add a `batch_register_issuers` function that accepts a `Vec<(Address, Map<String, String>)>` and registers multiple issuers in a single transaction.

## Acceptance Criteria
- [ ] `batch_register_issuers(env, entries: Vec<(Address, Map<String, String>)>) -> u32` returns count of registered issuers
- [ ] Skips already-registered addresses without panicking (returns count of newly registered only)
- [ ] Emits `issuer_registered` event for each newly registered address
- [ ] Unit tests cover: empty vec, all new, all duplicate, mixed

## Context
This is needed for onboarding flows where an admin registers multiple SME partners at once.

## Tech Stack
Rust · Soroban SDK · soroban-sdk Vec and Map types"""
    },
    {
        "title": "feat(registry): add metadata update function for registered profiles",
        "labels": "enhancement,good first issue,complexity:low",
        "body": """## Summary
After registration, issuers and buyers cannot update their profile metadata (company name, contact info, etc). Add an `update_metadata` function.

## Acceptance Criteria
- [ ] `update_metadata(env, address: Address, metadata: Map<String, String>) -> bool`
- [ ] address.require_auth() — only the address itself can update its own metadata
- [ ] Panics with `NotFound` if address is not registered
- [ ] Emits `metadata_updated` event
- [ ] Unit tests cover: self-update succeeds, unregistered panics, wrong auth panics

## Tech Stack
Rust · Soroban SDK"""
    },
    {
        "title": "test(registry): achieve 100% branch coverage on registry_contract",
        "labels": "testing,good first issue,complexity:low",
        "body": """## Summary
The registry contract currently has unit tests for happy paths only. This issue covers writing tests for all error branches.

## Acceptance Criteria
- [ ] Test `AlreadyRegistered` error on duplicate issuer registration
- [ ] Test `AlreadyRegistered` error on duplicate buyer registration
- [ ] Test `NotFound` error on `get_profile` for unknown address
- [ ] Test `NotAuthorized` error on `revoke` called by non-admin
- [ ] Test `is_verified` returns false for unknown address (no panic)
- [ ] All tests pass with `cargo test -p clearledger-registry`

## Tech Stack
Rust · soroban-sdk testutils · Env::default() · mock_all_auths()"""
    },
    # ── INVOICE CONTRACT ──────────────────────────────────────────────────────────
    {
        "title": "feat(invoice): implement invoice expiry mechanism for Listed invoices",
        "labels": "enhancement,complexity:medium",
        "body": """## Summary
Invoices in `Listed` status can sit unfunded indefinitely. Add an expiry mechanism: if a Listed invoice is not funded within 7 days (configurable), it auto-transitions to a new `Expired` status.

## Acceptance Criteria
- [ ] Add `Expired` variant to `InvoiceStatus` enum
- [ ] Add `expire_listing(env, invoice_id: BytesN<32>) -> bool` function
- [ ] Validates: status must be `Listed`, current timestamp > listed_at + expiry_window
- [ ] Admin OR issuer can call this function
- [ ] Emits `invoice_expired` event
- [ ] Unit tests cover: early call panics, correct expiry succeeds

## Tech Stack
Rust · Soroban SDK · env.ledger().timestamp()"""
    },
    {
        "title": "feat(invoice): add get_invoice_count_by_status read function",
        "labels": "enhancement,good first issue,complexity:low",
        "body": """## Summary
The frontend needs to display counts per status (e.g., '12 Listed, 3 Funded, 8 Repaid') without loading all invoices. Add a read function that returns counts per status.

## Acceptance Criteria
- [ ] `get_counts(env) -> Map<String, u32>` returns a map of status name to count
- [ ] Read-only — no auth required
- [ ] Counts are maintained as storage entries updated on every status transition
- [ ] Unit test verifies counts update correctly through full lifecycle

## Tech Stack
Rust · Soroban SDK · persistent storage"""
    },
    {
        "title": "test(invoice): write full lifecycle integration test for invoice_contract",
        "labels": "testing,complexity:high",
        "body": """## Summary
Write a single end-to-end integration test that exercises the complete invoice lifecycle in one test function using the Soroban test environment.

## Test Flow
1. Deploy registry, invoice, escrow, and pool contracts
2. Register issuer and buyer
3. Create invoice
4. List for financing
5. Fund via pool
6. Mark as shipped
7. Confirm delivery (both parties)
8. Repay
9. Assert final status == Repaid
10. Assert pool yield increased

## Acceptance Criteria
- [ ] Test lives in `contracts/invoice/src/test.rs`
- [ ] All four contracts deployed and wired in the test environment
- [ ] Assertions at every stage verify correct status transition
- [ ] Test passes with `cargo test -p clearledger-invoice`

## Tech Stack
Rust · soroban-sdk testutils · env.register_contract()"""
    },
    {
        "title": "feat(invoice): add early repayment support with partial discount refund",
        "labels": "enhancement,complexity:high",
        "body": """## Summary
Currently buyers must repay the full face value on or before the due date. Add support for early repayment where the buyer pays face value but receives a partial refund of the discount proportional to how early they paid.

## Example
- Invoice face value: 10,000 USDC
- Discount: 200 bps (2%) = 200 USDC
- Funded at day 0, due at day 60
- Buyer repays at day 30
- Discount earned by pool: 100 USDC (50%)
- Discount refunded to buyer: 100 USDC (50%)

## Acceptance Criteria
- [ ] `repay_early(env, invoice_id: BytesN<32>) -> bool`
- [ ] Calculates pro-rata refund based on days elapsed vs total term
- [ ] Transfers full face value from buyer to pool
- [ ] Pool refunds partial discount to buyer
- [ ] Unit tests verify refund calculation at 25%, 50%, 75% of term

## Tech Stack
Rust · Soroban SDK · u128 arithmetic"""
    },
    # ── ESCROW CONTRACT ───────────────────────────────────────────────────────────
    {
        "title": "test(escrow): write unit tests for all escrow_contract functions",
        "labels": "testing,good first issue,complexity:low",
        "body": """## Summary
The escrow contract is missing comprehensive unit tests. Write tests for all functions.

## Required Tests
- [ ] `test_lock_stores_record_and_transfers_usdc`
- [ ] `test_lock_fails_if_already_locked`
- [ ] `test_lock_only_callable_by_pool`
- [ ] `test_release_to_issuer_sends_correct_amount`
- [ ] `test_release_to_pool_sends_correct_amount`
- [ ] `test_handle_default_returns_funds_to_pool`
- [ ] `test_handle_default_returns_false_if_no_record`
- [ ] `test_get_locked_returns_zero_for_unknown_id`

## Tech Stack
Rust · soroban-sdk testutils · token::StellarAssetClient for mock USDC"""
    },
    {
        "title": "feat(escrow): add escrow record history log for audit trail",
        "labels": "enhancement,complexity:medium",
        "body": """## Summary
Once an escrow record is deleted (after release or default), there is no on-chain record it existed. Add an append-only history log that records every escrow action for audit purposes.

## Acceptance Criteria
- [ ] Add `EscrowEvent` struct: `{ invoice_id, action: EscrowAction, amount, timestamp }`
- [ ] `EscrowAction` enum: `Locked | ReleasedToIssuer | ReleasedToPool | DefaultHandled`
- [ ] Append to `Vec<EscrowEvent>` in persistent storage on every action
- [ ] Add `get_history(env, invoice_id: BytesN<32>) -> Vec<EscrowEvent>` read function
- [ ] Unit tests verify history entries are created correctly

## Tech Stack
Rust · Soroban SDK · contracttype · persistent storage"""
    },
    # ── POOL CONTRACT ─────────────────────────────────────────────────────────────
    {
        "title": "feat(pool): add per-LP yield tracking and claim history",
        "labels": "enhancement,complexity:high",
        "body": """## Summary
LPs currently see their total yield earned but cannot see a breakdown of which invoice repayments contributed yield to their position. Add per-LP yield event history.

## Acceptance Criteria
- [ ] Add `YieldEvent` struct: `{ invoice_id, yield_amount, timestamp, lp_share_bps }`
- [ ] On `receive_repayment`: calculate each LP's proportional yield share and append to their history
- [ ] Add `get_lp_yield_history(env, lp: Address) -> Vec<YieldEvent>`
- [ ] Unit tests verify yield history is accurate after multiple repayments with multiple LPs

## Tech Stack
Rust · Soroban SDK · u128 proportional math"""
    },
    {
        "title": "feat(pool): add maximum utilization rate cap to protect liquidity",
        "labels": "enhancement,complexity:medium",
        "body": """## Summary
The pool can currently fund invoices until 100% of liquidity is deployed, leaving no buffer for withdrawals. Add a configurable maximum utilization rate (default 85%) above which new invoice funding is rejected.

## Acceptance Criteria
- [ ] Add `max_utilization_bps: u32` to pool initialization (default 8500 = 85%)
- [ ] `fund_invoice` panics with `UtilizationCapExceeded` if funding would push utilization above cap
- [ ] Add `set_max_utilization(env, admin, new_cap_bps: u32)` admin function
- [ ] `get_stats` includes `max_utilization_bps` in the returned struct
- [ ] Unit tests verify cap enforcement

## Tech Stack
Rust · Soroban SDK"""
    },
    {
        "title": "test(pool): write deposit, withdraw, and yield distribution unit tests",
        "labels": "testing,good first issue,complexity:medium",
        "body": """## Summary
Write comprehensive unit tests for the pool contract covering share math and yield distribution.

## Required Tests
- [ ] `test_first_deposit_issues_one_to_one_shares`
- [ ] `test_second_deposit_issues_proportional_shares`
- [ ] `test_withdraw_returns_correct_usdc`
- [ ] `test_withdraw_fails_if_insufficient_liquidity`
- [ ] `test_yield_increases_share_price_after_repayment`
- [ ] `test_two_lps_receive_proportional_yield`
- [ ] `test_utilization_rate_calculates_correctly`
- [ ] `test_lp_position_reflects_current_share_price`

## Tech Stack
Rust · soroban-sdk testutils · mock USDC token"""
    },
    # ── DEVOPS / CI ───────────────────────────────────────────────────────────────
    {
        "title": "chore(ci): add cargo clippy lint check to GitHub Actions workflow",
        "labels": "devops,good first issue,complexity:low",
        "body": """## Summary
The current CI workflow runs `cargo test` but does not run `cargo clippy`. Add a clippy step that fails the build on any warnings.

## Acceptance Criteria
- [ ] Add clippy step to `.github/workflows/ci.yml`
- [ ] Command: `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] Clippy runs after build, before tests
- [ ] CI fails if clippy produces any warnings
- [ ] All existing clippy warnings in the codebase are resolved

## Tech Stack
GitHub Actions · cargo clippy"""
    },
    {
        "title": "docs(contracts): write inline rustdoc comments for all public functions",
        "labels": "documentation,good first issue,complexity:low",
        "body": """## Summary
None of the public contract functions have rustdoc comments. Add `///` doc comments to every public function across all four contracts.

## Requirements
Each doc comment must include:
- One-line summary
- `# Arguments` section listing each parameter
- `# Returns` section
- `# Panics` section listing all panic conditions with error variant names
- `# Example` section with a usage snippet where applicable

## Contracts to document
- [ ] registry_contract — all 7 functions
- [ ] invoice_contract — all 11 functions
- [ ] escrow_contract — all 5 functions
- [ ] pool_contract — all 9 functions

## Tech Stack
Rust rustdoc syntax"""
    },
    {
        "title": "chore(scripts): add contract verification script for deployed testnet contracts",
        "labels": "devops,good first issue,complexity:low",
        "body": """## Summary
After deployment, there is no automated way to verify that contracts are initialized correctly. Add a `scripts/verify.sh` that invokes read functions on each deployed contract and prints the results.

## Script Should Verify
- [ ] `registry_contract`: call `get_admin` and print result
- [ ] `invoice_contract`: call `get_counts` and print result
- [ ] `pool_contract`: call `get_stats` and print result
- [ ] `escrow_contract`: confirm contract exists by calling `get_locked` with a dummy ID

## Acceptance Criteria
- [ ] Script reads contract IDs from `.env.example`
- [ ] Uses Stellar CLI `contract invoke` for each call
- [ ] Prints pass/fail for each check
- [ ] Script exits with code 1 if any check fails

## Tech Stack
Bash · Stellar CLI"""
    }
]

print(f"Ensuring required labels exist for {REPO}...")
for label, color in [("testing", "fbca04"), ("devops", "006b75")]:
    cmd = ["gh", "label", "create", label, "--color", color, "--repo", REPO]
    subprocess.run(cmd, capture_output=True)

print(f"Fetching existing issues from {REPO}...")
existing_titles = set()
res = subprocess.run(["gh", "issue", "list", "--repo", REPO, "--limit", "100", "--json", "title"], capture_output=True, text=True)
if res.returncode == 0:
    import json
    try:
        issues_json = json.loads(res.stdout)
        existing_titles = {iss["title"] for iss in issues_json}
    except Exception as e:
        print(f"Warning: Failed to parse existing issues: {e}")

print(f"Creating 15 issues for {REPO}...")

for idx, issue in enumerate(issues, start=1):
    if issue["title"] in existing_titles:
        print(f"SKIP: Issue {idx} already exists: {issue['title']}")
        continue

    cmd = [
        "gh", "issue", "create",
        "--repo", REPO,
        "--title", issue["title"],
        "--label", issue["labels"],
        "--body", issue["body"]
    ]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, check=True)
        print(f"OK: Issue {idx} created: {issue['title']}")
    except subprocess.CalledProcessError as e:
        print(f"ERROR creating Issue {idx}: {e.stderr.strip()}", file=sys.stderr)
        sys.exit(1)

print("\n===========================================")
print(f"All 15 issues created/verified for {REPO}")
print("===========================================")
