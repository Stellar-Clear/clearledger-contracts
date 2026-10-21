#!/bin/bash

# Exit immediately if any command in a pipeline fails, or if a variable is unset
set -u

# CLI path configuration (matching deploy.sh with fallback)
STELLAR="/mnt/c/Program Files (x86)/Stellar CLI/stellar.exe"
if [ ! -f "$STELLAR" ]; then
  STELLAR="stellar"
fi

# Load environment configuration
if [ ! -f .env ]; then
  echo "Error: .env file not found." >&2
  echo "Run ./scripts/deploy.sh first to create the .env file with contract IDs." >&2
  exit 1
fi
source .env

# Tracking execution status
FAILED=0

# Helper function to assert that required variables are set
check_env_var() {
  local var_name="$1"
  local var_val="${!var_name-}"
  if [ -z "$var_val" ]; then
    echo "Error: $var_name is not set or is empty."
    FAILED=1
  fi
}

echo "=== Verifying Contract Deployment Configuration ==="
check_env_var "DEPLOYER_ACCOUNT"
check_env_var "REGISTRY_CONTRACT_ID"
check_env_var "INVOICE_CONTRACT_ID"
check_env_var "POOL_USDC_CONTRACT_ID"
check_env_var "ESCROW_USDC_CONTRACT_ID"
check_env_var "USDC_ISSUER"
if [ "${DEPLOY_XLM:-false}" = "true" ]; then
  check_env_var "POOL_XLM_CONTRACT_ID"
  check_env_var "ESCROW_XLM_CONTRACT_ID"
  check_env_var "XLM_ASSET"
fi

if [ $FAILED -ne 0 ]; then
  echo "Configuration check failed. Run ./scripts/deploy.sh first to populate contract IDs." >&2
  exit 1
fi

# Helper function to run a check and report status
verify_check() {
  local name="$1"
  shift
  local output

  echo "Verifying $name..."
  output=$("$@" 2>&1)
  local status=$?

  if [ $status -eq 0 ]; then
    echo "  [PASS] $name"
    echo "  Result: $output"
  else
    echo "  [FAIL] $name"
    echo "  Error: $output"
    FAILED=1
  fi
  echo ""
}

verify_value() {
  local name="$1"
  local expected="$2"
  shift 2
  local output

  echo "Verifying $name..."
  output=$("$@" 2>&1)
  local status=$?
  if [ $status -eq 0 ] && [[ "$output" == *"$expected"* ]]; then
    echo "  [PASS] $name"
  else
    echo "  [FAIL] $name"
    echo "  Expected: $expected"
    echo "  Result: $output"
    FAILED=1
  fi
  echo ""
}

verify_wasm_hash() {
  local name="$1"
  local contract_id="$2"
  local wasm_path="$3"
  local info_output actual_hash expected_hash

  echo "Verifying $name WASM hash..."
  if [ ! -f "$wasm_path" ]; then
    echo "  [FAIL] Local WASM not found: $wasm_path"
    FAILED=1
  else
    info_output=$("$STELLAR" contract info --id "$contract_id" --network testnet 2>&1)
    local status=$?
    actual_hash=$(printf '%s\n' "$info_output" | grep -Eio '[[:xdigit:]]{64}' | tail -n 1)
    expected_hash=$(sha256sum "$wasm_path" | cut -d ' ' -f 1)
    if [ $status -eq 0 ] && [ -n "$actual_hash" ] && [ "${actual_hash,,}" = "${expected_hash,,}" ]; then
      echo "  [PASS] $name WASM hash ($actual_hash)"
    else
      echo "  [FAIL] $name WASM hash"
      echo "  Expected: $expected_hash"
      echo "  Result: $info_output"
      FAILED=1
    fi
  fi
  echo ""
}

echo "=== Running Contract Verification Queries ==="

DEPLOYER_ADDRESS=$("$STELLAR" keys address "$DEPLOYER_ACCOUNT") || {
  echo "Error: could not resolve DEPLOYER_ACCOUNT '$DEPLOYER_ACCOUNT'." >&2
  exit 1
}

verify_wasm_hash "registry_contract" "$REGISTRY_CONTRACT_ID" "target/wasm32v1-none/release/clearledger_registry.wasm"
verify_wasm_hash "invoice_contract" "$INVOICE_CONTRACT_ID" "target/wasm32v1-none/release/clearledger_invoice.wasm"
verify_wasm_hash "pool_usdc_contract" "$POOL_USDC_CONTRACT_ID" "target/wasm32v1-none/release/clearledger_pool.wasm"
verify_wasm_hash "escrow_usdc_contract" "$ESCROW_USDC_CONTRACT_ID" "target/wasm32v1-none/release/clearledger_escrow.wasm"

verify_value "registry admin" "$DEPLOYER_ADDRESS" "$STELLAR" contract invoke --id "$REGISTRY_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin
verify_value "invoice admin" "$DEPLOYER_ADDRESS" "$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin
verify_value "USDC pool admin" "$DEPLOYER_ADDRESS" "$STELLAR" contract invoke --id "$POOL_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin
verify_value "USDC escrow admin" "$DEPLOYER_ADDRESS" "$STELLAR" contract invoke --id "$ESCROW_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin

verify_value "invoice registry reference" "$REGISTRY_CONTRACT_ID" "$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_registry_contract
verify_value "invoice pool reference" "$POOL_USDC_CONTRACT_ID" "$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_pool_contract
verify_value "invoice escrow reference" "$ESCROW_USDC_CONTRACT_ID" "$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_escrow_contract
verify_value "USDC pool invoice reference" "$INVOICE_CONTRACT_ID" "$STELLAR" contract invoke --id "$POOL_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_invoice_contract
verify_value "USDC pool escrow reference" "$ESCROW_USDC_CONTRACT_ID" "$STELLAR" contract invoke --id "$POOL_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_escrow_contract
verify_value "USDC pool asset" "$USDC_ISSUER" "$STELLAR" contract invoke --id "$POOL_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_usdc_asset
verify_value "USDC escrow pool reference" "$POOL_USDC_CONTRACT_ID" "$STELLAR" contract invoke --id "$ESCROW_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_pool_contract
verify_value "USDC escrow asset" "$USDC_ISSUER" "$STELLAR" contract invoke --id "$ESCROW_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_usdc_asset

# 1. Registry Contract - get_admin
CMD_REGISTRY=("$STELLAR" contract invoke --id "$REGISTRY_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin)
verify_check "registry_contract (get_admin)" "${CMD_REGISTRY[@]}"

# 2. Invoice Contract - get_counts
CMD_INVOICE=("$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_counts)
verify_check "invoice_contract (get_counts)" "${CMD_INVOICE[@]}"

# 3. USDC Pool Contract - get_stats
CMD_POOL_USDC=("$STELLAR" contract invoke --id "$POOL_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_stats)
verify_check "pool_usdc_contract (get_stats)" "${CMD_POOL_USDC[@]}"

# 4. XLM Pool Contract - get_stats
if [ "${DEPLOY_XLM:-false}" = "true" ]; then
  verify_wasm_hash "pool_xlm_contract" "$POOL_XLM_CONTRACT_ID" "target/wasm32v1-none/release/clearledger_pool.wasm"
  verify_wasm_hash "escrow_xlm_contract" "$ESCROW_XLM_CONTRACT_ID" "target/wasm32v1-none/release/clearledger_escrow.wasm"
  verify_value "XLM pool admin" "$DEPLOYER_ADDRESS" "$STELLAR" contract invoke --id "$POOL_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin
  verify_value "XLM escrow admin" "$DEPLOYER_ADDRESS" "$STELLAR" contract invoke --id "$ESCROW_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_admin
  verify_value "XLM pool asset" "$XLM_ASSET" "$STELLAR" contract invoke --id "$POOL_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_usdc_asset
  verify_value "XLM escrow asset" "$XLM_ASSET" "$STELLAR" contract invoke --id "$ESCROW_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_usdc_asset
  CMD_POOL_XLM=("$STELLAR" contract invoke --id "$POOL_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_stats)
  verify_check "pool_xlm_contract (get_stats)" "${CMD_POOL_XLM[@]}"
fi

# 5. USDC Escrow Contract - get_locked (confirm existence with dummy ID)
CMD_ESCROW_USDC=("$STELLAR" contract invoke --id "$ESCROW_USDC_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_locked --invoice_id 0000000000000000000000000000000000000000000000000000000000000000)
verify_check "escrow_usdc_contract (get_locked)" "${CMD_ESCROW_USDC[@]}"

# 6. XLM Escrow Contract - get_locked (confirm existence with dummy ID)
CMD_ESCROW_XLM=("$STELLAR" contract invoke --id "$ESCROW_XLM_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_locked --invoice_id 0000000000000000000000000000000000000000000000000000000000000000)
verify_check "escrow_xlm_contract (get_locked)" "${CMD_ESCROW_XLM[@]}"

# 7. Invoice Contract - get_escrow_contract (wired by deploy.sh; null if unset)
CMD_INVOICE_ESCROW=("$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_escrow_contract)
verify_check "invoice_contract (get_escrow_contract)" "${CMD_INVOICE_ESCROW[@]}"

# 8. Invoice Contract - get_agent_registry_contract (null until wired)
CMD_INVOICE_AGENT=("$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- get_agent_registry_contract)
verify_check "invoice_contract (get_agent_registry_contract)" "${CMD_INVOICE_AGENT[@]}"

# 9. Invoice Contract - is_supported_asset (USDC must be allow-listed by deploy.sh)
CMD_INVOICE_ASSET=("$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- is_supported_asset --asset "$USDC_ISSUER")
verify_check "invoice_contract (is_supported_asset USDC)" "${CMD_INVOICE_ASSET[@]}"
verify_value "invoice USDC allow-list" "true" "$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- is_supported_asset --asset "$USDC_ISSUER"

if [ "${DEPLOY_XLM:-false}" = "true" ]; then
  verify_value "invoice XLM allow-list" "true" "$STELLAR" contract invoke --id "$INVOICE_CONTRACT_ID" --source "$DEPLOYER_ACCOUNT" --network testnet -- is_supported_asset --asset "$XLM_ASSET"
fi

if [ $FAILED -ne 0 ]; then
  echo "Verification failed."
  exit 1
else
  echo "All contract verifications passed successfully."
  exit 0
fi
