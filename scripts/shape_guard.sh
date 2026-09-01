#!/usr/bin/env bash
# Shape guard: file size, comment blocks, comment ratio, and narrative prose.
#
# Four checks, all of them things a reviewer will not catch reliably:
#
#   size       code lines before `mod tests` in a .rs file, ceiling 800
#   block      one run of consecutive comment lines, ceiling 12
#   ratio      comment lines over code lines in a .rs file, ceiling 0.5
#   narrative  branch names, ISO dates and status phrases in prose
#
# Existing violations are counted per file in `scripts/shape_baseline.txt`.
# The guard fails when a file's count goes above its baseline, or when a file
# with no baseline entry violates at all. A count below the baseline prints a
# note asking you to shrink the baseline; shrinking it is the only edit this
# file is supposed to receive.
#
# Run it with no arguments to check the whole tree. `--print-baseline` writes
# a fresh baseline to stdout, which is how you shrink it after a cleanup.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
exec python3 - "$@" <<'PYTHON'
import os
import re
import subprocess
import sys

BASELINE = "scripts/shape_baseline.txt"

SIZE_CEILING = 800
BLOCK_CEILING = 12
RATIO_CEILING = 0.5

# Prose that describes the moment a line was written rather than the system it
# documents. Each one goes stale and none of them can be checked by a reader.
NARRATIVE = [
    ("branch name", re.compile(r"\b(?:fix|feat)/[a-z0-9][a-z0-9._-]*")),
    ("ISO date", re.compile(r"\b\d{4}-\d{2}-\d{2}\b")),
    ("status phrase", re.compile(r"measured on|this week|\bbatch (?:\d+|N)\b", re.I)),
]

COMMENT = re.compile(r"^\s*(//|/\*|\*(?!/))")
TESTS_MOD = re.compile(r"^\s*mod tests\b|^\s*#\[cfg\(test\)\]")


def tracked_rust_files():
    out = subprocess.run(
        ["git", "ls-files", "*.rs"], capture_output=True, text=True, check=True
    ).stdout
    return [p for p in out.splitlines() if p]


def read(path):
    with open(path, encoding="utf-8", errors="replace") as handle:
        return handle.read().splitlines()


def rust_metrics(lines):
    """Code lines before the test module, comment lines, longest comment run."""
    cut = len(lines)
    for index, line in enumerate(lines):
        if TESTS_MOD.match(line):
            cut = index
            break
    code = comment = 0
    for line in lines[:cut]:
        if not line.strip():
            continue
        if COMMENT.match(line):
            comment += 1
        else:
            code += 1
    return code, comment


def comment_blocks(lines):
    """Every run of consecutive comment lines, as (start line, length)."""
    blocks = []
    run_start = None
    for index, line in enumerate(lines, start=1):
        if COMMENT.match(line):
            if run_start is None:
                run_start = index
        elif run_start is not None:
            blocks.append((run_start, index - run_start))
            run_start = None
    if run_start is not None:
        blocks.append((run_start, len(lines) + 1 - run_start))
    return blocks


def spec_exempt_ranges(lines):
    """Line numbers inside spec.md section 16 and Appendix A, which may narrate.

    Section 16 is the decisions log and Appendix A is the measured baseline.
    Both exist to record when something was decided or measured, so a date is
    the content there rather than residue.
    """
    exempt = set()
    inside = False
    for index, line in enumerate(lines, start=1):
        if line.startswith("## "):
            inside = line.startswith("## 16.") or line.startswith("## Appendix A")
        if inside:
            exempt.add(index)
    return exempt


def narrative_hits(path, lines):
    """Narrative violations as (line number, what matched)."""
    hits = []
    if path.endswith(".rs"):
        candidates = [(n, l) for n, l in enumerate(lines, 1) if COMMENT.match(l)]
    elif path == "docs/shapes.md":
        # The `fleet` field is defined as a count and the date it was taken, so
        # the date is data there. Every other line in the atlas is prose.
        candidates = [
            (n, l)
            for n, l in enumerate(lines, 1)
            if not re.match(r"^\s*-?\s*fleet:", l)
        ]
    elif path == "spec.md":
        exempt = spec_exempt_ranges(lines)
        candidates = [(n, l) for n, l in enumerate(lines, 1) if n not in exempt]
    else:
        candidates = list(enumerate(lines, 1))
    for number, line in candidates:
        for name, pattern in NARRATIVE:
            if pattern.search(line):
                hits.append((number, name))
    return hits


def collect():
    """Every violation in the tree, as {(check, path): [detail, ...]}."""
    found = {}

    def add(check, path, detail):
        found.setdefault((check, path), []).append(detail)

    for path in tracked_rust_files():
        lines = read(path)
        code, comment = rust_metrics(lines)
        if code > SIZE_CEILING:
            add("size", path, f"{code} code lines before the test module")
        if comment > RATIO_CEILING * max(code, 1):
            add("ratio", path, f"{comment} comment lines over {code} code lines")
        for start, length in comment_blocks(lines):
            if length > BLOCK_CEILING:
                add("block", path, f"line {start}: {length} comment lines")
        for number, name in narrative_hits(path, lines):
            add("narrative", path, f"line {number}: {name}")

    prose = ["spec.md", "AGENTS.md", "CONTRIBUTING.md"]
    prose += [
        p
        for p in subprocess.run(
            ["git", "ls-files", "docs/*.md"], capture_output=True, text=True, check=True
        ).stdout.split()
        if not p.startswith("docs/vendor/")
    ]
    for path in prose:
        if os.path.exists(path):
            lines = read(path)
            for number, name in narrative_hits(path, lines):
                add("narrative", path, f"line {number}: {name}")

    return found


def load_baseline():
    counts = {}
    if not os.path.exists(BASELINE):
        return counts
    for line in read(BASELINE):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        check, count, path = line.split(None, 2)
        counts[(check, path)] = int(count)
    return counts


def print_baseline(found):
    print("# Shape guard baseline. `scripts/shape_guard.sh` fails when a count")
    print("# here is exceeded. Shrinking a count is the only edit this file")
    print("# should ever receive. Regenerate with `shape_guard.sh --print-baseline`.")
    for check, path in sorted(found):
        print(f"{check}\t{len(found[(check, path)])}\t{path}")


def main():
    found = collect()
    if "--print-baseline" in sys.argv:
        print_baseline(found)
        return 0

    baseline = load_baseline()
    failures = []
    slack = []
    for key in sorted(set(found) | set(baseline)):
        check, path = key
        actual = len(found.get(key, []))
        allowed = baseline.get(key, 0)
        if actual > allowed:
            failures.append((check, path, actual, allowed, found[key]))
        elif actual < allowed:
            slack.append((check, path, actual, allowed))

    for check, path, actual, allowed, details in failures:
        print(f"FAIL {check} {path}: {actual} violations, baseline allows {allowed}")
        for detail in details[:8]:
            print(f"       {detail}")
        if len(details) > 8:
            print(f"       ... and {len(details) - 8} more")

    for check, path, actual, allowed in slack:
        print(f"note: {check} {path} is down to {actual} from {allowed}; shrink {BASELINE}")

    if failures:
        print()
        print(f"shape guard: {len(failures)} file/check pairs over baseline")
        return 1
    print("shape guard: ok")
    return 0


sys.exit(main())
PYTHON
