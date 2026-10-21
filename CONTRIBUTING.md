# Contributing to ClearLedger Contracts


Thank you for contributing to ClearLedger's Soroban contracts. Keep changes focused,
tested, and tied to an issue.


## Claim an issue


1. Choose an open issue that is not assigned.
2. Comment `.take` (or state that you are working on it).
3. Wait for the issue to show you as the assignee before starting.
4. Ask in the issue if its scope overlaps another ticket.


Issues use complexity labels:

| Label | Expected scope |
| --- | --- |
| `complexity:low` | An isolated function, test, or documentation change |
| `complexity:medium` | Contract logic or storage changes |
| `complexity:high` | Cross-contract behavior or new protocol mechanics |

Contract and type labels further identify the affected area, such as `registry`,
`invoice`, `escrow`, `pool`, `test`, or `documentation`.


## Create a branch


Start from the latest `main` and create one branch per issue:


```bash
git switch main
git pull --ff-only
git switch -c fix/123-short-description
```

Use a descriptive prefix such as `fix/`, `feat/`, `test/`, or `docs/`. Do not mix
unrelated refactors or formatting changes into the issue branch.

## Make and test changes


Prefer the smallest change that satisfies the issue acceptance criteria. Follow the
storage and error conventions documented in the
[README](./README.md#key-conventions).

Run the affected crate first for quick feedback:


```bash
cargo test -p clearledger-registry
```


Before opening a pull request, run the same checks enforced by CI:


```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --workspace
cargo build --workspace --release --target wasm32v1-none
cargo test --workspace
```

See [Building and inspecting WASM artifacts](./docs/developer-guide/wasm-artifacts.md)
for WASM artifact paths, hash comparison against a deployed contract, contract
spec inspection, and target/version troubleshooting.

Add positive and negative tests where applicable. New public functions must include
rustdoc for arguments, authorization, panics, and return values.

CI also measures line coverage with [cargo-tarpaulin](https://github.com/xd009642/tarpaulin).
It never fails the build on a coverage number, but you can reproduce the report
locally to check that new code is exercised:


```bash
cargo install cargo-tarpaulin@0.32.7 --locked
cargo tarpaulin --workspace --engine llvm --out html --output-dir coverage
```

Open `coverage/tarpaulin-report.html` to see which lines are uncovered. The same
report is published as the `coverage-report` artifact on every CI run.

## RUSTSEC advisory exceptions

CI runs `cargo audit` against `audit.toml` in the repo root. If a RUSTSEC
advisory affects a transitive dependency and has no available fix, do not
disable or skip the audit step. Instead add a time-boxed, justified entry to
`audit.toml`:

```toml
[advisories]
ignore = [
    "RUSTSEC-2024-0000", # <why no fix exists> — revisit by <YYYY-MM-DD>
]
```

Each entry must include the advisory ID, a one-line reason there is no fix
available yet, and a revisit date no more than 90 days out. Remove the entry
as soon as a fixed version of the dependency is available.

## Commit changes

Use the conventional format already shown in the README:

```text
feat(registry): add batch issuer registration
fix(pool): reject zero-share deposits
test(invoice): cover the repayment lifecycle
docs(repo): document the review process
```

Keep commits reviewable and avoid generated files unless the issue requires them.

## Open and review a pull request

1. Push the issue branch and open a pull request against `main`.
2. Complete the pull request template and include `Closes #123`.
3. Summarize the behavior change and list the commands used to validate it.
4. Mark unfinished work as a draft.
5. Address review comments with focused follow-up commits.

Maintainers review correctness, test coverage, scope, and compatibility with the
other contracts. A pull request is ready to merge after required approvals and CI
checks pass.
