"""Check generated deployment addresses against README markers and each other."""

import json
import re
import sys
from pathlib import Path


CORE_ADDRESSES = {
    "registry": "registry_contract",
    "invoice": "invoice_contract",
    "escrow_usdc": "escrow_contract",
    "pool_usdc": "pool_contract",
}
ALL_ADDRESSES = {
    **CORE_ADDRESSES,
    "escrow_xlm": None,
    "pool_xlm": None,
}
ENV_KEYS = {
    "NEXT_PUBLIC_REGISTRY_CONTRACT_ID": "registry",
    "NEXT_PUBLIC_INVOICE_CONTRACT_ID": "invoice",
    "NEXT_PUBLIC_ESCROW_USDC_CONTRACT_ID": "escrow_usdc",
    "NEXT_PUBLIC_ESCROW_XLM_CONTRACT_ID": "escrow_xlm",
    "NEXT_PUBLIC_POOL_USDC_CONTRACT_ID": "pool_usdc",
    "NEXT_PUBLIC_POOL_XLM_CONTRACT_ID": "pool_xlm",
}
ADDRESS_PATTERN = re.compile(r"C[A-Z2-7]{55}\Z")
START_MARKER = "<!-- START_DEPLOYED_ADDRESSES -->"
END_MARKER = "<!-- END_DEPLOYED_ADDRESSES -->"


def _read_key_values(path, errors):
    values = {}
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            errors.append(f"{path.name}:{line_number}: expected KEY=VALUE")
            continue
        key, value = line.split("=", 1)
        value = value.partition(" #")[0]
        if key in values:
            errors.append(f"{path.name}:{line_number}: duplicate key {key!r}")
        values[key] = value.strip()
    return values


def _validate_source(name, source, errors):
    for key in CORE_ADDRESSES:
        if key not in source or not source[key]:
            errors.append(f"{name}: missing required {key!r} contract ID")
    for key, value in source.items():
        if key not in ALL_ADDRESSES or not value:
            continue
        if not isinstance(value, str) or not ADDRESS_PATTERN.fullmatch(value):
            errors.append(f"{name}: {key!r} is not a valid contract address")


def validate(root):
    root = Path(root)
    errors = []
    readme_path = root / "README.md"
    if not readme_path.is_file():
        return ["README.md: file is missing"]

    readme = readme_path.read_text()
    if readme.count(START_MARKER) != 1 or readme.count(END_MARKER) != 1:
        errors.append("README.md: expected exactly one start and end address marker")
        readme_addresses = {}
    else:
        start = readme.index(START_MARKER) + len(START_MARKER)
        end = readme.index(END_MARKER)
        if start > end:
            errors.append("README.md: address markers are out of order")
            readme_addresses = {}
        else:
            section = readme[start:end]
            readme_addresses = {}
            for key, row_name in CORE_ADDRESSES.items():
                pattern = re.compile(
                    rf"^\|\s*{re.escape(row_name)}\s*\|\s*`([^`]+)`\s*\|\s*$",
                    re.MULTILINE,
                )
                matches = pattern.findall(section)
                if len(matches) != 1:
                    errors.append(
                        f"README.md: expected exactly one {row_name!r} address row between markers"
                    )
                else:
                    readme_addresses[key] = matches[0]

    if readme_addresses:
        _validate_source("README.md", readme_addresses, errors)

    sources = {}
    json_path = root / "deployments.json"
    if json_path.exists():
        try:
            raw = json.loads(json_path.read_text())
            if not isinstance(raw, dict):
                raise ValueError("top-level value must be an object")
            sources[json_path.name] = {
                key: raw.get(key) for key in ALL_ADDRESSES if key in raw
            }
        except (json.JSONDecodeError, ValueError) as error:
            errors.append(f"{json_path.name}: invalid JSON ({error})")

    env_path = root / ".env.deployed"
    if env_path.exists():
        raw = _read_key_values(env_path, errors)
        sources[env_path.name] = {
            address_key: raw[env_key]
            for env_key, address_key in ENV_KEYS.items()
            if env_key in raw
        }

    addresses_path = root / ".deployed-addresses"
    if addresses_path.exists():
        raw = _read_key_values(addresses_path, errors)
        sources[addresses_path.name] = {
            key: raw[key] for key in ALL_ADDRESSES if key in raw
        }

    for name, source in sources.items():
        _validate_source(name, source, errors)
        for key in CORE_ADDRESSES:
            if key in source and key in readme_addresses:
                if source[key] != readme_addresses[key]:
                    errors.append(
                        f"{name}: {key!r} is stale; it disagrees with README.md"
                    )

    source_items = list(sources.items())
    for index, (left_name, left) in enumerate(source_items):
        for right_name, right in source_items[index + 1 :]:
            for key in ALL_ADDRESSES:
                left_value = left.get(key)
                right_value = right.get(key)
                if left_value and right_value and left_value != right_value:
                    errors.append(
                        f"{left_name} and {right_name}: {key!r} contract IDs disagree"
                    )

    return errors


def main():
    errors = validate(Path(__file__).resolve().parent.parent)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(
        "Deployment metadata is valid: README markers and address rows are present; "
        "generated address files agree when present."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())