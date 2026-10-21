# Building and inspecting Soroban WASM artifacts

This guide covers the local workflow for producing ClearLedger Soroban
contract WASM, checking that a local build matches what is deployed, inspecting
a contract's interface (spec), and running the test suite. Run every command
from the repository root.

> These instructions produce artifacts for **Stellar Testnet**. Mainnet
> deployment is not supported yet; see
> [Mainnet Deployment](../DEPLOYMENT.md#mainnet-deployment).

## Toolchain

The workspace pins its build inputs in two places:

| Input | Value | Source |
| --- | --- | --- |
| Rust | `1.85.0` | `rust-toolchain.toml`, CI |
| WASM target | `wasm32v1-none` | `rust-toolchain.toml`, CI, deploy scripts |
| Soroban SDK | `21.7.6` (resolved to `21.7.7` in `Cargo.lock`) | `Cargo.toml`, `Cargo.lock` |
| Stellar CLI | latest | `DEPLOYMENT.md`; the repo does not pin a CLI version |

Install the pinned toolchain and target:

```bash
rustup toolchain install 1.85.0
rustup target add wasm32v1-none --toolchain 1.85.0
```

Confirm that the repository is using the pinned toolchain and that the target
is present:

```bash
rustup show
rustup target list --installed --toolchain 1.85.0
```

`rust-toolchain.toml` makes `rustup` select `1.85.0` inside this checkout, so
the plain `cargo` commands below use the right compiler automatically.

Install the Stellar CLI as described in
[DEPLOYMENT.md](../DEPLOYMENT.md#prerequisites), then record the version you
are using:

```bash
stellar --version
```

The CLI is not pinned by the repository. Use the same CLI version when you
compare hashes or specs, and mention it in pull requests that change an
artifact.

## Build release WASM

CI and `CONTRIBUTING.md` build directly with Cargo:

```bash
cargo build --workspace --release --target wasm32v1-none
```

For a reproducible check against the committed lockfile, add `--locked`:

```bash
cargo build --workspace --release --target wasm32v1-none --locked
```

The deployment scripts instead invoke the Stellar CLI build:

```bash
stellar contract build
```

`scripts/deploy.sh` and `scripts/deploy.ps1` run this command themselves before
deploying, so you normally only run it manually to inspect or test artifacts.
The CLI may optimize the generated WASM by default depending on its version;
check `stellar contract build --help` and do not mix artifacts built with
different options.

List the artifacts:

```bash
ls -lh target/wasm32v1-none/release/*.wasm
```

The workspace produces these contract artifacts:

| Artifact | Package | Used by deploy scripts |
| --- | --- | --- |
| `clearledger_registry.wasm` | `clearledger-registry` | yes |
| `clearledger_invoice.wasm` | `clearledger-invoice` | yes |
| `clearledger_escrow.wasm` | `clearledger-escrow` | yes |
| `clearledger_pool.wasm` | `clearledger-pool` | yes |
| `clearledger_pool_factory.wasm` | `clearledger-pool-factory` | no |

`clearledger-ttl`, `clearledger-pause`, and `clearledger-test-utils` are
`rlib` crates, so they do not produce deployable WASM.

The deployment scripts hard-code paths such as
`target/wasm32v1-none/release/clearledger_registry.wasm`. If you pass
`--out-dir` to `stellar contract build`, copy the files back to that path or
the scripts will report a missing WASM.

## Compare WASM hashes

`stellar contract info hash` prints the SHA-256 hash of a WASM file or of the
WASM installed for a deployed contract. A contract ID is **not** the same value
as the WASM hash; do not compare them.

Hash a local artifact:

```bash
stellar contract info hash \
  --wasm target/wasm32v1-none/release/clearledger_registry.wasm
```

To check a before/after change, capture the hash, make the change, rebuild with
the same command, and hash again:

```bash
stellar contract info hash \
  --wasm target/wasm32v1-none/release/clearledger_invoice.wasm

cargo build --workspace --release --target wasm32v1-none --locked

stellar contract info hash \
  --wasm target/wasm32v1-none/release/clearledger_invoice.wasm
```

An unchanged source tree, lockfile, toolchain, CLI, and build profile should
produce the same hash. A source or dependency change normally changes it.

To compare a local artifact with Testnet, read the deployed contract ID from
`.deployed-addresses` (written by `scripts/deploy.sh`) and hash the deployed
WASM:

```bash
REGISTRY_ID=$(grep '^registry=' .deployed-addresses | cut -d= -f2)

stellar contract info hash \
  --contract-id "$REGISTRY_ID" \
  --network testnet
```

You can also fetch the deployed bytes and hash them locally. This is useful
when you want to inspect the exact on-chain artifact:

```bash
stellar contract fetch \
  --id "$REGISTRY_ID" \
  --network testnet \
  --out-file /tmp/registry-onchain.wasm

stellar contract info hash --wasm /tmp/registry-onchain.wasm
```

Repeat with `invoice`, `escrow_usdc`, or `pool_usdc` in place of `registry` for
the other deployed contracts. The mapping from saved keys to artifacts is in
`scripts/deploy.sh`.

## Inspect the contract spec

`stellar contract info interface` reads the spec embedded in a WASM file or
fetched from a deployed contract. It can print generated Rust types, XDR, or
JSON:

```bash
stellar contract info interface \
  --wasm target/wasm32v1-none/release/clearledger_registry.wasm \
  --output json-formatted

stellar contract info interface \
  --wasm target/wasm32v1-none/release/clearledger_registry.wasm \
  --output rust
```

Inspect the deployed contract's interface instead of the local file:

```bash
stellar contract info interface \
  --contract-id "$REGISTRY_ID" \
  --network testnet \
  --output rust
```

Use `--output json-formatted` when reviewing or diffing the public functions,
argument and return types, and events in a pull request. Use `--output rust`
when you want the client-facing type definitions.

The related metadata commands help explain hash differences:

```bash
stellar contract info meta \
  --wasm target/wasm32v1-none/release/clearledger_registry.wasm \
  --output json-formatted

stellar contract info env-meta \
  --wasm target/wasm32v1-none/release/clearledger_registry.wasm \
  --output json-formatted
```

`meta` includes build information such as the Rust and Soroban SDK versions.
`env-meta` records the Soroban environment the contract was built for. If two
builds have different metadata, their hashes will differ too.

Older Stellar CLI releases used `stellar contract inspect --wasm <path>`,
which is now deprecated in favor of `stellar contract info`. Upgrade the CLI
rather than mixing the two commands in one comparison.

## Run local tests

Run the same host-target test command as CI:

```bash
cargo test --workspace
```

Useful narrower runs while iterating:

```bash
cargo test -p clearledger-registry
cargo test -p clearledger-invoice
cargo test -p clearledger-integration-tests -- --nocapture
```

The tests run on the host target, not on `wasm32v1-none`, but some of them need
a WASM artifact. In particular, `contracts/pool_factory/src/test.rs` loads
`clearledger_pool.wasm` to exercise the deploy-and-initialize path. It reuses
`target/wasm32v1-none/release/clearledger_pool.wasm` when present; otherwise it
builds the pool crate into `target/pool-factory-test-fixtures`. That fallback
still requires the `wasm32v1-none` target, so build the WASM first if you want
to avoid a slow nested build:

```bash
cargo build --workspace --release --target wasm32v1-none --locked
cargo test --workspace
```

CI also runs `cargo fmt --all --check`, Clippy with `-D warnings`, and
`cargo audit`. See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the full
pre-pull-request command list.

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `error[E0463]: can't find crate for core` (or a similar missing `std`/`core` error) | The WASM target is not installed. Run `rustup target add wasm32v1-none --toolchain 1.85.0`. Confirm with `rustup target list --installed --toolchain 1.85.0`. |
| `the wasm32v1-none target is required, install it with rustup target add wasm32v1-none` from a test | Same missing target. Install it for `1.85.0`, then rerun the test. |
| `ERROR: WASM file not found: target/wasm32v1-none/release/...` from `scripts/deploy.sh` | The build did not produce the expected artifact. Run `stellar contract build` from the repo root, check `target/wasm32v1-none/release/`, and do not use an alternate `--out-dir` without copying the files back. |
| Artifacts appear under `target/wasm32-unknown-unknown/` | You built a different target. This repository uses `wasm32v1-none`; CI, `rust-toolchain.toml`, and the deploy scripts all hard-code it. Rebuild with `--target wasm32v1-none`; do not copy the other directory into place. |
| Hashes differ after rebuilding unchanged source | Use the same commit, `Cargo.lock`, Rust `1.85.0`, Stellar CLI version, build command, and profile. Check `git status`, rebuild with `--locked`, and compare `stellar contract info meta` output. Different CLI optimization settings also change the bytes. |
| Local and deployed hashes differ | Confirm you are hashing the artifact the deploy script uploaded, not a stale file from another target or profile. Fetch the deployed WASM with `stellar contract fetch` and compare it. If it still differs, the deployment used different source, toolchain, or build flags. |
| `stellar: command not found` or `stellar CLI not found` | Install the Stellar CLI or point `STELLAR_BIN` at the executable. The deploy scripts prefer `stellar` on `PATH` and then `STELLAR_BIN`. |
| Spec output looks stale or empty | Rebuild the artifact, confirm the path, and run `stellar contract info interface` again. An empty result can mean the WASM has no exported spec entries; verify you are inspecting the contract artifact rather than an `rlib` helper crate. |

## CI and deployment artifacts

The `build-and-test` job in `.github/workflows/ci.yml` installs Rust `1.85.0`
and `wasm32v1-none`, builds the workspace with
`cargo build --workspace --release --target wasm32v1-none`, runs the tests, and
enforces these WASM size budgets:

| Artifact | Budget |
| --- | --- |
| `clearledger_registry.wasm` | 1 MiB (1,048,576 bytes) |
| `clearledger_invoice.wasm` | 2 MiB (2,097,152 bytes) |
| `clearledger_escrow.wasm` | 1 MiB (1,048,576 bytes) |
| `clearledger_pool.wasm` | 2 MiB (2,097,152 bytes) |

CI builds and size-checks artifacts but does not deploy them or publish hashes;
deployment remains a local, operator-run step through
`scripts/setup-testnet.sh`, `scripts/deploy.sh`, and `scripts/verify.sh`. See
[DEPLOYMENT.md](../DEPLOYMENT.md) for the full Testnet flow, wiring order, and
rollback procedure.
