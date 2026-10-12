#!/usr/bin/env node
// Devnet-only, journaled issuance with exact V2 allocations and a fixed mint cap.
import fs from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';

const args = Object.fromEntries(process.argv.slice(2).reduce((pairs, value, index, values) => {
  if (value.startsWith('--')) pairs.push([value.slice(2), values[index + 1]]);
  return pairs;
}, []));
for (const key of ['wallet', 'policy', 'state', 'report']) {
  if (!args[key]) throw new Error(`Missing --${key}`);
}
const mode = args.mode || 'prepare';
if (!['prepare', 'create', 'verify'].includes(mode)) throw new Error('Invalid mode');
const dependencyRoot = args['dependency-root'] || path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(path.resolve(dependencyRoot, 'package.json'));
const web3 = require('@solana/web3.js');
const token = require('@solana/spl-token');
const mpl = require('@metaplex-foundation/mpl-token-metadata');
const umiCore = require('@metaplex-foundation/umi');
const {createUmi} = require('@metaplex-foundation/umi-bundle-defaults');
const base58 = require('bs58');
const RPC = 'https://api.devnet.solana.com';
const GENESIS = 'EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG';
const URI = 'https://flybrain.work/assets/brand/fbw-devnet-v1.json';
const IMAGE = 'https://flybrain.work/assets/brand/flybrainlabs-logo-v1-500.png';
const hash = value => createHash('sha256').update(value).digest('hex');
const policyBytes = fs.readFileSync(args.policy);
const policy = JSON.parse(policyBytes);
const poolNames = ['miners', 'verifiers', 'community'];
const cap = BigInt(policy.capAtoms);
const amounts = poolNames.map(name => BigInt(policy.pools[name]));
if (policy.schema !== 'flybrain-economic-policy-v2' || policy.asset.decimals !== 8 ||
    cap !== 31183324300000000n || amounts.reduce((a, b) => a + b, 0n) !== cap ||
    amounts.some(amount => amount <= 0n || amount > 0xffffffffffffffffn)) {
  throw new Error('The frozen V2 supply, precision and pool amounts are required');
}

function privatePath(filename) {
  let current = path.resolve(filename);
  while (true) {
    if (fs.existsSync(path.join(current, '.git'))) throw new Error('Private state must be outside repositories');
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
}
privatePath(args.wallet);
privatePath(args.state);
if ((fs.statSync(args.wallet).mode & 0o077) !== 0) throw new Error('Wallet file permissions must be 0600');
const wallet = web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(args.wallet, 'utf8'))));
const stateDir = path.resolve(args.state);
fs.mkdirSync(stateDir, {recursive: true, mode: 0o700});
fs.chmodSync(stateDir, 0o700);
function keypair(name) {
  const filename = path.join(stateDir, `${name}.json`);
  if (!fs.existsSync(filename)) {
    const value = web3.Keypair.generate();
    fs.writeFileSync(filename, JSON.stringify(Array.from(value.secretKey)), {flag: 'wx', mode: 0o600});
  }
  if ((fs.statSync(filename).mode & 0o077) !== 0) throw new Error('Private key file permissions must be 0600');
  return web3.Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(filename, 'utf8'))));
}
const mint = keypair('mint');
const pools = poolNames.map(name => keypair(`pool-${name}`));
const journalFile = path.join(stateDir, 'journal.json');
const identity = {
  network: 'devnet', genesisHash: GENESIS, name: 'flybrainwork', symbol: 'FBW', decimals: 8,
  mint: mint.publicKey.toBase58(), owner: wallet.publicKey.toBase58(), capAtoms: cap.toString(),
  policySha256: hash(policyBytes), metadataUri: URI,
  pools: Object.fromEntries(poolNames.map((name, i) => [name, {address: pools[i].publicKey.toBase58(), atoms: amounts[i].toString()}]))
};
let journal = fs.existsSync(journalFile) ? JSON.parse(fs.readFileSync(journalFile, 'utf8')) : {identity, transactions: {}};
if (JSON.stringify(journal.identity) !== JSON.stringify(identity)) throw new Error('Private deployment identity changed');
function save() {
  const temporary = journalFile + '.tmp';
  fs.writeFileSync(temporary, JSON.stringify(journal, null, 2) + '\n', {mode: 0o600});
  fs.renameSync(temporary, journalFile);
}
save();
const connection = new web3.Connection(RPC, {commitment: 'finalized', confirmTransactionInitialTimeout: 90_000});
if (await connection.getGenesisHash() !== GENESIS) throw new Error('RPC is not Solana Devnet');
const umi = createUmi(RPC).use(mpl.mplTokenMetadata());
const signer = umiCore.createSignerFromKeypair(umi, umi.eddsa.createKeypairFromSecretKey(wallet.secretKey));
umi.use(umiCore.keypairIdentity(signer));
const metadataInstructions = mpl.createV1(umi, {
  mint: umiCore.publicKey(mint.publicKey.toBase58()), authority: signer, payer: signer,
  updateAuthority: signer.publicKey, name: identity.name, symbol: identity.symbol, uri: URI,
  sellerFeeBasisPoints: umiCore.percentAmount(0), tokenStandard: mpl.TokenStandard.Fungible,
  decimals: 8, isMutable: true, splTokenProgram: umiCore.publicKey(token.TOKEN_PROGRAM_ID.toBase58())
}).getInstructions().map(instruction => new web3.TransactionInstruction({
  programId: new web3.PublicKey(instruction.programId),
  keys: instruction.keys.map(key => ({pubkey: new web3.PublicKey(key.pubkey), isSigner: key.isSigner, isWritable: key.isWritable})),
  data: Buffer.from(instruction.data)
}));

async function submit(stage, instructions, signers = []) {
  let record = journal.transactions[stage];
  if (!record) {
    const block = await connection.getLatestBlockhash('finalized');
    const transaction = new web3.Transaction({feePayer: wallet.publicKey, ...block}).add(...instructions);
    transaction.sign(wallet, ...signers);
    const raw = transaction.serialize();
    if (raw.length > 1232) throw new Error('Transaction exceeds the packet limit');
    const simulation = await connection.simulateTransaction(transaction);
    if (simulation.value.err) throw new Error(`Devnet simulation failed: ${JSON.stringify(simulation.value.err)} ${simulation.value.logs?.join('\n')}`);
    record = {signature: base58.encode(transaction.signature), blockhash: block.blockhash,
              lastValidBlockHeight: block.lastValidBlockHeight, rawBase64: raw.toString('base64'), status: 'prepared'};
    journal.transactions[stage] = record;
    save();
  }
  const prior = (await connection.getSignatureStatuses([record.signature], {searchTransactionHistory: true})).value[0];
  if (prior?.err) throw new Error(`Recorded transaction failed: ${JSON.stringify(prior.err)}`);
  if (prior?.confirmationStatus !== 'finalized') {
    if (!prior) {
      // Re-broadcast the same signed bytes after an uncertain response, never a new mint.
      record.status = 'submitted';
      save();
      const actual = await connection.sendRawTransaction(Buffer.from(record.rawBase64, 'base64'), {skipPreflight: false, maxRetries: 3});
      if (actual !== record.signature) throw new Error('Submitted signature differs from the private journal');
    }
    const confirmation = await connection.confirmTransaction({signature: record.signature,
      blockhash: record.blockhash, lastValidBlockHeight: record.lastValidBlockHeight}, 'finalized');
    if (confirmation.value.err) throw new Error(`Transaction failed: ${JSON.stringify(confirmation.value.err)}`);
  }
  record.status = 'finalized';
  save();
  console.log(JSON.stringify({stage, signature: record.signature, finalized: true}));
  return record.signature;
}

if (mode === 'create') {
  const metadataResponse = await fetch(URI);
  if (!metadataResponse.ok) throw new Error('Public metadata must be deployed before issuance');
  const metadata = await metadataResponse.json();
  if (metadata.name !== identity.name || metadata.symbol !== identity.symbol || metadata.image !== IMAGE) throw new Error('Unexpected public token metadata');
  const logoResponse = await fetch(IMAGE);
  if (!logoResponse.ok || !logoResponse.headers.get('content-type')?.startsWith('image/png')) throw new Error('Public logo is unavailable');
  const balances = await connection.getMultipleAccountsInfo([mint.publicKey, ...pools.map(pool => pool.publicKey)], 'finalized');
  if (balances.every(account => account === null)) {
    const [mintRent, accountRent] = await Promise.all([
      connection.getMinimumBalanceForRentExemption(token.MINT_SIZE),
      connection.getMinimumBalanceForRentExemption(token.ACCOUNT_SIZE)
    ]);
    const instructions = [web3.SystemProgram.createAccount({fromPubkey: wallet.publicKey, newAccountPubkey: mint.publicKey,
      lamports: mintRent, space: token.MINT_SIZE, programId: token.TOKEN_PROGRAM_ID}),
      token.createInitializeMint2Instruction(mint.publicKey, 8, wallet.publicKey, null)];
    for (const pool of pools) {
      instructions.push(web3.SystemProgram.createAccount({fromPubkey: wallet.publicKey, newAccountPubkey: pool.publicKey,
        lamports: accountRent, space: token.ACCOUNT_SIZE, programId: token.TOKEN_PROGRAM_ID}),
        token.createInitializeAccount3Instruction(pool.publicKey, mint.publicKey, wallet.publicKey));
    }
    await submit('create-mint-and-pools', instructions, [mint, ...pools]);
  } else if (balances.some(account => account === null)) {
    throw new Error('Partial account state requires explicit reconciliation');
  }
  const current = await token.getMint(connection, mint.publicKey, 'finalized');
  for (const pool of pools) {
    const account = await token.getAccount(connection, pool.publicKey, 'finalized');
    if (!account.owner.equals(wallet.publicKey) || !account.mint.equals(mint.publicKey)) throw new Error('Unexpected pool account');
  }
  if (current.supply === 0n && current.mintAuthority?.equals(wallet.publicKey)) {
    const instructions = [web3.ComputeBudgetProgram.setComputeUnitLimit({units: 400_000}), ...metadataInstructions];
    for (let i = 0; i < pools.length; i++) instructions.push(token.createMintToInstruction(mint.publicKey, pools[i].publicKey, wallet.publicKey, amounts[i]));
    instructions.push(token.createSetAuthorityInstruction(mint.publicKey, wallet.publicKey, token.AuthorityType.MintTokens, null));
    await submit('metadata-and-fixed-supply', instructions);
  } else if (current.supply !== cap || current.mintAuthority !== null) {
    throw new Error('Unexpected supply or authority; issuance will not be repeated');
  }
}

const report = {schema: 'flybrainlabs-fbw-devnet-v1', project: 'flybrainlabs', ...identity,
  rpc: RPC, image: IMAGE, totalSupply: '311833243', sourcePolicyId: policy.policyId,
  sourcePolicyAssetSymbol: policy.asset.symbol,
  custody: 'Three separate Devnet token accounts controlled by the supplied test wallet; not service-contract escrow',
  explorer: `https://explorer.solana.com/address/${identity.mint}?cluster=devnet`,
  transactions: Object.fromEntries(Object.entries(journal.transactions).map(([stage, record]) => [stage, {signature: record.signature, status: record.status}]))};
if (mode !== 'prepare') {
  const [onchainMint, accounts, metadata] = await Promise.all([
    token.getMint(connection, mint.publicKey, 'finalized'),
    Promise.all(pools.map(pool => token.getAccount(connection, pool.publicKey, 'finalized'))),
    mpl.safeFetchMetadataFromSeeds(umi, {mint: umiCore.publicKey(identity.mint)}, {commitment: 'finalized'})
  ]);
  if (onchainMint.supply !== cap || onchainMint.decimals !== 8 || onchainMint.mintAuthority !== null || onchainMint.freezeAuthority !== null) throw new Error('On-chain cap, precision or authority verification failed');
  if (accounts.some((account, i) => account.amount !== amounts[i] || !account.owner.equals(wallet.publicKey) || !account.mint.equals(mint.publicKey))) throw new Error('On-chain pool allocation verification failed');
  if (!metadata || metadata.name.replace(/\0/g, '') !== identity.name || metadata.symbol.replace(/\0/g, '') !== identity.symbol || metadata.uri.replace(/\0/g, '') !== URI || metadata.updateAuthority !== identity.owner || metadata.tokenStandard.value !== mpl.TokenStandard.Fungible) throw new Error('On-chain fungible metadata verification failed');
  report.verification = {finalized: true, exactSupply: true, exactPools: true, decimals: 8,
    mintAuthorityRevoked: true, freezeAuthority: null, metadataAccount: metadata.publicKey,
    metadataName: metadata.name, metadataSymbol: metadata.symbol, metadataUri: metadata.uri,
    tokenStandard: 'Fungible', metadataUpdateAuthority: metadata.updateAuthority};
}
fs.mkdirSync(path.dirname(path.resolve(args.report)), {recursive: true});
fs.writeFileSync(args.report, JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify({mode, network: identity.network, mint: identity.mint, name: identity.name,
  symbol: identity.symbol, totalSupply: report.totalSupply, verified: Boolean(report.verification)}));
