#!/usr/bin/env bash
set -euo pipefail

if [[ "${0##*/}" == "stellar" ]]; then
  if [[ -n "${UNRELATED_TOKEN:-}" ]]; then
    printf 'Unrelated .env value leaked into Stellar CLI environment\n' >&2
    exit 3
  fi
  printf '%s\n' "$*" >> "$STELLAR_LOG"
  if [[ "${STELLAR_FAIL:-}" == "deploy" && "$1 $2" == "contract deploy" ]]; then
    printf 'secret-signature-material\n' >&2
    exit 1
  fi
  if [[ "${STELLAR_FAIL:-}" == "invoke" && "$1 $2" == "contract invoke" ]]; then
    printf 'secret-invocation-material\n' >&2
    exit 1
  fi
  case "$1 $2" in
    "keys address")
      printf 'G%055d\n' 0 | tr '0' 'A'
      ;;
    "contract build")
      mkdir -p target/wasm32v1-none/release
      touch target/wasm32v1-none/release/clearledger_{registry,invoice,escrow,pool}.wasm
      ;;
    "contract fetch")
      exit 0
      ;;
    "contract invoke")
      printf 'true\n'
      ;;
    "contract deploy")
      wasm=""
      while (($#)); do
        if [[ "$1" == "--wasm" ]]; then
          wasm="$2"
          shift 2
        else
          shift
        fi
      done
      case "$wasm" in
        *clearledger_registry.wasm) letter=A ;;
        *clearledger_invoice.wasm) letter=B ;;
        *clearledger_escrow.wasm) letter=C ;;
        *clearledger_pool.wasm) letter=D ;;
        *) exit 2 ;;
      esac
      printf -v body '%*s' 55 ''
      printf 'C%s\n' "${body// /$letter}"
      ;;
    *)
      printf 'Unexpected Stellar CLI call: %s\n' "$*" >&2
      exit 2
      ;;
  esac
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
temp_dir="$(mktemp -d)"
trap 'rm -rf -- "$temp_dir"' EXIT
mkdir -p "$temp_dir/bin" "$temp_dir/work/scripts"
mkdir -p "$temp_dir/work/scripts/maintainer"
cp "$repo_root/scripts/deploy.sh" "$temp_dir/work/scripts/deploy.sh"
cp "$repo_root/scripts/maintainer/update-readme-addresses.sh" \
  "$temp_dir/work/scripts/maintainer/update-readme-addresses.sh"
ln -s "$(realpath "${BASH_SOURCE[0]}")" "$temp_dir/bin/stellar"
printf 'DEPLOYER_ACCOUNT=test-deployer\nUSDC_ISSUER=G%s\nUNRELATED_TOKEN=do-not-inherit\n' \
  "$(printf '%055d' 0 | tr '0' 'A')" > "$temp_dir/work/.env"
cat > "$temp_dir/work/README.md" <<'README'
before
<!-- START_DEPLOYED_ADDRESSES -->
old deployment
<!-- END_DEPLOYED_ADDRESSES -->
after
README

run_deploy() {
  (
    cd "$temp_dir/work"
    PATH="$temp_dir/bin:$PATH" STELLAR_LOG="$temp_dir/stellar.log" \
      bash scripts/deploy.sh "$@" >/dev/null
  )
}

assert_count() {
  local expected="$1" pattern="$2" actual
  actual="$(grep -c "$pattern" "$temp_dir/stellar.log" || true)"
  if [[ "$actual" != "$expected" ]]; then
    printf 'Expected %s calls matching %s, got %s\n' "$expected" "$pattern" "$actual" >&2
    exit 1
  fi
}

run_deploy --resume
assert_count 4 '^contract deploy '
assert_count 7 '^contract invoke '
for key in registry invoice escrow_usdc pool_usdc registry_initialized invoice_initialized \
  escrow_usdc_initialized pool_usdc_initialized invoice_set_pool_initialized \
  invoice_set_escrow_initialized invoice_add_asset_usdc_initialized; do
  grep -q "^${key}=" "$temp_dir/work/.deployed-addresses"
done
for address in \
  CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA \
  CBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB \
  CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC \
  CDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD; do
  grep -Fq "https://stellar.expert/explorer/testnet/contract/$address" "$temp_dir/work/README.md"
done

: > "$temp_dir/stellar.log"
run_deploy --resume
assert_count 0 '^contract deploy '
assert_count 0 '^contract invoke '

printf 'stale=old-address\n' >> "$temp_dir/work/.deployed-addresses"
: > "$temp_dir/stellar.log"
run_deploy --fresh
assert_count 4 '^contract deploy '
assert_count 7 '^contract invoke '
if grep -q '^stale=' "$temp_dir/work/.deployed-addresses"; then
  printf 'Fresh mode retained stale saved-address state\n' >&2
  exit 1
fi

: > "$temp_dir/work/.deployed-addresses"
if (
  cd "$temp_dir/work"
  PATH="$temp_dir/bin:$PATH" STELLAR_LOG="$temp_dir/stellar.log" \
    STELLAR_FAIL=deploy bash scripts/deploy.sh > "$temp_dir/failure.log" 2>&1
); then
  printf 'Expected mocked deploy failure\n' >&2
  exit 1
fi
if grep -q 'secret-signature-material' "$temp_dir/failure.log"; then
  printf 'Deployment failure leaked raw CLI output\n' >&2
  exit 1
fi
if [[ "$(stat -c '%a' "$temp_dir/work/.deployed-addresses")" != 600 ]]; then
  printf 'Deployment state file permissions are not private\n' >&2
  exit 1
fi

: > "$temp_dir/work/.deployed-addresses"
if (
  cd "$temp_dir/work"
  PATH="$temp_dir/bin:$PATH" STELLAR_LOG="$temp_dir/stellar.log" \
    STELLAR_FAIL=invoke bash scripts/deploy.sh > "$temp_dir/failure.log" 2>&1
); then
  printf 'Expected mocked invoke failure\n' >&2
  exit 1
fi
if grep -q 'secret-invocation-material' "$temp_dir/failure.log"; then
  printf 'Initialization failure leaked raw CLI output\n' >&2
  exit 1
fi

printf 'Deployment resume/fresh smoke tests passed.\n'