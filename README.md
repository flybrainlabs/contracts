# flybrainlabs contracts

Solana programs for [flybrainlabs](https://flybrain.work/).

## Scope

The preserved V1 campaign program and the independent V2 global issuance/finite-budget settlement program. Existing account layouts and authorities are retained. Service operations must not mint new tokens or reinterpret old receipts.

## Source map

- `programs/flypow-campaign/`
- `programs/flypow-economy-v2/`
- `tests/campaign_localnet.mjs`
- `tests/economy_v2_localnet.mjs`

Each maintained file has one owning repository. `component.json` maps maintained
files into the shared build layout. Dependencies are pinned by commit in the
[infrastructure release manifest](https://github.com/flybrainlabs/infra/blob/main/release-manifest.json).

## Build and test

Use Rust 1.90 or newer (the source uses edition 2024), Node 22 or newer and Python
3.11 or newer. Optional scientific Python dependencies are in
`tools/requirements-research.txt` in the assembled workspace. SBF builds require
the compatible Solana/Agave build tools.

First assemble the exact source revisions:

```sh
git clone https://github.com/flybrainlabs/infra.git flybrainlabs/infra
python3 flybrainlabs/infra/tools/workspace.py checkout --repos flybrainlabs
python3 flybrainlabs/infra/tools/workspace.py assemble --repos flybrainlabs --output flybrainlabs-workspace
cd flybrainlabs-workspace
npm ci
```

Run the applicable checks from the assembled workspace:

```sh
cargo check -p flypow-campaign-program -p flypow-economy-v2-program
cargo-build-sbf --manifest-path programs/flypow-economy-v2/Cargo.toml
```

The assembled layout keeps cross-component import paths intact. Cloning
this repository alone does not supply its external components. The assembler
creates a disposable build layout; all maintained edits belong to the owning
repositories. Use `workspace.py sync` for intentional changes made in that layout,
then review the component diffs and validate them before publication.

Full-model and browser integration checks require existing, verified local
resources and explicit arguments. They do not download a model or reset a
browser profile by default. Isolated chain checks require their own local bank;
production funds and addresses are never substituted.

## Development policy

- All maintained documentation, commit messages, issues, PRs and release notes
  are written in English. Product interfaces may remain bilingual.
- Completed, verified updates are committed and pushed automatically to
  `flybrainlabs/contracts` using normal fast forwards.
- Keys, wallets, customer inputs, node databases, private artifacts, browser
  profiles and model caches stay outside Git and website exports.
- Production payment and reward configuration remains disabled until its actual
  asset identities, authorities, funds and operating gates are satisfied.

See [AGENTS.md](AGENTS.md), the
[development guide](https://github.com/flybrainlabs/docs/blob/main/docs/DEVELOPMENT.md),
[architecture](https://github.com/flybrainlabs/docs/blob/main/docs/ARCHITECTURE.md)
and [status](https://github.com/flybrainlabs/docs/blob/main/docs/STATUS.md).

## License and provenance

Project code retains the declared `MIT OR Apache-2.0` license. Data descriptors
and third-party assets retain their own source attribution and licenses; this
code license does not relicense MaleCNS data or third-party libraries. The
maintained sources preserve numerical/identity fixtures and the meaning of
existing scientific and financial records. See [NOTICE](NOTICE).

## Wallet mining V3

The finite Solana Devnet wallet-mining extension is described in the
[execution and acceptance record](https://github.com/flybrainlabs/docs/blob/main/docs/WALLET_MINING_V3_EXECUTION.md)
and [canonical Result layout](https://github.com/flybrainlabs/docs/blob/main/docs/WALLET_MINING_V3_RESULT_LAYOUT.md).
Use the assembled workspace for cross-component imports and pinned release
verification. Public receipts are in the contracts repository under
`deploy/wallet-mining-v3/`; private signers, signed journals and full response
archives remain outside source repositories. The two-unit acceptance budget is
complete. Future paid work requires a new prospectively frozen finite plan.
Actual extension-wallet and physical-device confirmation remain separate gates.

## Contract task factory V4

The existing Devnet program now contains V4 at finalized slot `511257144`.
`deploy/task-factory-v4/devnet-program.json` records the exact executable hash,
preserved account data and loader receipts. `devnet-simulation.json` records a
read-only initialization/upload preview. No public factory or new FBW budget has
been activated; the deployed product UI still uses V3.

The opt-in V4 lifecycle freezes a predefined catalog on chain, accepts full replay
attestations and advances tasks without a privileged relayer or minimum successful-
task interval. Existing V3 records and grants are retained. Public activation is
pending an approved calibrated catalog and finite funding. See
[the lifecycle specification](https://github.com/flybrainlabs/docs/blob/feat/contract-task-factory-v4-20261015/docs/CONTRACT_TASK_FACTORY_V4.md).

## Scientific round V5 acceptance

The economic program appends instruction tag 12 and disjoint V5 accounts; previous
account layouts and instruction tags are retained. After building the program and
`exact_trial`, run `node tests/scientific_round_v5_sbf.mjs` in the assembled workspace.
Set `FLYBRAIN_POOL_TEST_SOCKET` to the dedicated isolated acceptance socket for the
HTTP/PostgreSQL/restart path. `SCIENCE_V5_NODE_BUNDLE` optionally checks the actual
self-contained maintenance package. These tests use real SBF, deterministic fixture
computation and virtual SPL funds. They never send public transactions or assert
biological validity. Reject SBF builds containing stack errors, even on exit zero.
