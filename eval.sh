#!/usr/bin/env bash
# Score a strategy on two seed blocks and log the result.
#   dev     = seeds 0..N-1           (what you tune against)
#   holdout = seeds 1000000..+N-1    (never tune against; stands in for the server's unseen seeds)
# If dev improves but holdout doesn't, you're overfitting.
#
# Usage: ./eval.sh strategies/v1_recenter.rs [N=1000]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
STRAT="$(realpath "$1")"
N="${2:-1000}"
WORKERS="${WORKERS:-$(nproc)}"
CLI="$HERE/prop-amm-challenge/target/release/prop-amm"
[ -x "$CLI" ] || { echo "Run ./setup.sh first"; exit 1; }

# The CLI builds in ./.build relative to the challenge repo, so run from there.
cd "$HERE/prop-amm-challenge"

run_block() {
  local seed="$1" out
  out="$("$CLI" run "$STRAT" --simulations "$N" --seed-start "$seed" --workers "$WORKERS" 2>&1)" || {
    echo "$out" >&2; exit 1; }
  echo "$out" | awk '/Avg edge:/ {print $3}'
}

DEV="$(run_block 0)"
HOLD="$(run_block 1000000)"
SHA="$(sha1sum "$STRAT" | cut -c1-8)"

printf "%-40s  N=%-5s  dev=%9s  holdout=%9s\n" "$(basename "$STRAT")" "$N" "$DEV" "$HOLD"

LOG="$HERE/results.csv"
[ -f "$LOG" ] || echo "time,strategy,sha,n,dev_avg_edge,holdout_avg_edge,note" > "$LOG"
echo "$(date -Iseconds),$(basename "$STRAT"),$SHA,$N,$DEV,$HOLD,${NOTE:-}" >> "$LOG"
