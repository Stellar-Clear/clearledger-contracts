#!/usr/bin/env python3
"""Compare public deployment output to simulated Testnet contract reads; never send."""
import argparse
import base64
import binascii
import json
from pathlib import Path
import subprocess
import sys

RPC = "https://soroban-testnet.stellar.org"
PASSPHRASE = "Test SDF Network ; September 2015"


def strkey(value, version, label):
    if not isinstance(value, str) or len(value) != 56 or value != value.upper():
        raise ValueError(f"{label} must be a canonical public StrKey")
    try:
        raw = base64.b32decode(value)
    except (binascii.Error, ValueError) as error:
        raise ValueError(f"invalid {label} encoding") from error
    if len(raw) != 35 or raw[0] != version:
        raise ValueError(f"{label} has the wrong public address type")
    checksum = binascii.crc_hqx(raw[:-2], 0).to_bytes(2, "little")
    if raw[-2:] != checksum:
        raise ValueError(f"invalid {label} checksum")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--deployments", type=Path, default=Path("deployments.json"))
    parser.add_argument("--asset", required=True, help="verified Testnet funding SAC C-address")
    parser.add_argument("--source-account", required=True, help="existing Testnet G-address for simulation; public only")
    parser.add_argument("--agent-registry", help="expected optional agent registry C-address")
    parser.add_argument("--stellar", default="stellar", help="trusted installed Stellar CLI executable")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    try:
        config = json.loads(args.deployments.read_text())
        ids = {key: strkey(config[key], 16, key) for key in ("registry", "invoice", "escrow_usdc", "pool_usdc")}
        asset = strkey(args.asset, 16, "asset contract")
        source = strkey(args.source_account, 48, "source account")
        agent = args.agent_registry or config.get("agent_registry")
        if agent:
            agent = strkey(agent, 16, "agent registry")
    except (KeyError, OSError, ValueError, TypeError) as error:
        print(f"Invalid public deployment configuration: {error}", file=sys.stderr)
        return 2

    def read(contract, method, arguments=()):
        # Explicit network and send policy override CLI defaults/environment.
        command = [args.stellar, "contract", "invoke", "--contract-id", contract,
                   "--source-account", source, "--rpc-url", RPC,
                   "--network-passphrase", PASSPHRASE, "--send=no", "--", method, *arguments]
        result = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout, check=False)
        if result.returncode:
            raise ValueError(f"CLI read failed for {method} (exit {result.returncode})")
        return json.loads(result.stdout)

    checks = [
        ("invoice registry", ids["invoice"], "get_registry_contract", (), ids["registry"]),
        ("invoice pool", ids["invoice"], "get_pool_contract", (), ids["pool_usdc"]),
        ("invoice escrow", ids["invoice"], "get_escrow_contract", (), ids["escrow_usdc"]),
        ("invoice supported asset", ids["invoice"], "is_supported_asset", ("--asset", asset), True),
        ("pool invoice", ids["pool_usdc"], "get_invoice_contract", (), ids["invoice"]),
        ("pool escrow", ids["pool_usdc"], "get_escrow_contract", (), ids["escrow_usdc"]),
        ("pool funding asset", ids["pool_usdc"], "get_funding_asset", (), asset),
        ("escrow pool", ids["escrow_usdc"], "get_pool_contract", (), ids["pool_usdc"]),
        ("escrow funding asset", ids["escrow_usdc"], "get_usdc_asset", (), asset),
    ]
    if agent:
        checks.append(("invoice agent registry", ids["invoice"], "get_agent_registry_contract", (), agent))
    failed = 0
    # Verify the registry is deployed/readable as well as referenced by invoice.
    try:
        admin = read(ids["registry"], "get_admin")
        if not isinstance(admin, str):
            raise ValueError("registry get_admin returned an unexpected value")
        strkey(admin, 48 if admin.startswith("G") else 16, "registry admin")
        print("PASS registry is readable")
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"FAIL registry: {error}", file=sys.stderr)
        failed += 1
    for label, contract, method, arguments, expected in checks:
        try:
            actual = read(contract, method, arguments)
            # bool must not compare equal to integer 1 by accident.
            if type(actual) is not type(expected) or actual != expected:
                raise ValueError("returned value differs from deployment configuration")
            print(f"PASS {label}")
        except (OSError, ValueError, subprocess.TimeoutExpired) as error:
            print(f"FAIL {label}: {error}", file=sys.stderr)
            failed += 1
    if not agent:
        print("SKIP agent registry: no expected optional address supplied")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
