#!/usr/bin/env bash
# Parity oracle: the Rust rewrite's (skill_name, trigger_type) invocation
# counts for MAIN-session transcripts must exactly match the Python
# reference (experiments/python/src/skillscope). Subagent-origin rows are
# additive on top of that — the Python reference has no concept of
# subagent transcripts, so it can't be compared against them.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PYTHON_REPO="${PYTHON_REPO:-$REPO_ROOT/experiments/python}"
RUST_BIN="${RUST_BIN:-$REPO_ROOT/target/release/skillscope}"

if [[ ! -d "$PYTHON_REPO" ]]; then
  echo "Python reference repo not found at $PYTHON_REPO" >&2
  exit 1
fi
if [[ ! -x "$RUST_BIN" ]]; then
  echo "Rust binary not found at $RUST_BIN — run 'make build' first" >&2
  exit 1
fi

py_export="$(mktemp)"
rust_export="$(mktemp)"
trap 'rm -f "$py_export" "$rust_export"' EXIT

(cd "$PYTHON_REPO" && uv run skillscope export) >"$py_export"
"$RUST_BIN" export --origin main >"$rust_export"

uv run python3 - "$py_export" "$rust_export" <<'PYEOF'
import json
import sys
from collections import Counter

py_path, rust_path = sys.argv[1], sys.argv[2]


def load_counts(path):
    counts = Counter()
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            record = json.loads(line)
            counts[(record["skill_name"], record["trigger_type"])] += 1
    return counts


py_counts = load_counts(py_path)
rust_counts = load_counts(rust_path)

keys = set(py_counts) | set(rust_counts)
mismatches = [k for k in keys if py_counts.get(k, 0) != rust_counts.get(k, 0)]

py_total = sum(py_counts.values())
rust_total = sum(rust_counts.values())

print(f"Python (main-only):  {py_total} invocations across {len(py_counts)} (skill, trigger_type) pairs")
print(f"Rust (--origin main): {rust_total} invocations across {len(rust_counts)} (skill, trigger_type) pairs")

if mismatches:
    print(f"\nMISMATCH: {len(mismatches)} (skill, trigger_type) pairs differ:", file=sys.stderr)
    for k in sorted(mismatches):
        print(f"  {k}: python={py_counts.get(k, 0)} rust={rust_counts.get(k, 0)}", file=sys.stderr)
    sys.exit(1)

print("\nParity OK: every (skill_name, trigger_type) pair matches exactly.")
PYEOF
