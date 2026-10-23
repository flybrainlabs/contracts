# FBW validator bond V1

This is a separate Devnet collateral program. It does not change the existing
FBW mint, global supply, economic pools, historical receipts or settlement code.

## Fixed policy

- Minimum collateral: 100,000 FBW, or 10,000,000,000,000 atoms at eight decimals.
- The validator's scientific Ed25519 public key is also its bond owner address.
- Activation starts a minimum 30-day term, using the on-chain Clock.
- After that term the owner can request exit. New-work admission stops once the
  exit is finalized. Previously admitted work and evidence remain recoverable.
- Withdrawal starts at least seven days after the request: earliest day 37.
- Withdrawal also requires the owner's signature and the initialized clearance
  authority's co-signature over the exact transaction, generation and nonzero
  evidence hash. Time alone does not certify that outstanding duties are closed.
- Cleared principal goes only to a token account owned by that validator.
- Re-entry increments the generation and starts a fresh 30-day term.

The clearance authority reviews outstanding tasks, signed admissions, unresolved
crashes and disputes before co-signing. This is a trusted operational role; the
program cannot determine scientific liabilities from a hash. Missing clearance
can delay withdrawal beyond seven days. It cannot seize or redirect collateral.
There is no slashing, yield, policy update, partial withdrawal or admin bypass in
this version. Any future penalty policy requires a separate reviewed migration.
The Devnet loader upgrade authority is retained, so this is not an immutable or
independently audited custody deployment.

## Build and acceptance

Build this standalone crate with the retained Solana toolchain:

```sh
cargo-build-sbf --manifest-path programs/fbw-validator-bond/Cargo.toml --arch v3 --offline --sbf-out-dir BOND_ARTIFACTS
BOND_SBF=BOND_ARTIFACTS/fbw_validator_bond.so node tests/validator_bond_sbf.mjs
```

Assemble the pinned component workspace before the JavaScript suite. It executes
the compiled SBF and SPL instructions in LiteSVM, advances only the isolated VM
clock, and exercises signed fixture HTTP reports against actual bonded state.
It does not claim a public-chain exit after 37 days or miner reward acceptance.
Keep build keypairs, signed transport journals and deployment identities private.
