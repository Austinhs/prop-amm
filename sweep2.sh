#!/usr/bin/env bash
# Grid-sweep two consts at once.
# Usage: N=500 ./sweep2.sh strategies/v2_volspread.rs K_PCT "100 150 200" MIN_SPREAD_BPS "10 20 30"
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
STRAT="$(realpath "$1")"; C1="$2"; V1="$3"; C2="$4"; V2="$5"
N="${N:-500}"
mkdir -p "$HERE/.sweep"
for a in $V1; do
  for b in $V2; do
    tmp="$HERE/.sweep/$(basename "$STRAT" .rs)__${C1}_${a}__${C2}_${b}.rs"
    sed -E -e "s/^(const ${C1}: [^=]+= )[^;]+;/\1${a};/" \
           -e "s/^(const ${C2}: [^=]+= )[^;]+;/\1${b};/" "$STRAT" > "$tmp"
    grep -q "^const ${C1}: .*= ${a};" "$tmp" || { echo "const ${C1} not found"; exit 1; }
    grep -q "^const ${C2}: .*= ${b};" "$tmp" || { echo "const ${C2} not found"; exit 1; }
    NOTE="grid ${C1}=${a} ${C2}=${b}" "$HERE/eval.sh" "$tmp" "$N"
  done
done
