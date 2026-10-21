#!/bin/bash
# deploy.sh — Idempotent deployment script for ClearLedger contracts
#
# Usage:
#   ./scripts/deploy.sh                         # Normal deploy (skips already-deployed contracts)
#   ./scripts/deploy.sh --resume                # Explicit resume mode (same as default)
#   ./scripts/deploy.sh --fresh                 # Ignore saved addresses and redeploy everything
#   ./scripts/deploy.sh --dry-run               # Show what would be deployed without actually deploying
#   ./scripts/deploy.sh --only <contract>       # Redeploy a single contract (hotfix mode)
#   ./scripts/deploy.sh --help                  # Show this help
#
# <contract> for --only must be one of:
#   registry, invoice, escrow_usdc, pool_usdc, escrow_xlm, pool_xlm
#
# Deployed addresses are persisted to .deployed-addresses after each successful
# deployment step.  Re-running the script after a partial failure will skip any
# step whose address was already saved.

set -euo pipefail
umask 077

# Load configuration from .env (falling back to .env.example) using safe
# line-by-line parsing. Unlike `source`, this never executes arbitrary shell
# commands, so shell metacharacters in the file cannot be used for code
# execution (see CVE / issue #455).
load_env() {
  local file="$1"
  if [ ! -f "$file" ]; then
    echo "Warning: $file not found; skipping." >&2
    return 1
  fi
  while IFS= read -r line || [ -n "$line" ]; do
    # Trim surrounding whitespace
    line="$(printf '%s' "$line" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
    # Skip blank lines and comments
    [ -z "$line" ] && continue
    case "$line" in
      \#*) continue ;;
    esac
    # Only accept KEY=VALUE pairs
    case "$line" in
      *=*)
        key="${line%%=*}"
        value="${line#*=}"
        # Trim trailing whitespace from the key
        key="$(printf '%s' "$key" | sed 's/[[:space:]]*$//')"
        # Strip matching surrounding quotes from the value
        case "$value" in
          \"*\") value="${value#\"}"; value="${value%\"}" ;;
          \'*\') value="${value#\'}"; value="${value%\'}" ;;
        esac
        # Only export keys that are valid shell identifiers; anything else
        # (e.g. keys with spaces or special characters) is skipped.
        case "$key" in
          *[!A-Za-z0-9_]*|'') continue ;;
        esac
        # Keep unrelated .env values out of the Stellar CLI child process.
        case "$key" in
          DEPLOYER_ACCOUNT|DEPLOY_XLM|USDC_ISSUER|XLM_ASSET|AGENT_REGISTRY_CONTRACT|STELLAR_BIN) ;;
          *) continue ;;
        esac
        export "$key=$value"
        ;;
    esac
  done < "$file"
}

if [ -f .env ]; then
  load_env .env
else
  load_env .env.example
fi

# ---------------------------------------------------------------------------
# 0. CLI / env setup
# ---------------------------------------------------------------------------

FRESH=false
RESUME=false
DRY_RUN=false
ONLY_CONTRACT=""

# Parse arguments, handling --only which consumes the next argument
_args=("$@")
_i=0
while [ $_i -lt ${#_args[@]} ]; do
  arg="${_args[$_i]}"
  case "$arg" in
    --fresh)   FRESH=true ;;
    --resume)  RESUME=true ;;
    --dry-run) DRY_RUN=true ;;
    --only)
      _i=$((_i + 1))
      if [ $_i -ge ${#_args[@]} ]; then
        echo "Error: --only requires a contract key argument."
        echo "Valid keys: registry, invoice, escrow_usdc, pool_usdc, escrow_xlm, pool_xlm"
        exit 1
      fi
      ONLY_CONTRACT="${_args[$_i]}"
      case "$ONLY_CONTRACT" in
        registry|invoice|escrow_usdc|pool_usdc|escrow_xlm|pool_xlm) ;;
        *)
          echo "Error: unknown contract key '$ONLY_CONTRACT' for --only."
          echo "Valid keys: registry, invoice, escrow_usdc, pool_usdc, escrow_xlm, pool_xlm"
          exit 1
          ;;
      esac
      ;;
    --help|-h)
      sed -n '2,17p' "$0" | sed 's/^# //'
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg  (use --help for usage)"
      exit 1
      ;;
  esac
  _i=$((_i + 1))
done

# XLM contracts are experimental. Opt in only when their testnet asset is ready.
DEPLOY_XLM="${DEPLOY_XLM:-false}"
if [ "$ONLY_CONTRACT" = "escrow_xlm" ] || [ "$ONLY_CONTRACT" = "pool_xlm" ]; then
  DEPLOY_XLM=true
fi

# Prefer a globally available `stellar` on PATH, fall back to STELLAR_BIN env var,
# then fall back to a common WSL installation
if command -v stellar &> /dev/null; then
  STELLAR="stellar"
elif [ -n "${STELLAR_BIN:-}" ] && [ -f "$STELLAR_BIN" ]; then
  STELLAR="$STELLAR_BIN"
elif [ -f "/mnt/c/Program Files (x86)/Stellar CLI/stellar.exe" ]; then
  STELLAR="/mnt/c/Program Files (x86)/Stellar CLI/stellar.exe"
else
  echo "Error: stellar CLI not found."
  echo ""
  echo "Try one of:"
  echo "  1. Install stellar CLI globally (https://developers.stellar.org/docs/learn/developing-with-soroban/setup)"
  echo "  2. Set STELLAR_BIN=/path/to/stellar.exe"
  echo "  3. On WSL, ensure 'Stellar CLI' is installed in Program Files (x86)"
  echo ""
  exit 1
fi

if [ -f .env ]; then
  load_env .env
else
  load_env .env.example
fi

# ---------------------------------------------------------------------------
# 1. Address persistence helpers
# ---------------------------------------------------------------------------

ADDRESSES_FILE=".deployed-addresses"

if [ "$FRESH" = true ]; then
  echo "=== --fresh flag set: removing saved addresses and starting clean ==="
  rm -f "$ADDRESSES_FILE"
fi

touch "$ADDRESSES_FILE"
chmod 600 "$ADDRESSES_FILE"

# Write or overwrite a key in the addresses file
save_address() {
  local key="$1"
  local value="$2"
  # Remove any existing line for this key, then append
  if grep -q "^${key}=" "$ADDRESSES_FILE" 2>/dev/null; then
    # Use a temp file for portability (no in-place sed on all systems)
    grep -v "^${key}=" "$ADDRESSES_FILE" > "${ADDRESSES_FILE}.tmp" && mv "${ADDRESSES_FILE}.tmp" "$ADDRESSES_FILE"
  fi
  echo "${key}=${value}" >> "$ADDRESSES_FILE"
}

# Return the saved address for a key (empty string if not found)
load_address() {
  local key="$1"
  grep "^${key}=" "$ADDRESSES_FILE" 2>/dev/null | cut -d= -f2 || true
}

# ---------------------------------------------------------------------------
# 2. Transaction confirmation polling (replaces sleep 3)
# ---------------------------------------------------------------------------

# Wait until `stellar contract invoke` no longer returns a "transaction not found"
# style error, or until max retries are exhausted.
# For Stellar/Soroban the CLI itself handles submission retries, so what we
# actually need here is a brief back-off after a *successful* deploy call before
# the next dependent call.  We poll the contract's existence via `contract fetch`.
wait_for_contract() {
  local contract_id="$1"
  local max_attempts=15
  local attempt=0
  local delay=2

  echo "  Waiting for contract $contract_id to be confirmed on-chain..."
  while [ $attempt -lt $max_attempts ]; do
    if "$STELLAR" contract fetch \
        --id "$contract_id" \
        --network testnet > /dev/null 2>&1; then
      echo "  Confirmed."
      return 0
    fi
    attempt=$((attempt + 1))
    echo "  Attempt $attempt/$max_attempts — not yet visible, retrying in ${delay}s..."
    sleep "$delay"
  done

  echo "ERROR: Contract $contract_id was not confirmed after $((max_attempts * delay))s."
  echo "       Check your network connection or the Stellar testnet status."
  echo "       The address has NOT been saved.  Re-run the script to resume."
  return 1
}

# ---------------------------------------------------------------------------
# 3. Core deployment helper
# ---------------------------------------------------------------------------

# deploy_contract <KEY> <WASM_PATH>
# Deploys the wasm if KEY is not already in .deployed-addresses.
# In dry-run mode, prints what would be done without deploying.
# Prints the contract ID and saves it.
deploy_contract() {
  local key="$1"
  local wasm="$2"

  local existing
  existing=$(load_address "$key")

  if [ -n "$existing" ]; then
    echo "  $key already deployed at $existing — skipping." >&2
    echo "$existing"
    return 0
  fi

  if [ "$DRY_RUN" = true ]; then
    echo "  [DRY-RUN] Would deploy $key from $wasm" >&2
    echo "<dry-run>"
    return 0
  fi

  if [ ! -f "$wasm" ]; then
    echo "ERROR: WASM file not found: $wasm" >&2
    echo "       Run 'stellar contract build' first, or check the build output." >&2
    return 1
  fi

  echo "  Deploying $key from $wasm ..." >&2
  local deploy_output contract_id
  if ! deploy_output=$("$STELLAR" contract deploy \
      --wasm "$wasm" \
      --source "$DEPLOYER_ACCOUNT" \
      --network testnet 2>&1); then
    echo "ERROR: Deploy failed for $key:" >&2
    echo "  Stellar CLI output withheld to avoid logging transaction signing material." >&2
    echo "  Fix the error above and rerun — already-deployed contracts will be skipped." >&2
    return 1
  fi

  # Stellar CLI prints transaction details as well as the contract address.
  # Keep only the address so callers using command substitution receive a
  # valid contract ID rather than the progress log.
  contract_id=$(printf '%s\n' "$deploy_output" | grep -Eo 'C[A-Z2-7]{55}' | tail -n 1)
  if [ -z "$contract_id" ]; then
    echo "ERROR: Deploy output for $key did not contain a contract address:" >&2
    echo "  $deploy_output" >&2
    return 1
  fi

  # Confirm the contract is visible on-chain before saving
  if ! wait_for_contract "$contract_id" >&2; then
    return 1
  fi

  save_address "$key" "$contract_id"
  echo "  Saved: $key=$contract_id" >&2
  echo "$contract_id"
}

# ---------------------------------------------------------------------------
# 4. Initialization helper
# ---------------------------------------------------------------------------

# invoke_init <LABEL> <CONTRACT_ID> [-- args...]
# Runs `stellar contract invoke` and verifies the call succeeded.
# Skips if the KEY_initialized flag is already set in .deployed-addresses.
# In dry-run mode, prints what would be done without invoking.
invoke_init() {
  local label="$1"
  local contract_id="$2"
  shift 2   # remaining args are passed verbatim to stellar contract invoke

  local init_key="${label}_initialized"
  local existing
  existing=$(load_address "$init_key")

  if [ -n "$existing" ]; then
    echo "  $label already initialized — skipping."
    return 0
  fi

  if [ "$DRY_RUN" = true ]; then
    echo "  [DRY-RUN] Would initialize $label ($contract_id) with: $*"
    return 0
  fi

  echo "  Initializing $label ($contract_id) ..."
  local output
  if ! output=$("$STELLAR" contract invoke \
      --id "$contract_id" \
      --source "$DEPLOYER_ACCOUNT" \
      --network testnet \
      "$@" 2>&1); then
    echo "ERROR: Initialization failed for $label:"
    echo "  Stellar CLI output withheld to avoid logging transaction signing material."
    echo "  The contract is deployed but NOT initialized."
    echo "  Fix the error and rerun — this step will be retried automatically."
    return 1
  fi

  save_address "$init_key" "true"
  echo "  $label initialized successfully."
}

# ---------------------------------------------------------------------------
# 5. Pre-flight checks
# ---------------------------------------------------------------------------

echo ""
echo "=== Pre-flight checks ==="

if [ -z "${DEPLOYER_ACCOUNT:-}" ]; then
  echo "ERROR: DEPLOYER_ACCOUNT is not set.  Check your .env file."
  exit 1
fi

if [ -z "${USDC_ISSUER:-}" ]; then
  echo "ERROR: USDC_ISSUER is not set.  Check your .env file."
  exit 1
fi

if [ "$DEPLOY_XLM" = true ] && [ -z "${XLM_ASSET:-}" ]; then
  echo "ERROR: XLM_ASSET is not set.  Check your .env file."
  exit 1
fi

DEPLOYER_ADDRESS=$("$STELLAR" keys address "$DEPLOYER_ACCOUNT" 2>&1) || {
  echo "ERROR: Could not resolve address for key '$DEPLOYER_ACCOUNT'."
  echo "       Run scripts/setup-testnet.sh first."
  exit 1
}
echo "  Deployer address : $DEPLOYER_ADDRESS"
echo "  Addresses file   : $ADDRESSES_FILE"
echo "  Fresh deploy     : $FRESH"
echo "  Dry-run mode     : $DRY_RUN"
echo "  Deploy XLM       : $DEPLOY_XLM"
if [ -n "$ONLY_CONTRACT" ]; then echo "  Only deploying   : $ONLY_CONTRACT"; fi
echo ""

# ---------------------------------------------------------------------------
# 6. Build
# ---------------------------------------------------------------------------

if [ "$DRY_RUN" = true ]; then
  echo "=== [DRY-RUN] Skipping build ==="
else
  echo "=== Building all contracts ==="
  "$STELLAR" contract build
fi
echo ""

# ---------------------------------------------------------------------------
# 7. Deploy & initialize all contracts
# ---------------------------------------------------------------------------

ESCROW_XLM_ID=""
POOL_XLM_ID=""

# In --only mode, load all existing saved addresses first so cross-contract
# references remain valid even when only one contract is redeployed.
_load_or_deploy() {
  local key="$1"
  local var="$2"
  local existing
  existing=$(load_address "$key")
  if [ -n "$existing" ]; then
    eval "$var=\$existing"
  fi
}

if [ -n "$ONLY_CONTRACT" ]; then
  echo "=== --only mode: redeploying single contract: $ONLY_CONTRACT ==="
  # Clear the saved address + init flag for the targeted contract so
  # deploy_contract / invoke_init will re-run it.
  grep -v "^${ONLY_CONTRACT}=" "$ADDRESSES_FILE" > "${ADDRESSES_FILE}.tmp" 2>/dev/null || true
  grep -v "^${ONLY_CONTRACT}_initialized=" "${ADDRESSES_FILE}.tmp" > "${ADDRESSES_FILE}.tmp2" 2>/dev/null || true
  mv "${ADDRESSES_FILE}.tmp2" "$ADDRESSES_FILE"
  rm -f "${ADDRESSES_FILE}.tmp"

  # Load all surviving saved addresses into variables for cross-refs
  REGISTRY_ID=$(load_address "registry")
  INVOICE_ID=$(load_address "invoice")
  ESCROW_USDC_ID=$(load_address "escrow_usdc")
  POOL_USDC_ID=$(load_address "pool_usdc")
  ESCROW_XLM_ID=$(load_address "escrow_xlm")
  POOL_XLM_ID=$(load_address "pool_xlm")
  echo "  Existing saved addresses loaded for cross-contract references."
  echo ""
fi

# Helper: returns true if we should run this step.
# In --only mode only the targeted step runs; otherwise all steps run.
_should_run() {
  [ -z "$ONLY_CONTRACT" ] || [ "$ONLY_CONTRACT" = "$1" ]
}

if _should_run "registry"; then
  echo "=== Deploying registry_contract ==="
  REGISTRY_ID=$(deploy_contract "registry" "target/wasm32v1-none/release/clearledger_registry.wasm")
  echo "Registry: $REGISTRY_ID"

  invoke_init "registry" "$REGISTRY_ID" \
    -- initialize \
    --admin "$DEPLOYER_ADDRESS"
  echo ""
fi

if _should_run "invoice"; then
  echo "=== Deploying invoice_contract ==="
  INVOICE_ID=$(deploy_contract "invoice" "target/wasm32v1-none/release/clearledger_invoice.wasm")
  echo "Invoice: $INVOICE_ID"

  invoke_init "invoice" "$INVOICE_ID" \
    -- initialize \
    --admin "$DEPLOYER_ADDRESS" \
    --registry_contract "$REGISTRY_ID"
  echo ""
fi

if _should_run "escrow_usdc"; then
  echo "=== Deploying USDC escrow_contract ==="
  ESCROW_USDC_ID=$(deploy_contract "escrow_usdc" "target/wasm32v1-none/release/clearledger_escrow.wasm")
  echo "USDC Escrow: $ESCROW_USDC_ID"
  echo ""
fi

if _should_run "pool_usdc"; then
  echo "=== Deploying USDC pool_contract ==="
  POOL_USDC_ID=$(deploy_contract "pool_usdc" "target/wasm32v1-none/release/clearledger_pool.wasm")
  echo "USDC Pool: $POOL_USDC_ID"
  echo ""
fi

# escrow_usdc init depends on pool_usdc, so run after both are deployed
if _should_run "escrow_usdc"; then
  invoke_init "escrow_usdc" "$ESCROW_USDC_ID" \
    -- initialize \
    --admin "$DEPLOYER_ADDRESS" \
    --pool_contract "$POOL_USDC_ID" \
    --usdc_asset "$USDC_ISSUER"
fi


if _should_run "pool_usdc"; then
  invoke_init "pool_usdc" "$POOL_USDC_ID" \
    -- initialize \
    --admin "$DEPLOYER_ADDRESS" \
    --invoice_contract "$INVOICE_ID" \
    --escrow_contract "$ESCROW_USDC_ID" \
    --funding_asset "$USDC_ISSUER" \
    --registry_contract "$REGISTRY_ID" \
    --treasury "$DEPLOYER_ADDRESS" \
    --min_initial_deposit 10000000 \
    --share_name '"ClearLedger USDC Pool Shares"' \
    --share_symbol '"TT-USDC"' \
    --share_decimals 7
fi

if [ "$DEPLOY_XLM" = true ] && _should_run "escrow_xlm"; then
  echo ""
  echo "=== Deploying XLM escrow_contract (EXPERIMENTAL) ==="
  ESCROW_XLM_ID=$(deploy_contract "escrow_xlm" "target/wasm32v1-none/release/clearledger_escrow.wasm")
  echo "XLM Escrow: $ESCROW_XLM_ID"
fi

if [ "$DEPLOY_XLM" = true ] && _should_run "pool_xlm"; then
  echo ""
  echo "=== Deploying XLM pool_contract (EXPERIMENTAL) ==="
  POOL_XLM_ID=$(deploy_contract "pool_xlm" "target/wasm32v1-none/release/clearledger_pool.wasm")
  echo "XLM Pool: $POOL_XLM_ID"
fi

if [ "$DEPLOY_XLM" = true ] && _should_run "escrow_xlm"; then
  invoke_init "escrow_xlm" "$ESCROW_XLM_ID" \
    -- initialize \
    --admin "$DEPLOYER_ADDRESS" \
    --pool_contract "$POOL_XLM_ID" \
    --usdc_asset "$XLM_ASSET"
fi

if [ "$DEPLOY_XLM" = true ] && _should_run "pool_xlm"; then
  invoke_init "pool_xlm" "$POOL_XLM_ID" \
    -- initialize \
    --admin "$DEPLOYER_ADDRESS" \
    --invoice_contract "$INVOICE_ID" \
    --escrow_contract "$ESCROW_XLM_ID" \
    --funding_asset "$XLM_ASSET" \
    --registry_contract "$REGISTRY_ID" \
    --treasury "$DEPLOYER_ADDRESS" \
    --min_initial_deposit 10000000 \
    --share_name '"ClearLedger XLM Pool Shares"' \
    --share_symbol '"TT-XLM"' \
    --share_decimals 7
fi

if [ -z "$ONLY_CONTRACT" ]; then
  echo ""
  echo "=== Wiring USDC pool_contract into invoice_contract ==="
  invoke_init "invoice_set_pool" "$INVOICE_ID" \
    -- set_pool_contract \
    --pool_contract "$POOL_USDC_ID"
fi

# Without the escrow wiring below, repay / repay_partial / repay_early panic
# with InvoiceError::NotFound when reading DataKey::EscrowContract.
echo ""
echo "=== Wiring USDC escrow_contract into invoice_contract ==="
invoke_init "invoice_set_escrow" "$INVOICE_ID" \
  -- set_escrow_contract \
  --escrow_contract "$ESCROW_USDC_ID"

# Without allow-listing, create rejects every invoice with UnsupportedAsset.
# add_supported_asset is itself idempotent on-chain (a no-op for an asset
# that is already allow-listed).
echo ""
echo "=== Allow-listing USDC as a supported funding asset ==="
invoke_init "invoice_add_asset_usdc" "$INVOICE_ID" \
  -- add_supported_asset \
  --asset "$USDC_ISSUER"

if [ "$DEPLOY_XLM" = true ]; then
  echo ""
  echo "=== Allow-listing XLM as a supported funding asset (EXPERIMENTAL) ==="
  invoke_init "invoice_add_asset_xlm" "$INVOICE_ID" \
    -- add_supported_asset \
    --asset "$XLM_ASSET"
fi

# Agent-registry wiring is optional: it is only performed when
# AGENT_REGISTRY_CONTRACT is set in .env / .env.example.  Without it,
# submit_attestation panics with InvoiceError::NotFound and
# list_for_financing can never unlock.
if [ -n "${AGENT_REGISTRY_CONTRACT:-}" ]; then
  echo ""
  echo "=== Wiring agent_registry_contract into invoice_contract ==="
  invoke_init "invoice_set_agent_registry" "$INVOICE_ID" \
    -- set_agent_registry_contract \
    --agent_registry_contract "$AGENT_REGISTRY_CONTRACT"
else
  echo ""
  echo "=== AGENT_REGISTRY_CONTRACT is not set — skipping agent-registry wiring ==="
  echo "    submit_attestation will fail until it is wired (see DEPLOYMENT.md)."
fi

# ---------------------------------------------------------------------------
# 8. Persist final addresses to .deployed-addresses (already done per step)
#    and write a ready-to-use .env.deployed for the frontend
# ---------------------------------------------------------------------------

if [ "$DRY_RUN" = true ]; then
  echo "=== [DRY-RUN] Skipping .env.deployed generation ==="
else
ENV_OUT=".env.deployed"
cat > "$ENV_OUT" <<EOF
# Generated by deploy.sh on $(date -u '+%Y-%m-%dT%H:%M:%SZ')
# Copy these values into clearledger-app .env.local

NEXT_PUBLIC_REGISTRY_CONTRACT_ID=$REGISTRY_ID
NEXT_PUBLIC_INVOICE_CONTRACT_ID=$INVOICE_ID
NEXT_PUBLIC_ESCROW_USDC_CONTRACT_ID=$ESCROW_USDC_ID
NEXT_PUBLIC_ESCROW_XLM_CONTRACT_ID=$ESCROW_XLM_ID # EXPERIMENTAL
NEXT_PUBLIC_POOL_USDC_CONTRACT_ID=$POOL_USDC_ID
NEXT_PUBLIC_POOL_XLM_CONTRACT_ID=$POOL_XLM_ID # EXPERIMENTAL
EOF

JSON_OUT="deployments.json"
cat > "$JSON_OUT" <<EOF
{
  "registry": "$REGISTRY_ID",
  "invoice": "$INVOICE_ID",
  "escrow_usdc": "$ESCROW_USDC_ID",
  "escrow_xlm": "$ESCROW_XLM_ID",
  "pool_usdc": "$POOL_USDC_ID",
  "pool_xlm": "$POOL_XLM_ID",
  "updated_at": "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
}
EOF
chmod 600 "$ENV_OUT" "$JSON_OUT" "$ADDRESSES_FILE"

if [ -f "scripts/maintainer/update-readme-addresses.sh" ]; then
  bash scripts/maintainer/update-readme-addresses.sh
fi

echo ""
echo "==========================================="
echo "Deployment complete."
echo ""
echo "Addresses saved to: $ADDRESSES_FILE"
echo "JSON addresses saved to: $JSON_OUT"
echo "Frontend env saved to: $ENV_OUT"
echo ""
echo "Add to clearledger-app .env.local:"
echo ""
cat "$ENV_OUT"
echo "==========================================="
fi
