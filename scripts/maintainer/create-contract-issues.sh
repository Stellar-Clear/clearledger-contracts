#!/bin/bash
set -euo pipefail

# Run from inside ClearLedger-contract repo root
# Usage: bash create-contract-issues.sh

REPO="ClearLedger/ClearLedger-contract"

# ── gh wrapper with exponential-backoff rate-limit handling ─────────────────
gh_req() {
  local max_attempts=5
  local attempt=1
  local wait=10
  while true; do
    local output
    output=$(gh "$@" 2>&1) && {
      echo "$output"
      return 0
    }
    if echo "$output" | grep -qiE "rate limit|API rate limit|403|too many requests"; then
      if (( attempt >= max_attempts )); then
        echo "[FATAL] Rate limit exceeded after $max_attempts attempts." >&2
        return 1
      fi
      echo "[WARN] Rate limit hit (attempt $attempt/$max_attempts). Retrying in ${wait}s..." >&2
      sleep "$wait"
      wait=$(( wait * 2 ))
      ((attempt++))
    else
      echo "[ERROR] $output" >&2
      return 1
    fi
  done
}

echo "Creating issues for $REPO..."

# ── Re-run guard: fetch existing issue titles ────────────────────────────────
echo "Fetching existing issues to avoid duplicates..."
EXISTING=$(gh_req issue list --repo "$REPO" --limit 500 --json title --jq '.[].title' || echo "")

issue_exists() {
  local title="$1"
  while IFS= read -r line; do
    if [[ "$line" == "$title" ]]; then
      return 0
    fi
  done <<< "$EXISTING"
  return 1
}

create_issue() {
  local title="$1"
  local labels="$2"
  local body="$3"

  if issue_exists "$title"; then
    echo "⏭ SKIP: Already exists — $title"
    return 0
  fi

  gh_req issue create --repo "$REPO" --title "$title" --label "$labels" --body "$body"
  echo "✓ Created: $title"
}

# ── Ensure required labels exist ────────────────────────────────────────────
echo "Ensuring required labels exist..."
for entry in "testing:fbca04" "devops:006b75"; do
  label="${entry%%:*}"
  color="${entry##*:}"
  gh_req label create "$label" --color "$color" --repo "$REPO" 2>/dev/null || true
done

# ── REGISTRY CONTRACT ─────────────────────────────────────────────────────────

create_issue \
  "feat(registry): add batch registration support for multiple issuers" \
  "enhancement,good first issue,complexity:medium" \
  "## Summary\nThe current \`register_issuer\` function registers one address at a time. Add a \`batch_register_issuers\` function that accepts a \`Vec<(Address, Map<String, String>)>\` and registers multiple issuers in a single transaction.\n\n## Acceptance Criteria\n- [ ] \`batch_register_issuers(env, entries: Vec<(Address, Map<String, String>)>) -> u32\` returns count of registered issuers\n- [ ] Skips already-registered addresses without panicking (returns count of newly registered only)\n- [ ] Emits \`issuer_registered\` event for each newly registered address\n- [ ] Unit tests cover: empty vec, all new, all duplicate, mixed\n\n## Context\nThis is needed for onboarding flows where an admin registers multiple SME partners at once.\n\n## Tech Stack\nRust · Soroban SDK · soroban-sdk Vec and Map types"

create_issue \
  "feat(registry): add metadata update function for registered profiles" \
  "enhancement,good first issue,complexity:low" \
  "## Summary\nAfter registration, issuers and buyers cannot update their profile metadata (company name, contact info, etc). Add an \`update_metadata\` function.\n\n## Acceptance Criteria\n- [ ] \`update_metadata(env, address: Address, metadata: Map<String, String>) -> bool\`\n- [ ] address.require_auth() — only the address itself can update its own metadata\n- [ ] Panics with \`NotFound\` if address is not registered\n- [ ] Emits \`metadata_updated\` event\n- [ ] Unit tests cover: self-update succeeds, unregistered panics, wrong auth panics\n\n## Tech Stack\nRust · Soroban SDK"

create_issue \
  "test(registry): achieve 100% branch coverage on registry_contract" \
  "testing,good first issue,complexity:low" \
  "## Summary\nThe registry contract currently has unit tests for happy paths only. This issue covers writing tests for all error branches.\n\n## Acceptance Criteria\n- [ ] Test \`AlreadyRegistered\` error on duplicate issuer registration\n- [ ] Test \`AlreadyRegistered\` error on duplicate buyer registration\n- [ ] Test \`NotFound\` error on \`get_profile\` for unknown address\n- [ ] Test \`NotAuthorized\` error on \`revoke\` called by non-admin\n- [ ] Test \`is_verified\` returns false for unknown address (no panic)\n- [ ] All tests pass with \`cargo test -p clearledger-registry\`\n\n## Tech Stack\nRust · soroban-sdk testutils · Env::default() · mock_all_auths()"

# ── INVOICE CONTRACT ──────────────────────────────────────────────────────────

create_issue \
  "feat(invoice): implement invoice expiry mechanism for Listed invoices" \
  "enhancement,complexity:medium" \
  "## Summary\nInvoices in \`Listed\` status can sit unfunded indefinitely. Add an expiry mechanism: if a Listed invoice is not funded within 7 days (configurable), it auto-transitions to a new \`Expired\` status.\n\n## Acceptance Criteria\n- [ ] Add \`Expired\` variant to \`InvoiceStatus\` enum\n- [ ] Add \`expire_listing(env, invoice_id: BytesN<32>) -> bool\` function\n- [ ] Validates: status must be \`Listed\`, current timestamp > listed_at + expiry_window\n- [ ] Admin OR issuer can call this function\n- [ ] Emits \`invoice_expired\` event\n- [ ] Unit tests cover: early call panics, correct expiry succeeds\n\n## Tech Stack\nRust · Soroban SDK · env.ledger().timestamp()"

create_issue \
  "feat(invoice): add get_invoice_count_by_status read function" \
  "enhancement,good first issue,complexity:low" \
  "## Summary\nThe frontend needs to display counts per status (e.g., '12 Listed, 3 Funded, 8 Repaid') without loading all invoices. Add a read function that returns counts per status.\n\n## Acceptance Criteria\n- [ ] \`get_counts(env) -> Map<String, u32>\` returns a map of status name to count\n- [ ] Read-only — no auth required\n- [ ] Counts are maintained as storage entries updated on every status transition\n- [ ] Unit test verifies counts update correctly through full lifecycle\n\n## Tech Stack\nRust · Soroban SDK · persistent storage"

create_issue \
  "test(invoice): write full lifecycle integration test for invoice_contract" \
  "testing,complexity:high" \
  "## Summary\nWrite a single end-to-end integration test that exercises the complete invoice lifecycle in one test function using the Soroban test environment.\n\n## Test Flow\n1. Deploy registry, invoice, escrow, and pool contracts\n2. Register issuer and buyer\n3. Create invoice\n4. List for financing\n5. Fund via pool\n6. Mark as shipped\n7. Confirm delivery (both parties)\n8. Repay\n9. Assert final status == Repaid\n10. Assert pool yield increased\n\n## Acceptance Criteria\n- [ ] Test lives in \`contracts/invoice/src/test.rs\`\n- [ ] All four contracts deployed and wired in the test environment\n- [ ] Assertions at every stage verify correct status transition\n- [ ] Test passes with \`cargo test -p clearledger-invoice\`\n\n## Tech Stack\nRust · soroban-sdk testutils · env.register_contract()"

create_issue \
  "feat(invoice): add early repayment support with partial discount refund" \
  "enhancement,complexity:high" \
  "## Summary\nCurrently buyers must repay the full face value on or before the due date. Add support for early repayment where the buyer pays face value but receives a partial refund of the discount proportional to how early they paid.\n\n## Example\n- Invoice face value: 10,000 USDC\n- Discount: 200 bps (2%) = 200 USDC\n- Funded at day 0, due at day 60\n- Buyer repays at day 30\n- Discount earned by pool: 100 USDC (50%)\n- Discount refunded to buyer: 100 USDC (50%)\n\n## Acceptance Criteria\n- [ ] \`repay_early(env, invoice_id: BytesN<32>) -> bool\`\n- [ ] Calculates pro-rata refund based on days elapsed vs total term\n- [ ] Transfers full face value from buyer to pool\n- [ ] Pool refunds partial discount to buyer\n- [ ] Unit tests verify refund calculation at 25%, 50%, 75% of term\n\n## Tech Stack\nRust · Soroban SDK · u128 arithmetic"

# ── ESCROW CONTRACT ───────────────────────────────────────────────────────────

create_issue \
  "test(escrow): write unit tests for all escrow_contract functions" \
  "testing,good first issue,complexity:low" \
  "## Summary\nThe escrow contract is missing comprehensive unit tests. Write tests for all functions.\n\n## Required Tests\n- [ ] \`test_lock_stores_record_and_transfers_usdc\`\n- [ ] \`test_lock_fails_if_already_locked\`\n- [ ] \`test_lock_only_callable_by_pool\`\n- [ ] \`test_release_to_issuer_sends_correct_amount\`\n- [ ] \`test_release_to_pool_sends_correct_amount\`\n- [ ] \`test_handle_default_returns_funds_to_pool\`\n- [ ] \`test_handle_default_returns_false_if_no_record\`\n- [ ] \`test_get_locked_returns_zero_for_unknown_id\`\n\n## Tech Stack\nRust · soroban-sdk testutils · token::StellarAssetClient for mock USDC"

create_issue \
  "feat(escrow): add escrow record history log for audit trail" \
  "enhancement,complexity:medium" \
  "## Summary\nOnce an escrow record is deleted (after release or default), there is no on-chain record it existed. Add an append-only history log that records every escrow action for audit purposes.\n\n## Acceptance Criteria\n- [ ] Add \`EscrowEvent\` struct: \`{ invoice_id, action: EscrowAction, amount, timestamp }\`\n- [ ] \`EscrowAction\` enum: \`Locked | ReleasedToIssuer | ReleasedToPool | DefaultHandled\`\n- [ ] Append to \`Vec<EscrowEvent>\` in persistent storage on every action\n- [ ] Add \`get_history(env, invoice_id: BytesN<32>) -> Vec<EscrowEvent>\` read function\n- [ ] Unit tests verify history entries are created correctly\n\n## Tech Stack\nRust · Soroban SDK · contracttype · persistent storage"

# ── POOL CONTRACT ─────────────────────────────────────────────────────────────

create_issue \
  "feat(pool): add per-LP yield tracking and claim history" \
  "enhancement,complexity:high" \
  "## Summary\nLPs currently see their total yield earned but cannot see a breakdown of which invoice repayments contributed yield to their position. Add per-LP yield event history.\n\n## Acceptance Criteria\n- [ ] Add \`YieldEvent\` struct: \`{ invoice_id, yield_amount, timestamp, lp_share_bps }\`\n- [ ] On \`receive_repayment\`: calculate each LP's proportional yield share and append to their history\n- [ ] Add \`get_lp_yield_history(env, lp: Address) -> Vec<YieldEvent>\`\n- [ ] Unit tests verify yield history is accurate after multiple repayments with multiple LPs\n\n## Tech Stack\nRust · Soroban SDK · u128 proportional math"

create_issue \
  "feat(pool): add maximum utilization rate cap to protect liquidity" \
  "enhancement,complexity:medium" \
  "## Summary\nThe pool can currently fund invoices until 100% of liquidity is deployed, leaving no buffer for withdrawals. Add a configurable maximum utilization rate (default 85%) above which new invoice funding is rejected.\n\n## Acceptance Criteria\n- [ ] Add \`max_utilization_bps: u32\` to pool initialization (default 8500 = 85%)\n- [ ] \`fund_invoice\` panics with \`UtilizationCapExceeded\` if funding would push utilization above cap\n- [ ] Add \`set_max_utilization(env, admin, new_cap_bps: u32)\` admin function\n- [ ] \`get_stats\` includes \`max_utilization_bps\` in the returned struct\n- [ ] Unit tests verify cap enforcement\n\n## Tech Stack\nRust · Soroban SDK"

create_issue \
  "test(pool): write deposit, withdraw, and yield distribution unit tests" \
  "testing,good first issue,complexity:medium" \
  "## Summary\nWrite comprehensive unit tests for the pool contract covering share math and yield distribution.\n\n## Required Tests\n- [ ] \`test_first_deposit_issues_one_to_one_shares\`\n- [ ] \`test_second_deposit_issues_proportional_shares\`\n- [ ] \`test_withdraw_returns_correct_usdc\`\n- [ ] \`test_withdraw_fails_if_insufficient_liquidity\`\n- [ ] \`test_yield_increases_share_price_after_repayment\`\n- [ ] \`test_two_lps_receive_proportional_yield\`\n- [ ] \`test_utilization_rate_calculates_correctly\`\n- [ ] \`test_lp_position_reflects_current_share_price\`\n\n## Tech Stack\nRust · soroban-sdk testutils · mock USDC token"

# ── DEVOPS / CI ───────────────────────────────────────────────────────────────

create_issue \
  "chore(ci): add cargo clippy lint check to GitHub Actions workflow" \
  "devops,good first issue,complexity:low" \
  "## Summary\nThe current CI workflow runs \`cargo test\` but does not run \`cargo clippy\`. Add a clippy step that fails the build on any warnings.\n\n## Acceptance Criteria\n- [ ] Add clippy step to \`.github/workflows/ci.yml\`\n- [ ] Command: \`cargo clippy --all-targets --all-features -- -D warnings\`\n- [ ] Clippy runs after build, before tests\n- [ ] CI fails if clippy produces any warnings\n- [ ] All existing clippy warnings in the codebase are resolved\n\n## Tech Stack\nGitHub Actions · cargo clippy"

create_issue \
  "docs(contracts): write inline rustdoc comments for all public functions" \
  "documentation,good first issue,complexity:low" \
  "## Summary\nNone of the public contract functions have rustdoc comments. Add \`///\` doc comments to every public function across all four contracts.\n\n## Requirements\nEach doc comment must include:\n- One-line summary\n- \`# Arguments\` section listing each parameter\n- \`# Returns\` section\n- \`# Panics\` section listing all panic conditions with error variant names\n- \`# Example\` section with a usage snippet where applicable\n\n## Contracts to document\n- [ ] registry_contract — all 7 functions\n- [ ] invoice_contract — all 11 functions\n- [ ] escrow_contract — all 5 functions\n- [ ] pool_contract — all 9 functions\n\n## Tech Stack\nRust rustdoc syntax"

create_issue \
  "chore(scripts): add contract verification script for deployed testnet contracts" \
  "devops,good first issue,complexity:low" \
  "## Summary\nAfter deployment, there is no automated way to verify that contracts are initialized correctly. Add a \`scripts/verify.sh\` that invokes read functions on each deployed contract and prints the results.\n\n## Script Should Verify\n- [ ] \`registry_contract\`: call \`get_admin\` and print result\n- [ ] \`invoice_contract\`: call \`get_counts\` and print result\n- [ ] \`pool_contract\`: call \`get_stats\` and print result\n- [ ] \`escrow_contract\`: confirm contract exists by calling \`get_locked\` with a dummy ID\n\n## Acceptance Criteria\n- [ ] Script reads contract IDs from \`.env.example\`\n- [ ] Uses Stellar CLI \`contract invoke\` for each call\n- [ ] Prints pass/fail for each check\n- [ ] Script exits with code 1 if any check fails\n\n## Tech Stack\nBash · Stellar CLI"

echo ""
echo "==========================================="
echo "All 15 issues created/skipped for $REPO"
echo "==========================================="
