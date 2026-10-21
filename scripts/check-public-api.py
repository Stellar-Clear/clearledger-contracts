#!/usr/bin/env python3
"""Reviewable source API snapshot; does not claim to decode exported WASM specs."""
import argparse
import difflib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = ROOT / "spec/public-api.source.json"


def clean(source):
    return re.sub(r"/\*.*?\*/|//[^\n]*", " ", source, flags=re.S)


def block_end(source, start):
    depth = 1
    for i in range(start + 1, len(source)):
        if source[i] == "{": depth += 1
        if source[i] == "}":
            depth -= 1
            if depth == 0: return i
    raise ValueError("Unbalanced source block")


def snapshot(sources):
    result = {}
    for name, raw in sorted(sources.items()):
        source = clean(raw)
        entries = []
        for match in re.finditer(r"#\[contractimpl\]\s*impl\s+[^\{]+\{", source):
            end = block_end(source, match.end() - 1)
            body = source[match.end():end]
            for function in re.finditer(r"pub\s+fn\s+\w+\s*\([^{}]*?\)\s*(?:->[^{}]+)?\{", body):
                entries.append(re.sub(r"\s+", " ", function.group()[:-1]).strip())
        for match in re.finditer(r"#\[(?:contracttype|contracterror)(?:\([^\]]*\))?\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:struct|enum)\s+\w+[^;{]*\{", source):
            end = block_end(source, match.end() - 1)
            entries.append(re.sub(r"\s+", " ", source[match.start():end + 1]).strip())
        if entries:
            result[name] = sorted(entries)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--update", action="store_true", help="explicitly accept a reviewed source API change")
    args = parser.parse_args()
    sources = {str(p.relative_to(ROOT)): p.read_text()
               for p in (ROOT / "contracts").glob("*/src/*.rs")
               if p.name not in {"test.rs", "tests.rs"} and p.parent.parent.name != "test-utils"}
    actual = json.dumps(snapshot(sources), indent=2, sort_keys=True) + "\n"
    if not snapshot(sources):
        raise SystemExit("No contract exports found; refusing an empty snapshot")
    if args.update:
        BASELINE.parent.mkdir(parents=True, exist_ok=True)
        BASELINE.write_text(actual)
        return 0
    expected = BASELINE.read_text()
    if actual != expected:
        sys.stderr.writelines(difflib.unified_diff(expected.splitlines(True), actual.splitlines(True),
                                                  fromfile=str(BASELINE), tofile="current source API"))
        print("Review compatibility, then run python3 scripts/check-public-api.py --update", file=sys.stderr)
        return 1
    print("Public source API matches the reviewed snapshot.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
