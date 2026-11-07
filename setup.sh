#!/usr/bin/env bash
# One-time setup: clone the official simulator and build its CLI.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"

if [ ! -d prop-amm-challenge ]; then
  git clone https://github.com/benedictbrady/prop-amm-challenge
fi

cd prop-amm-challenge
git pull --ff-only || true
cargo build --release -p prop-amm
echo
echo "Built: $HERE/prop-amm-challenge/target/release/prop-amm"
echo "CPU threads available: $(nproc)"

if ! cargo build-sbf --version >/dev/null 2>&1; then
  echo
  echo "NOTE: Solana's cargo build-sbf isn't installed. Native runs work without it,"
  echo "but 'validate' (native/BPF parity) and --bpf runs need it before submitting:"
  echo '  sh -c "$(curl -sSfL https://release.anza.xyz/stable/install)"'
fi
