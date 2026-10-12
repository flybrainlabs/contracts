#!/usr/bin/env bash
set -euo pipefail
ledger=${1:?Usage: tools/start_localnet.sh NEW_LEDGER_DIRECTORY}
validator=${FLYPOW_VALIDATOR_PATH:-solana-test-validator}
if [[ -e "$ledger" ]]; then
  echo 'Choose a new ledger directory. This script never resets an existing ledger.' >&2
  exit 1
fi
if ! command -v "$validator" >/dev/null; then
  echo 'Install the locked Agave tools or set FLYPOW_VALIDATOR_PATH to a compatible local build.' >&2
  exit 1
fi
mkdir -p "$ledger"
export RAYON_NUM_THREADS=${RAYON_NUM_THREADS:-4}
exec "$validator" --ledger "$ledger" --bind-address 127.0.0.1 --rpc-port 8899 --dynamic-port-range 11000-11040 --quiet
