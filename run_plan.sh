#!/usr/bin/env bash
# Run every experiment in a plan file, then print a ranked scoreboard.
#
# Usage:  ./run_plan.sh plans/v2.plan            (N defaults to 500 sims per seed block)
#         N=1000 ./run_plan.sh plans/v2.plan
#
# Plan file format, one experiment line per row (# comments and blank lines ignored):
#   <strategy file in strategies/>  [CONST=v1,v2,...]  [CONST2=a,b] ...
# Comma lists expand to every combination. A line can also be:  set N=1000
#
# Output: runs/<timestamp>/summary.tsv, the variant .rs files, and best.rs (the top
# scorer on dev). Every run is also appended to results.csv as before.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PLAN="$(realpath "$1")"
N="${N:-500}"
RUN_ID="$(date +%Y%m%d-%H%M%S)"
OUT="$HERE/runs/$RUN_ID"
mkdir -p "$OUT/variants"
SUMMARY="$OUT/summary.tsv"
: > "$SUMMARY"

make_variant() { # src dst KEY=VAL...
  local src="$1" dst="$2"; shift 2
  cp "$src" "$dst"
  local kv k v
  for kv in "$@"; do
    k="${kv%%=*}"; v="${kv#*=}"
    sed -i -E "s/^(const ${k}: [^=]+= )[^;]+;/\1${v};/" "$dst"
    grep -q "^const ${k}: .*= ${v};" "$dst" || { echo "  ! const ${k} not found in $(basename "$src")" >&2; return 1; }
  done
}

expand() { # "fixed assigns" then remaining KEY=a,b tokens -> one combo per output line
  local fixed="$1"; shift
  if [ $# -eq 0 ]; then echo "$fixed"; return; fi
  local tok="$1"; shift
  local k="${tok%%=*}" vals="${tok#*=}" v
  local -a arr
  IFS=',' read -ra arr <<< "$vals"
  for v in "${arr[@]}"; do expand "$fixed $k=$v" "$@"; done
}

# Collect all jobs first so we can show progress.
JOBS=()
while IFS= read -r line || [ -n "$line" ]; do
  line="${line%%#*}"
  read -r -a toks <<< "$line"
  [ ${#toks[@]} -eq 0 ] && continue
  if [ "${toks[0]}" = "set" ]; then JOBS+=("SET ${toks[1]}"); continue; fi
  strat="${toks[0]}"
  while IFS= read -r combo; do
    JOBS+=("RUN $strat$combo")
  done < <(expand "" "${toks[@]:1}")
done < "$PLAN"

TOTAL=0; for j in "${JOBS[@]}"; do [[ $j == RUN* ]] && TOTAL=$((TOTAL+1)); done
echo "Plan: $(basename "$PLAN")  |  $TOTAL experiments  |  run id $RUN_ID"
START=$(date +%s); I=0

for job in "${JOBS[@]}"; do
  if [[ $job == SET* ]]; then
    kv="${job#SET }"; [ "${kv%%=*}" = "N" ] && N="${kv#*=}"; continue
  fi
  read -r -a parts <<< "${job#RUN }"
  strat="${parts[0]}"; assigns=("${parts[@]:1}")
  I=$((I+1))
  label="${strat%.rs}"
  [ ${#assigns[@]} -gt 0 ] && label="$label ${assigns[*]}"
  safe="$(echo "$label" | tr ' =' '_-')"
  variant="$OUT/variants/$safe.rs"

  printf "[%d/%d] %s (N=%s) ... " "$I" "$TOTAL" "$label" "$N"
  if ! make_variant "$HERE/strategies/$strat" "$variant" "${assigns[@]}"; then
    echo "SKIPPED"; printf "%s\t%s\t%s\t%s\t%s\n" "-" "-" "$N" "$label" "BAD-CONST" >> "$SUMMARY"; continue
  fi
  if res="$(NOTE="$RUN_ID $label" "$HERE/eval.sh" "$variant" "$N" 2>"$OUT/variants/$safe.err")"; then
    dev="$(echo "$res" | sed -E 's/.*dev= *([-0-9.]+).*/\1/')"
    hold="$(echo "$res" | sed -E 's/.*holdout= *([-0-9.]+).*/\1/')"
    echo "dev $dev  holdout $hold"
    printf "%s\t%s\t%s\t%s\t%s\n" "$dev" "$hold" "$N" "$label" "$variant" >> "$SUMMARY"
    rm -f "$OUT/variants/$safe.err"
  else
    echo "FAILED (see runs/$RUN_ID/variants/$safe.err)"
    printf "%s\t%s\t%s\t%s\t%s\n" "-" "-" "$N" "$label" "FAILED" >> "$SUMMARY"
  fi
done

echo
echo "=================== SCOREBOARD (ranked by dev; holdout = check) ==================="
printf "%-5s %10s %10s %6s  %s\n" "rank" "dev" "holdout" "N" "experiment"
sort -t$'\t' -k1,1 -g -r "$SUMMARY" | awk -F'\t' '{printf "%-5s %10s %10s %6s  %s\n", NR, $1, $2, $3, $4}'
BEST="$(grep -v -P '^-\t' "$SUMMARY" | sort -t$'\t' -k1,1 -g -r | head -1 | cut -f5)"
if [ -n "$BEST" ] && [ -f "$BEST" ]; then cp "$BEST" "$OUT/best.rs"; echo; echo "Best variant copied to runs/$RUN_ID/best.rs"; fi
echo "Took $(( $(date +%s) - START ))s. Summary: runs/$RUN_ID/summary.tsv"
