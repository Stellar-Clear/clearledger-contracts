#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
report_path="${1:-$repo_root/target/budgets/core-workflows.json}"
# Normalize a relative destination before Cargo changes the test working dir.
if [[ "$report_path" != /* ]]; then report_path="$repo_root/$report_path"; fi
export CLEARLEDGER_BUDGET_REPORT="$report_path"
cargo test --locked -p clearledger-integration-tests --test lifecycle \
  report_core_workflow_budgets -- --ignored --exact --nocapture --test-threads=1
python3 - "$report_path" <<'PY'
import json, pathlib, sys
p = pathlib.Path(sys.argv[1])
d = json.loads(p.read_text())
rows = d['results']
assert {r['workflow'] for r in rows} == {'create', 'list', 'fund', 'repay', 'default', 'deposit', 'withdraw'}
assert len(rows) == 7
assert all(isinstance(r['cpu_instructions'], int) and r['cpu_instructions'] > 0 and
           isinstance(r['memory_bytes'], int) and r['memory_bytes'] >= 0 for r in rows)
print(f'Validated report: {p}')
PY
