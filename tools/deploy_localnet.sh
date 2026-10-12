#!/usr/bin/env bash
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
cli=${FLYPOW_SOLANA_CLI:-solana}
[[ -f target/deploy/flypow_campaign_program.so ]] || { echo 'Build the SBF program first.' >&2; exit 1; }
mkdir -p deploy/localnet
node --input-type=module <<'JS'
import { Keypair } from '@solana/web3.js';
import { access,writeFile } from 'node:fs/promises';
try { await access('deploy/localnet/payer.json'); }
catch { await writeFile('deploy/localnet/payer.json',JSON.stringify(Array.from(Keypair.generate().secretKey)),{flag:'wx',mode:0o600}); }
try { await access('deploy/localnet/program-buffer.json'); }
catch { await writeFile('deploy/localnet/program-buffer.json',JSON.stringify(Array.from(Keypair.generate().secretKey)),{flag:'wx',mode:0o600}); }
JS
"$cli" airdrop 100 --url http://127.0.0.1:8899 --keypair deploy/localnet/payer.json
"$cli" program deploy target/deploy/flypow_campaign_program.so --buffer deploy/localnet/program-buffer.json --program-id target/deploy/flypow_campaign_program-keypair.json --url http://127.0.0.1:8899 --keypair deploy/localnet/payer.json --fee-payer deploy/localnet/payer.json
