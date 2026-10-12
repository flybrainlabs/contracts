// Real SBF acceptance on a retained, isolated validator. Scientific roots here
// are explicit fixtures; public acceptance must supply complete-model evidence.
import assert from 'node:assert/strict';
import {Connection,Keypair,PublicKey,SystemProgram,Transaction,sendAndConfirmTransaction,ComputeBudgetProgram,Ed25519Program} from '@solana/web3.js';
import {createMint,createAccount,mintTo,setAuthority,AuthorityType,getMint,getAccount,createTransferInstruction,burn,createAssociatedTokenAccountInstruction} from '@solana/spl-token';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {addresses,meta,instruction,signedInstruction,settlementKeys,system,instructionSysvar,TOKEN_PROGRAM_ID,decodeGrant,decodeGeneration,decodeUnit,decodeMiningStage,decodeResult,decodeStaging,intentMessage,encodeResult,winnerMessage,closureMessage,settlerPolicy,hash,text,hex,jsonValue,Writer} from '../tools/mining_v3_client.mjs';
import {decodeGlobal,instruction as oldInstruction,CAP,POOLS} from '../tools/economy_v2_client.mjs';

const keyRoot=process.env.MINING_V3_KEYS||'/workspace/.flypow-secrets/wallet-mining-v3-local',reportRoot=process.env.MINING_V3_REPORT||'/workspace/.flypow-publish/wallet-mining-v3/local';
await mkdir(reportRoot,{recursive:true});
const key=async n=>Keypair.fromSecretKey(Uint8Array.from(JSON.parse(await readFile(keyRoot+'/'+n+'.json','utf8'))));
const [project,relayer,miner,session,rogue,...settlers]=await Promise.all(['project','relayer','miner','session','rogue','settler-1','settler-2','settler-3'].map(key));
const c=new Connection(process.env.MINING_V3_RPC||'http://127.0.0.1:8899','confirmed');
const pid=Keypair.fromSecretKey(Uint8Array.from(JSON.parse(await readFile(process.env.MINING_V3_PROGRAM_KEY||'/workspace/.flypow-publish/wallet-mining-v3/sbf/flypow_economy_v2_program-keypair.json','utf8')))).publicKey,a=addresses(pid),genesis=await c.getGenesisHash();
if(!/^(http:\/\/127\.0\.0\.1|http:\/\/localhost):/.test(c.rpcEndpoint))throw new Error('Isolated test only');
const policy=hash(text('flybrainlabs-local-mining-v3-fixture')),stageId=hash(text('finite-local-sbf-stage')),stage=a.miningStage(stageId),trace=[],checks=[];let maxPacket=0,maxCU=0;
async function send(ixs,signers=[relayer],options={}){
  const tx=new Transaction().add(...(Array.isArray(ixs)?ixs:[ixs]));tx.feePayer=signers[0].publicKey;tx.recentBlockhash=(await c.getLatestBlockhash()).blockhash;tx.sign(...signers);maxPacket=Math.max(maxPacket,tx.serialize().length);
  const signature=await sendAndConfirmTransaction(c,tx,signers,{commitment:'confirmed',preflightCommitment:'confirmed',...options});
  const t=await c.getTransaction(signature,{commitment:'confirmed',maxSupportedTransactionVersion:0});maxCU=Math.max(maxCU,t?.meta?.computeUnitsConsumed||0);
  trace.push({signature,packetBytes:tx.serialize().length,computeUnits:t?.meta?.computeUnitsConsumed,feeLamports:String(t?.meta?.fee||0)});return signature;
}
async function reject(name,fn){await assert.rejects(fn);checks.push(name);}
async function state(address,decoder){const v=await c.getAccountInfo(address);assert(v&&v.owner.equals(pid));return decoder(v.data);}
const now=async()=>Number((await c.getBlockTime(await c.getSlot()))||Math.floor(Date.now()/1000));
for(const k of [project,relayer,miner,rogue,session])await c.confirmTransaction(await c.requestAirdrop(k.publicKey,k===project?30_000_000_000:2_000_000_000),'confirmed');
const mint=await createMint(c,project,project.publicKey,null,8),sources=[];
for(let n=0;n<3;n++){sources.push(await createAccount(c,project,mint,project.publicKey,Keypair.generate()));await mintTo(c,project,mint,sources[n],project,POOLS[n]);}
await setAuthority(c,project,mint,project,AuthorityType.MintTokens,null);
const programData=PublicKey.findProgramAddressSync([pid.toBytes()],new PublicKey('BPFLoaderUpgradeab1e11111111111111111111111'))[0],adoptKeys=[meta(mint),...[0,1,2].map(i=>meta(a.pool(i),true)),meta(project.publicKey,true,true),system(),meta(TOKEN_PROGRAM_ID),meta(programData)];
await reject('rogue cannot prepare adoption vaults',()=>send(instruction(pid,adoptKeys.map((k,i)=>i===4?meta(rogue.publicKey,true,true):k),0),[rogue]));
await send(instruction(pid,adoptKeys,0),[project]);
await send(sources.map((source,n)=>createTransferInstruction(source,a.pool(n),project.publicKey,POOLS[n])),[project]);
await send(oldInstruction(pid,[meta(a.global,true),meta(mint),...[0,1,2].map(i=>meta(a.pool(i))),meta(project.publicKey,true,true),system(),meta(programData)],8,new Writer().key(new PublicKey(genesis)).key(policy).bytes(settlers.flatMap(k=>Array.from(k.publicKey.toBytes()))).result()),[project]);
const g=await state(a.global,decodeGlobal);assert.equal(g.issued,true);assert.equal((await getMint(c,mint)).supply,CAP);checks.push('adopts the once-issued supply and all exact existing pools');
const cfg={relayer:relayer.publicKey,closeAuthority:project.publicKey,rentRecipient:project.publicKey,retentionSeconds:60n,maxGrantLifetime:3600n,maxDeposit:100_000_000n};
await send(instruction(pid,[meta(a.global),meta(a.config,true),meta(project.publicKey,true,true),system()],1,cfg),[project]);
await send(instruction(pid,[meta(a.global,true),meta(a.config),meta(stage,true),meta(project.publicKey,true,true),system()],2,{id:stageId,budget:50_000_000_000n,deadline:BigInt(await now()+3500)}),[project]);
const grantId=hash(text('single-wallet-finite-local-grant')),grant=a.grant(miner.publicKey,grantId),rent=BigInt(await c.getMinimumBalanceForRentExemption(1024)),baseGrant={id:grantId,generation:1n,session:session.publicKey,deposit:rent*8n,maxSpend:rent*4n,maxResultRent:rent,maxResults:4n,expiresAt:BigInt(await now()+1800)},grantKeys=[meta(a.global),meta(a.config),meta(a.generation(miner.publicKey),true),meta(grant,true),meta(miner.publicKey,true,true),meta(relayer.publicKey,true,true),system()];
await reject('login/session alone cannot fund a grant without the miner signer',()=>send(instruction(pid,grantKeys.map((k,i)=>i===4?meta(miner.publicKey,true):k),4,baseGrant)));
await reject('cannot sign an unlimited or greater-than-deposit maximum',()=>send(instruction(pid,grantKeys,4,{...baseGrant,maxSpend:baseGrant.deposit+1n}),[relayer,miner]));
await send(instruction(pid,grantKeys,4,baseGrant),[relayer,miner]);
assert.equal((await state(grant,decodeGrant)).available,baseGrant.deposit);checks.push('project pays auxiliary rent; miner principal equals only the finite deposit');
let unitCounter=0;
async function prepare(label,{amount=100_000_000n,currentGrant=grant,currentSession=session,generation=1n,partial=false,wrongChain=false,maxResultRent=rent}={}){
  const task=hash(text(label)),unit=a.unit(task),commitment=hash(text(label+'-commit')),intentAddress=a.intent(unit,miner.publicKey,commitment),staging=a.staging(unit,intentAddress);
  await send(instruction(pid,[meta(a.global),meta(a.config),meta(stage,true),meta(unit,true),meta(project.publicKey,true,true),system()],3,{economicTask:task,amount}),[project]);
  const i={genesis:wrongChain?hash(text('wrong-chain')):new PublicKey(genesis),program:pid,policy,unitAccount:unit,economicTask:task,unit:new Uint8Array(32),round:hash(text(label+'-round')),candidate:hash(text(label+'-candidate')),commitment,goalResult:hash(text(label+'-goal-result')),manifest:hash(text(label+'-manifest')),scientificIdentity:hash(text('fixture-scientific-identity')),miner:miner.publicKey,beneficiary:miner.publicKey,grant:baseGrant.id,grantGeneration:generation,session:currentSession.publicKey,rewardAtoms:amount,maxResultRent};
  const keys=[meta(a.global),meta(a.config),meta(unit),meta(currentGrant),meta(intentAddress,true),meta(relayer.publicKey,true,true),system(),instructionSysvar()];
  const register=()=>send(signedInstruction(pid,keys,9,{intent:i},intentMessage(i),currentSession));
  if(wrongChain){await reject('a genuine session signature for the wrong chain is rejected',register);return {unit};}
  await register();assert.equal(await c.getAccountInfo(a.result(unit)),null);checks.push('accepted intent creates no final result and consumes no principal');
  const roots=Array.from({length:25},(_,n)=>hash(text(label+'-root-'+n))),bindings={0:new PublicKey(genesis).toBytes(),1:pid.toBytes(),2:policy,3:stage.toBytes(),4:task,5:new Uint8Array(32),6:i.round,7:i.candidate,13:i.goalResult,14:i.manifest,16:miner.publicKey.toBytes(),17:miner.publicKey.toBytes(),18:baseGrant.id,19:currentSession.publicKey.toBytes(),20:unit.toBytes(),21:currentGrant.toBytes(),22:settlerPolicy(pid,g)};
  for(const [n,b]of Object.entries(bindings))roots[Number(n)]=b;
  const record={roots,originalGeneration:generation,roundSequence:BigInt(++unitCounter),claimSequence:1n,rewardAtoms:amount,rentLamports:0n,acceptedAt:0n,settledSlot:0n,settledAt:0n,requiredTrials:2,completeTrials:2,steps:256,neurons:165122,metricsVersion:1,metrics:[1n,2n,3n,4n,5n,6n,7n,8n,9n,10n,11n,12n,13n,0n,0n,0n]},blob=encodeResult(record),preparedHash=hash(blob);
  await send(instruction(pid,[meta(a.global),meta(a.config),meta(unit),meta(intentAddress),meta(staging,true),meta(relayer.publicKey,true,true),system()],10,{expectedHash:preparedHash}));
  const write=(offset,bytes)=>send(instruction(pid,[meta(a.global),meta(a.config),meta(staging,true),meta(relayer.publicKey,false,true)],11,{offset,bytes}));
  await reject('out-of-order staging chunks cannot be sealed',()=>write(100,blob.slice(100,200)));
  for(let offset=0;offset<1024;offset+=384)await write(offset,blob.slice(offset,Math.min(offset+384,1024)));
  await write(0,blob.slice(0,384));
  await reject('conflicting chunk retry cannot alter accepted bytes',()=>write(0,Uint8Array.from([255])));
  await send(instruction(pid,[meta(a.global),meta(a.config),meta(unit),meta(intentAddress),meta(staging,true),meta(relayer.publicKey,false,true)],12));
  const voteKeys=[meta(a.global),meta(a.config),meta(stage,true),meta(unit,true),meta(intentAddress),meta(staging,true),instructionSysvar()],message=winnerMessage(pid,g,unit,intentMessage(i),preparedHash);
  const vote=(signer,msg=message)=>send(signedInstruction(pid,voteKeys,13,{},msg,signer));
  const settleKeys=settlementKeys(a,{stage,unit,intent:intentAddress,staging,grant:currentGrant,mint,miner:miner.publicKey,relayer:relayer.publicKey});
  const settle=()=>send(instruction(pid,settleKeys,15));
  await reject('unverified candidate cannot settle',settle);await reject('unknown settler cannot approve a winner',()=>vote(rogue));
  await vote(settlers[0]);await vote(settlers[0]);assert.equal((await state(unit,decodeUnit)).accepted,false);await reject('one verifier repeated is not quorum',settle);
  if(!partial)await vote(settlers[1]);
  return {task,unit,i,intentAddress,staging,preparedHash,record,settleKeys,settle,vote,register};
}
await prepare('wrong-chain',{wrongChain:true});
const first=await prepare('first-fixture-complete-result');
await reject('old independent Claim cannot decode/bypass the mining unit gate',()=>send(oldInstruction(pid,[meta(a.global),meta(stage),meta(first.unit,true),meta(a.pool(0),true),meta(a.ata(mint,miner.publicKey),true),meta(a.payment(first.unit,hash(text('bypass'))),true),meta(relayer.publicKey,true,true),system(),meta(TOKEN_PROGRAM_ID)],5,new Writer().key(hash(text('bypass'))).u64(1n).result())));
const wrongBeneficiary=first.settleKeys.map((k,n)=>n===11?meta(rogue.publicKey):k);await reject('relayer cannot change the fixed beneficiary',()=>send(instruction(pid,wrongBeneficiary,15)));
const wrongPool=first.settleKeys.map((k,n)=>n===9?meta(a.pool(1),true):k);await reject('verification pool cannot pay discovery rewards',()=>send(instruction(pid,wrongPool,15)));
const before=await state(grant,decodeGrant),minerBefore=await c.getBalance(miner.publicKey);
await reject('runtime CU fault atomically rolls back all business state',()=>send([ComputeBudgetProgram.setComputeUnitLimit({units:10_000}),instruction(pid,first.settleKeys,15)],undefined,{skipPreflight:true}));
assert.equal((await state(grant,decodeGrant)).spent,before.spent);assert.equal(await c.getAccountInfo(a.result(first.unit)),null);assert.equal(await c.getBalance(miner.publicKey),minerBefore);assert.equal((await state(first.unit,decodeUnit)).paid,false);checks.push('no rent/partial result/payment survives a failed real transaction');
await first.settle();await first.settle();
let r=await state(a.result(first.unit),decodeResult),u=await state(first.unit,decodeUnit);assert.equal(r.rentLamports,rent);assert.equal(u.actualRent,rent);assert.equal((await getAccount(c,a.ata(mint,miner.publicKey))).amount,100_000_000n);assert.equal((await state(grant,decodeGrant)).successes,1n);checks.push('atomic exact-1024 result and correct ATA payment; retry consumes exactly once');
await burn(c,relayer,a.ata(mint,miner.publicKey),mint,miner,10n,[miner]);assert.equal((await getMint(c,mint)).supply,CAP-10n);
const second=await prepare('second-fixture-complete-result');await second.settle();await first.settle();assert.equal((await getAccount(c,a.ata(mint,miner.publicKey))).amount,200_000_000n-10n);assert.equal((await state(grant,decodeGrant)).successes,2n);checks.push('same finite grant automatically pays a second distinct unit after real burn; prior payment recovers');
const third=await prepare('valid-but-revoked-funding');
await send(instruction(pid,[meta(a.global),meta(grant,true),meta(miner.publicKey,false,true)],6),[relayer,miner]);
await reject('revocation stops an already accepted background settlement',third.settle);
await reject('project cannot withdraw the miner unused principal',()=>send(instruction(pid,[meta(a.global),meta(grant,true),meta(project.publicKey,true,true)],7,{amount:1n}),[project]));
const renew={...baseGrant,generation:2n,deposit:0n,expiresAt:BigInt(await now()+1800)};
await send(instruction(pid,grantKeys,5,renew),[relayer,miner]);await reject('renewal does not revive an accepted old generation intent',third.settle);
await send(instruction(pid,[meta(a.global),meta(a.config),meta(third.unit,true),meta(third.intentAddress),meta(grant),meta(miner.publicKey,false,true)],14,{generation:2n}),[relayer,miner]);
await third.settle();await third.settle();assert.equal((await state(grant,decodeGrant)).successes,3n);checks.push('original miner explicitly rebinds funding without changing original science/beneficiary/amount');
const fourth=await prepare('concurrent-final-credit',{generation:2n}),fifth=await prepare('concurrent-over-quota',{generation:2n});
const race=await Promise.allSettled([fourth.settle(),fifth.settle()]);assert.equal(race.filter(v=>v.status==='fulfilled').length,1);assert.equal((await state(grant,decodeGrant)).successes,4n);checks.push('two workers racing the last grant credit yield exactly one payment');
await reject('session cannot increase grant limits or withdraw principal',()=>send(instruction(pid,grantKeys.map((k,n)=>n===4?meta(session.publicKey,true,true):k),5,{...renew,generation:3n}),[relayer,session]));
await reject('stale generation cannot create another grant',()=>send(instruction(pid,grantKeys,4,baseGrant),[relayer,miner]));
const closeKeys=[meta(a.global),meta(a.config),meta(first.unit,true),meta(a.result(first.unit),true),meta(project.publicKey,false,true),meta(project.publicKey,true)];
for(const role of [miner,session,relayer])await reject('result close rejects '+(role===miner?'miner':role===session?'session':'relayer'),()=>send(instruction(pid,closeKeys.map((k,n)=>n===4?meta(role.publicKey,false,true):k),17),[role]));
await reject('project close requires finalized archive attestations',()=>send(instruction(pid,closeKeys,17),[project]));
for(const s of settlers.slice(0,2))await send(signedInstruction(pid,[meta(a.global),meta(a.config),meta(first.unit,true),instructionSysvar()],16,{},closureMessage(pid,g,first.unit,await state(first.unit,decodeUnit)),s));
const retainUntil=Number((await state(first.unit,decodeUnit)).paidAt)+60;
if(await now()<retainUntil)await reject('project close enforces actual retention',()=>send(instruction(pid,closeKeys,17),[project]));
while(await now()<retainUntil)await new Promise(done=>setTimeout(done,1000));
const projectBefore=await c.getBalance(project.publicKey),resultLamports=(await c.getAccountInfo(a.result(first.unit))).lamports;
const closeSignature=await send(instruction(pid,closeKeys,17),[project]);const closeTx=await c.getTransaction(closeSignature,{commitment:'confirmed'});
assert.equal(await c.getAccountInfo(a.result(first.unit)),null);assert.equal(await c.getBalance(project.publicKey),projectBefore+resultLamports-closeTx.meta.fee);assert.equal((await state(first.unit,decodeUnit)).closed,true);checks.push('real project signature closes only result to fixed project recipient; permanent paid unit survives');
const spent=(await state(grant,decodeGrant)).spent,tokenBalance=(await getAccount(c,a.ata(mint,miner.publicKey))).amount;await first.settle();assert.equal(await c.getAccountInfo(a.result(first.unit)),null);assert.equal((await state(grant,decodeGrant)).spent,spent);assert.equal((await getAccount(c,a.ata(mint,miner.publicKey))).amount,tokenBalance);checks.push('closed result replay cannot recreate a result, charge rent or pay twice');
const unused=(await state(grant,decodeGrant)).available;await send(instruction(pid,[meta(a.global),meta(grant,true),meta(miner.publicKey,true,true)],7,{amount:unused}),[relayer,miner]);assert.equal((await state(grant,decodeGrant)).available,0n);
await send(instruction(pid,[meta(a.global),meta(a.config),meta(grant,true),meta(miner.publicKey,true,true),meta(project.publicKey,true)],8),[relayer,miner]);assert.equal(await c.getAccountInfo(grant),null);assert.equal((await state(a.generation(miner.publicKey),decodeGeneration)).generation,2n);checks.push('only miner recovers unused principal; project management rent and generation tombstone stay separate');
await reject('closing and recreating a grant cannot reset wallet generation',()=>send(instruction(pid,grantKeys,4,{...baseGrant,generation:1n}),[relayer,miner]));
assert(maxPacket<=1232);assert(maxCU<1_400_000);checks.push('actual supported legacy packets and runtime CU stay within network limits');
await writeFile(reportRoot+'/acceptance.json',JSON.stringify({schema:'flybrainlabs-mining-v3-sbf-acceptance',environment:'isolated-real-sbf-fixture',genesisHash:genesis,programId:pid.toBase58(),mint:mint.toBase58(),roles:{project:project.publicKey.toBase58(),relayer:relayer.publicKey.toBase58(),miner:miner.publicKey.toBase58(),settlers:settlers.map(k=>k.publicKey.toBase58()),independentOperators:1},checks,checkCount:checks.length,maxPacketBytes:maxPacket,maxComputeUnits:maxCU,trace,firstUnit:first.unit.toBase58(),retainedResult:a.result(second.unit).toBase58(),retainedResultDecoded:jsonValue(await state(a.result(second.unit),decodeResult)),grantFinalSpent:spent.toString(),supplyAtoms:(await getMint(c,mint)).supply.toString(),completeModelAcceptance:false,productionFunds:false},null,2)+'\n');
console.log({passed:true,checks:checks.length,maxPacketBytes:maxPacket,maxComputeUnits:maxCU,scientificFixture:true});
