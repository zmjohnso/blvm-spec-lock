#!/usr/bin/env bash
# Publish blvm-spec-lock packages to crates.io (core first — required by the proc-macro crate).
#
# Prerequisites:
#   cargo login   # once per machine; or set CARGO_REGISTRY_TOKEN for CI
#
# Usage:
#   ./scripts/publish-crates-io.sh
#   ALLOW_DIRTY=1 ./scripts/publish-crates-io.sh   # only if you intentionally publish uncommitted changes
#
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

EXTRA=()
if [[ -n "$(git status --porcelain 2>/dev/null)" ]]; then
  if [[ "${ALLOW_DIRTY:-}" == "1" ]]; then
    EXTRA+=(--allow-dirty)
    echo "⚠️  Publishing with --allow-dirty (uncommitted changes included)."
  else
    echo "❌ Working tree has uncommitted changes. Commit first, or rerun with ALLOW_DIRTY=1." >&2
    exit 1
  fi
fi

echo "📦 Publishing blvm-spec-lock-core…"
cargo publish -p blvm-spec-lock-core "${EXTRA[@]}"

echo "📦 Publishing blvm-spec-lock (proc-macro + cargo-spec-lock CLI)…"
cargo publish -p blvm-spec-lock "${EXTRA[@]}"

echo "✅ Done. Confirm on https://crates.io/crates/blvm-spec-lock and https://crates.io/crates/blvm-spec-lock-core"
