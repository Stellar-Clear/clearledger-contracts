#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_DIR="$(mktemp -d)"
trap 'rm -rf "$TEST_DIR"' EXIT

VALID_ISSUER="GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
VALID_XLM="CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHQCYSC"
MOCK_STELLAR="$TEST_DIR/stellar"

cat > "$MOCK_STELLAR" <<'MOCK'
#!/bin/sh
if [ "$1" = "keys" ] && [ "$2" = "address" ]; then
  printf '%s\n' 'GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF'
  exit 0
fi
exit 1
MOCK
chmod +x "$MOCK_STELLAR"

write_env() {
  printf 'STELLAR_NETWORK=%s\nDEPLOYER_ACCOUNT=%s\nDEPLOY_XLM=%s\nUSDC_ISSUER=%s\nXLM_ASSET=%s\n' \
    "$1" "$2" "$3" "$4" "$5" > "$TEST_DIR/.env"
}

run_validation() {
  (cd "$TEST_DIR" && STELLAR_BIN="$MOCK_STELLAR" "$REPO_ROOT/scripts/deploy.sh" --validate-env 2>&1)
}

assert_rejected() {
  local expected="$1"
  shift
  write_env "$@"
  local output
  if output="$(run_validation)"; then
    printf 'Expected rejection containing: %s\n' "$expected" >&2
    exit 1
  fi
  if [[ "$output" != *"$expected"* ]]; then
    printf 'Expected error "%s", got:\n%s\n' "$expected" "$output" >&2
    exit 1
  fi
}

write_env testnet deployer false "$VALID_ISSUER" ''
run_validation >/dev/null

write_env testnet deployer true "$VALID_ISSUER" "$VALID_XLM"
run_validation >/dev/null

assert_rejected 'only supports testnet' mainnet deployer false "$VALID_ISSUER" ''
assert_rejected 'StrKey format' testnet deployer false invalid-address ''
assert_rejected 'valid key alias' testnet 'bad alias' false "$VALID_ISSUER" ''
assert_rejected 'valid key alias' testnet '--bad' false "$VALID_ISSUER" ''
assert_rejected 'XLM_ASSET is required' testnet deployer true "$VALID_ISSUER" ''
assert_rejected 'DEPLOY_XLM must be' testnet deployer yes "$VALID_ISSUER" ''

printf 'Deployment validation checks passed.\n'