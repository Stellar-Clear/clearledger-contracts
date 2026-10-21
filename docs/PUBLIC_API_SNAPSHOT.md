# Public API source snapshot

`python3 scripts/check-public-api.py` compares contract implementation public
function signatures and `contracttype` / `contracterror` declarations against
`spec/public-api.source.json`. CI fails with a reviewable diff when they change.
After reviewing client compatibility, explicitly regenerate using
`python3 scripts/check-public-api.py --update` and commit the baseline in the
same PR. Never automatically regenerate the baseline in CI.

The initial baseline comes from the repository's actual Rust source. It covers
all current contract implementations and source type declarations; documentation
and implementation bodies are excluded so routine edits do not churn signatures.

## Initial scope and remaining work

This is a **source API regression guard**, not an exported Soroban ABI guarantee.
The conservative parser is intended for the current declaration style. It does
not expand macros, follow aliases, resolve conditional compilation or decode
`contractspecv0` from WASM. It can conservatively flag formatting changes and
cannot prove serialization compatibility. Avoid unsupported declaration styles
without upgrading the guard. Public type snapshots include private storage types
as well, so reviewers should assess applicability when these change.

A follow-up should build each deployable WASM with the pinned toolchain, extract
and normalize actual `contractspecv0` entries, then commit reviewed baselines and
compare those artifacts in CI. No WASM baseline is invented here, and source
snapshots must not be described as exported spec snapshots.
