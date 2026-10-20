#!/bin/bash
# setup-testnet.sh — Idempotent testnet deployer setup
#
# Creates and funds the deployer key on the Stellar testnet.
# Safe to run multiple times: if the key already exists, it funds the
# existing account via Friendbot instead of failing.

set -euo pipefail

# Prefer a globally available `stellar` on PATH, fall back to a common WSL installation
if command -v stellar &> /dev/null; then
  STELLAR="stellar"
elif [ -n "${STELLAR_BIN:-}" ] && [ -f "$STELLAR_BIN" ]; then
  STELLAR="$STELLAR_BIN"
elif [ -f "/mnt/c/Program Files (x86)/Stellar CLI/stellar.exe" ]; then
  STELLAR="/mnt/c/Program Files (x86)/Stellar CLI/stellar.exe"
else
  echo "Error: stellar CLI not found on PATH or default Windows path."
  exit 1
fi

FRIENDBOT_URL="https://friendbot.stellar.org"
KEY_NAME="deployer"

if "$STELLAR" keys ls 2>/dev/null | grep -q "^${KEY_NAME}$"; then
  echo "Deployer key '$KEY_NAME' already exists — funding existing account via Friendbot..."
  DEPLOYER_ADDRESS=$("$STELLAR" keys address "$KEY_NAME")
  echo "Deployer address: $DEPLOYER_ADDRESS"

  HTTP_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "${FRIENDBOT_URL}?addr=${DEPLOYER_ADDRESS}")
  if [ "$HTTP_STATUS" = "200" ]; then
    echo "Friendbot funded successfully."
  elif [ "$HTTP_STATUS" = "400" ]; then
    # 400 from Friendbot typically means the account already has a balance — not an error
    echo "Friendbot returned 400 — account likely already funded (this is normal)."
  else
    echo "Warning: Friendbot returned HTTP $HTTP_STATUS. Funding may not have succeeded."
    echo "         You can retry manually: curl '${FRIENDBOT_URL}?addr=${DEPLOYER_ADDRESS}'"
  fi
else
  echo "Creating and funding testnet deployer account..."
  "$STELLAR" keys generate "$KEY_NAME" --network testnet --fund
  DEPLOYER_ADDRESS=$("$STELLAR" keys address "$KEY_NAME")
  echo "Deployer address: $DEPLOYER_ADDRESS"
  echo "Account created and funded."
fi

echo ""
echo "Done. Wait ~10 seconds for funding to confirm before running deploy.sh"
