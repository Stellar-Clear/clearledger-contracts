#!/bin/bash
set -euo pipefail

# This script updates the README.md with the currently deployed addresses
# by reading deployments.json or falling back to CLI arguments / environment variables.
# It relies on the injection markers:
# <!-- START_DEPLOYED_ADDRESSES -->
# <!-- END_DEPLOYED_ADDRESSES -->

show_help() {
  cat << 'HELP'
Usage:
  scripts/maintainer/update-readme-addresses.sh [REGISTRY] [INVOICE] [ESCROW_USDC] [POOL_USDC]

Description:
  Updates the README.md contract address table between the markers
  <!-- START_DEPLOYED_ADDRESSES --> and <!-- END_DEPLOYED_ADDRESSES -->.

Modes:
  1. Default (preferred): Reads addresses from deployments.json in the repository root.
  2. Fallback: If deployments.json is absent, reads addresses from:
     - Positional arguments: $1 (registry), $2 (invoice), $3 (escrow_usdc), $4 (pool_usdc)
     - OR environment variables: REGISTRY_ADDRESS, INVOICE_ADDRESS, ESCROW_USDC_ADDRESS, POOL_USDC_ADDRESS

Options:
  -h, --help    Show this help message and exit
HELP
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  show_help
  exit 0
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
README_PATH="$REPO_ROOT/README.md"
DEPLOYMENTS_FILE="$REPO_ROOT/deployments.json"

if [ ! -f "$README_PATH" ]; then
  echo "Error: $README_PATH not found."
  exit 1
fi

registry=""
invoice=""
escrow_usdc=""
pool_usdc=""

if [ -f "$DEPLOYMENTS_FILE" ]; then
  if ! command -v jq &> /dev/null; then
    echo "Error: jq is not installed. Please install jq to read deployments.json."
    exit 1
  fi
  echo "Updating README.md with latest deployed addresses from $DEPLOYMENTS_FILE..."
  registry=$(jq -r '.registry // empty' "$DEPLOYMENTS_FILE")
  invoice=$(jq -r '.invoice // empty' "$DEPLOYMENTS_FILE")
  escrow_usdc=$(jq -r '.escrow_usdc // empty' "$DEPLOYMENTS_FILE")
  pool_usdc=$(jq -r '.pool_usdc // empty' "$DEPLOYMENTS_FILE")
else
  # Fallback: check CLI arguments, then environment variables
  echo "deployments.json not found. Checking CLI arguments and environment variables fallback..."
  registry="${1:-}"
  invoice="${2:-}"
  escrow_usdc="${3:-}"
  pool_usdc="${4:-}"

  registry="${registry:-${REGISTRY_ADDRESS:-}}"
  invoice="${invoice:-${INVOICE_ADDRESS:-}}"
  escrow_usdc="${escrow_usdc:-${ESCROW_USDC_ADDRESS:-}}"
  pool_usdc="${pool_usdc:-${POOL_USDC_ADDRESS:-}}"
fi

if [[ -z "$registry" || -z "$invoice" || -z "$escrow_usdc" || -z "$pool_usdc" ]]; then
  echo "Error: Missing one or more required contract addresses."
  echo "Please provide deployments.json, pass 4 addresses as arguments, or set the environment variables:"
  echo "  REGISTRY_ADDRESS, INVOICE_ADDRESS, ESCROW_USDC_ADDRESS, POOL_USDC_ADDRESS"
  exit 1
fi

# Prepare the new table content
NEW_TABLE="| Contract | Address |\n|----------|---------|\n"
NEW_TABLE+="| registry_contract | \`$registry\` |\n"
NEW_TABLE+="| invoice_contract | \`$invoice\` |\n"
NEW_TABLE+="| escrow_contract | \`$escrow_usdc\` |\n"
NEW_TABLE+="| pool_contract | \`$pool_usdc\` |\n"

# Use awk to replace the section between markers in README.md
awk -v new_content="$NEW_TABLE" '
    /<!-- START_DEPLOYED_ADDRESSES -->/ {
        print
        printf "%s", new_content
        skip = 1
        next
    }
    /<!-- END_DEPLOYED_ADDRESSES -->/ {
        skip = 0
    }
    !skip { print }
' "$README_PATH" > "${README_PATH}.tmp" && mv "${README_PATH}.tmp" "$README_PATH"

echo "README.md successfully updated with contract addresses."
