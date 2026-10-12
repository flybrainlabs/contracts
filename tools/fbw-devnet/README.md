# FBW Devnet issuance

The owner requested the supplied test wallet and monochrome logo for a token
named **flybrainwork**, with symbol **FBW**, on Solana Devnet. The project and
GitHub owner remain **flybrainlabs**.

The V2 economic policy provides eight decimal places and exactly 31,183,324,300,000,000
atoms (311,833,243 tokens):

| Pool | Tokens | Atoms |
| --- | ---: | ---: |
| Miners | 224,519,936 | 22,451,993,600,000,000 |
| Verifiers | 56,129,984 | 5,612,998,400,000,000 |
| Community | 31,183,323 | 3,118,332,300,000,000 |

Issuance creates a classic SPL mint and three separate token accounts owned by
the supplied test wallet. A second atomic transaction creates standard Metaplex
fungible metadata, mints the exact pool amounts and revokes mint authority.
There is no freeze authority. Metadata update authority stays with the wallet.
These accounts provide test custody; no service-contract escrow or beneficiary
claims are deployed by this tool. Existing FLY policy and production settings
are not changed by the FBW test asset.

## Run and reconcile

Install the locked tool dependencies in an external tools directory, outside the
maintained repositories. Copy this directory's `package.json` and
`package-lock.json` there and run `npm ci --ignore-scripts` in that directory.
Umi and its plugins are pinned together to avoid incompatible transitive SDKs.

```sh
node tools/fbw-devnet/create_token.mjs \
  --mode prepare \
  --wallet /secure/test-wallet.json \
  --policy /build/workspace/web/data/economic-policy-v2.json \
  --state /secure/fbw-devnet-state \
  --report /private/reports/fbw-prepare.json \
  --dependency-root /external/tools/fbw-devnet
```

`prepare` checks the Devnet genesis and policy and retains private mint/account
keys without sending transactions. Before `create`, deploy the public logo and
metadata at the exact URLs in the tool. `create` simulates, journals and confirms
both transactions at finalized commitment. Reuse the same private state directory
after interruption. An uncertain submission reuses identical signed bytes;
unexpected supply, partial accounts or failed receipts stop reconciliation.
`verify` reads the finalized supply, pool balances, authorities and Metaplex
metadata without minting again.

Wallet and mint keys and the signed-transaction journal remain in restricted
external files. Public deployment records contain only account addresses,
transaction signatures, asset parameters and verification outcomes. The supplied
logo's readable 500px preview is recorded separately from an unavailable original.

The actual finalized deployment receipt is in `deploy/fbw-devnet.json` at the
contract repository root after successful issuance.
