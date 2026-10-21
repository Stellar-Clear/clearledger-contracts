#!/usr/bin/env python3
"""Conservative source guard: each direct persistent set must renew its own key."""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SET = re.compile(r"\.persistent\s*\(\s*\)\s*\.set\s*\(\s*(&?[A-Za-z_][A-Za-z_0-9]*)\s*,")


def check(path):
    raw = path.read_text()
    # Preserve offsets and newlines so comments cannot satisfy a check.
    source = re.sub(r"/\*.*?\*/|//[^\n]*|\"(?:\\.|[^\"\\])*\"",
                    lambda m: re.sub(r"[^\n]", " ", m.group()), raw, flags=re.S)
    failures = []
    writes = list(SET.finditer(source))
    # Refuse unsupported expressions instead of silently omitting a write.
    total = len(re.findall(r"\.persistent\s*\(\s*\)\s*\.set\s*\(", source))
    if total != len(writes):
        failures.append(f"{path.relative_to(ROOT)}: unsupported persistent set key; bind it to a local variable")
    for match in writes:
        key = match.group(1)
        tail = source[match.end():]
        # No cross-branch/function matches: stop at the enclosing block end.
        depth = 0
        stop = len(tail)
        for i, char in enumerate(tail):
            if char == "{": depth += 1
            if char == "}":
                if depth == 0:
                    stop = i
                    break
                depth -= 1
        extension = re.compile(r"\.persistent\s*\(\s*\)\s*\.extend_ttl\s*\(\s*" + re.escape(key) + r"\s*,")
        if not extension.search(tail[:stop]):
            line = raw.count("\n", 0, match.start()) + 1
            failures.append(f"{path.relative_to(ROOT)}:{line}: persistent set of {key} lacks same-block TTL renewal")
    return failures


def main():
    failures = []
    for path in sorted((ROOT / "contracts").glob("*/src/*.rs")):
        if path.parent.parent.name == "test-utils" or path.name in {"test.rs", "tests.rs"}:
            continue
        failures.extend(check(path))
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("All direct persistent writes have a same-key TTL renewal.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
