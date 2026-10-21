#!/usr/bin/env python3
"""Prevent removal of existing event calls from audited admin entry points."""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def function_body(source, name):
    # Strip comments, preserving quoted strings for publisher-topic inspection.
    source = re.sub(r"/\*.*?\*/|//[^\n]*", " ", source, flags=re.S)
    match = re.search(r"pub\s+fn\s+" + re.escape(name) + r"\s*\([^{}]*\)[^{]*\{", source)
    if not match:
        raise ValueError(f"missing public function {name}")
    depth = 1
    for i in range(match.end(), len(source)):
        if source[i] == "{": depth += 1
        if source[i] == "}":
            depth -= 1
            if depth == 0: return source[match.end():i]
    raise ValueError(f"unbalanced function {name}")


def main():
    matrix = json.loads((ROOT / "spec/admin-event-matrix.json").read_text())
    failures = []
    for contract, methods in matrix.items():
        directory = ROOT / "contracts" / contract / "src"
        source = (directory / "lib.rs").read_text()
        events = (directory / "events.rs").read_text()
        for method, event in methods.items():
            try:
                body = function_body(source, method)
                if not re.search(r"\bevents\s*::\s*" + re.escape(event) + r"\s*\(", body):
                    raise ValueError(f"{method} no longer calls events::{event}")
                publisher = function_body(events, event)
                if not re.search(r"\.events\s*\(\s*\)\s*\.publish\s*\(", publisher):
                    raise ValueError(f"events::{event} no longer publishes")
            except ValueError as error:
                failures.append(f"{contract}: {error}")
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("Audited admin event calls and publisher helpers are present.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
