#!/usr/bin/env bash
# Try several values of one `const` in a strategy and score each.
# Usage: N=200 ./sweep.sh strategies/cp_fee.rs FEE_BPS 20 30 50 80 120
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
STRAT="$(realpath "$1")"; CONST="$2"; shift 2
N="${N:-200}"
mkdir -p "$HERE/.sweep"
for v in "$@"; do
  tmp="$HERE/.sweep/$(basename "$STRAT" .rs)__${CONST}_${v}.rs"
  sed -E "s/^(const ${CONST}: [^=]+= )[^;]+;/\1${v};/" "$STRAT" > "$tmp"
  grep -q "^const ${CONST}: .*= ${v};" "$tmp" || { echo "const ${CONST} not found in $STRAT"; exit 1; }
  NOTE="sweep ${CONST}=${v}" "$HERE/eval.sh" "$tmp" "$N"
done
