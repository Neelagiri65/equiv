#!/usr/bin/env bash
# Build the equiv CLI and score it against the divergence corpus (divergences/).
# Extra arguments are passed to scripts/run_divergences.py, e.g.
#   scripts/run-divergences.sh --update-baseline
#   scripts/run-divergences.sh --only checkdigit
# Set EQUIV_BIN to score an existing binary instead of building one.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [[ -z "${EQUIV_BIN:-}" ]]; then
  cargo build --release --quiet -p equiv-cli
  EQUIV_BIN="$ROOT/target/release/equiv"
fi

exec python3 "$ROOT/scripts/run_divergences.py" --bin "$EQUIV_BIN" "$@"
