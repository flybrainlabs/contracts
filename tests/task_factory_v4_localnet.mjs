// Real SBF lifecycle acceptance. Scientific roots/votes are explicit fixtures;
// these tests neither assert biological accuracy nor authorize a public budget.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {Connection,Keypair,PublicKey,Transaction,sendAndConfirmTransaction,Ed25519Program,ComputeBudgetProgram} from '@solana/web3.js';
import {createAccount,getAccount} from '@solana/spl-token';
import {addresses,meta,instruction,system,instructionSysvar,TOKEN_PROGRAM_ID,decodeFactory,decodeTask,decodeSubmission,descriptorHash,catalogRoot,commitment,voteMessage,hash,text,hex,Writer} from '../tools/task_factory_v4_client.mjs';
import {instruction as economicInstruction,decodeGlobal} from '../tools/economy_v2_client.mjs';
const rpc=process.env.FACTORY_V4_RPC||'http://127.0.0.1:8999';
if(!/^http:\/\/(127\.0\.0\.1|localhost):\d+$/.test(rpc))throw new Error('Isolated local validator only');
const root=process.env.FACTORY_V4_ARTIFACTS||'/workspace/.flypow-publish/task-factory-v4-20261015',keyDir=process.env.FACTORY_V4_KEYS||'/workspace/.flypow-secrets/task-factory-v4-local';
const key=async name=>Keypair.fromSecretKey(Uint8Array.from(JSON.parse(await readFile(keyDir+'/'+name+'.json','utf8'))));
const project=await key('project'),program=new PublicKey(JSON.parse(await readFile(root+'/program.json','utf8')).programId),c=new Connection(rpc,'confirmed'),a=addresses(program),genesis=await c.getGenesisHash(),verifiers=Array.from({length:3},()=>Keypair.generate()),miner=Keypair.generate(),rogue=Keypair.generate(),policy=hash(text('task-factory-v4-local-fixture'));
const checks=[],trace=[];let maxPacket=0,maxCU=0;
async function send(ixs,signers=[miner]){
  const tx=new Transaction().add(...(Array.isArray(ixs)?ixs:[ixs]));tx.feePayer=signers[0].publicKey;tx.recentBlockhash=(await c.getLatestBlockhash()).blockhash;tx.sign(...signers);maxPacket=Math.max(maxPacket,tx.serialize().length);
  const signature=await sendAndConfirmTransaction(c,tx,signers,{commitment:'confirmed'}),actual=await c.getTransaction(signature,{commitment:'confirmed',maxSupportedTransactionVersion:0});
  maxCU=Math.max(maxCU,actual?.meta?.computeUnitsConsumed||0);trace.push({signature,bytes:tx.serialize().length,cu:actual?.meta?.computeUnitsConsumed});return signature;
}
async function reject(label,fn){await assert.rejects(fn);checks.push(label);}
async function state(address,decode){const raw=await c.getAccountInfo(address);assert(raw?.owner.equals(program));return decode(raw.data);}
const time=async()=>Number(await c.getBlockTime(await c.getSlot())||Math.floor(Date.now()/1000));
for(const who of [miner,rogue])await c.confirmTransaction(await c.requestAirdrop(who.publicKey,2e9),'confirmed');
const pd=PublicKey.findProgramAddressSync([program.toBytes()],new PublicKey('BPFLoaderUpgradeab1e11111111111111111111111'))[0];
await send(economicInstruction(program,[meta(a.global,true),meta(a.mint,true),...[0,1,2].map(n=>meta(a.pool(n),true)),meta(project.publicKey,true,true),system(),meta(TOKEN_PROGRAM_ID),meta(pd)],0,new Writer().key(new PublicKey(genesis)).key(policy).bytes(verifiers.flatMap(k=>Array.from(k.publicKey.toBytes()))).result()),[project]);
await send(economicInstruction(program,[meta(a.global,true),meta(a.mint,true),...[0,1,2].map(n=>meta(a.pool(n),true)),meta(TOKEN_PROGRAM_ID),meta(project.publicKey,false,true)],1),[project]);
const originalAvailable=(await state(a.global,decodeGlobal)).available[0];
const desc=(label,short=false)=>({scientificIdentity:hash(text(label+'science')),goal:hash(text(label+'goal')),content:hash(text(label+'content')),model:hash(text('model')),graph:hash(text('graph')),parameters:hash(text('params')),calibration:hash(text('calibration')),inputDomain:hash(text('domain')),rewardAtoms:100n,claimSeconds:short?1:120,revealSeconds:short?2:180,verifySeconds:short?3:240,maxAttempts:2,seeds:Array.from({length:16},(_,n)=>BigInt(n+1))});
const ds=[desc('task-a'),desc('task-b'),desc('task-c',true)],id=hash(text('local-v4-campaign')),factory=a.factory(id),fk=[meta(factory,true)],task=n=>a.task(factory,n);
await send(instruction(program,[meta(a.global,true),meta(factory,true),meta(project.publicKey,true,true),system()],0,{id,catalogRoot:catalogRoot(ds),count:3,budget:350n,deadline:BigInt(await time()+1200)}),[project]);
await reject('incomplete catalog cannot seal',()=>send(instruction(program,[...fk,meta(project.publicKey,false,true)],2),[project]));
await reject('unauthorized caller cannot upload a task',()=>send(instruction(program,[...fk,meta(task(0),true),meta(a.science(ds[0].scientificIdentity),true),meta(miner.publicKey,true,true),system()],1,{descriptor:ds[0]})));
for(let n=0;n<ds.length;n++)await send(instruction(program,[...fk,meta(task(n),true),meta(a.science(ds[n].scientificIdentity),true),meta(project.publicKey,true,true),system()],1,{descriptor:ds[n]}),[project]);
await send(instruction(program,[...fk,meta(project.publicKey,false,true)],2),[project]);
await reject('catalog is immutable after sealing',()=>send(instruction(program,[...fk,meta(task(3),true),meta(a.science(hash(text('extra'))),true),meta(project.publicKey,true,true),system()],1,{descriptor:desc('extra')}),[project]));
await send(instruction(program,[...fk,meta(task(0),true)],3));await send(instruction(program,[...fk,meta(task(0),true)],3));checks.push('ordinary miner starts/retries task activation without an authority or relayer');
const f=await state(factory,decodeFactory);
async function prepare(n,who=miner,label='candidate'){
  const t=await state(task(n),decodeTask),evidence={candidate:hash(text(label+'candidate')),result:hash(text(label+'result')),manifest:hash(text(label+'manifest')),archive:hash(text(label+'archive'))},salt=hash(text(label+'salt')),commit=commitment(program,f,task(n),t.attempt,who.publicKey,who.publicKey,evidence,salt),sub=a.submission(task(n),t.attempt,who.publicKey,commit);
  const commitKeys=[meta(factory),meta(task(n),true),meta(sub,true),meta(who.publicKey,false,true),meta(who.publicKey,true,true),system()];
  await send(instruction(program,commitKeys,4,{commitment:commit,beneficiary:who.publicKey}),[who]);await send(instruction(program,commitKeys,4,{commitment:commit,beneficiary:who.publicKey}),[who]);
  const s=await state(sub,decodeSubmission);while(BigInt(await c.getSlot())<=s.commitSlot)await new Promise(r=>setTimeout(r,100));
  const reveal=(e=evidence)=>send(instruction(program,[meta(factory),meta(task(n),true),meta(sub,true),meta(who.publicKey,false,true)],5,{evidence:e,salt}),[who]);
  await reject('changed reveal cannot reuse a genuine commitment',()=>reveal({...evidence,result:hash(text('tampered'))}));await reveal();await reveal();
  return {n,who,t,evidence,salt,commit,sub};
}
async function vote(x,verifier,valid=true,{next=true,report=hash(text('report-'+verifier.publicKey.toBase58()+'-'+valid)),message=null}={}){
  const t=await state(task(x.n),decodeTask),s=await state(x.sub,decodeSubmission),f=await state(factory,decodeFactory),msg=message||voteMessage(program,f,t,x.sub,s,valid,report,t.descriptor.seeds.length),keys=[...fk,meta(task(x.n),true),meta(x.sub,true),instructionSysvar()];
  if(next&&f.nextIndex<f.count)keys.push(meta(task(f.nextIndex),true));
  return send([Ed25519Program.createInstructionWithPrivateKey({privateKey:verifier.secretKey,message:msg}),instruction(program,keys,6,{valid,report,completeTrials:t.descriptor.seeds.length,ed25519Index:0})]);
}
const earlier=await prepare(0,rogue,'earlier-invalid'),later=await prepare(0,miner,'later-valid');
await reject('nonadmitted verifier cannot vote',()=>vote(later,rogue));
await reject('candidate cannot finalize before quorum',()=>send(instruction(program,[...fk,meta(task(0),true),meta(later.sub)],7)));
await vote(later,verifiers[0]);await vote(later,verifiers[0]);assert.equal((await state(later.sub,decodeSubmission)).validVotes,1);
await reject('verifier cannot equivocate',()=>vote(later,verifiers[0],false));
await vote(later,verifiers[1]);assert.equal((await state(task(0),decodeTask)).state,1);
await reject('later valid result cannot bypass earlier live commitment',()=>send(instruction(program,[...fk,meta(task(0),true),meta(later.sub),meta(task(1),true)],7)));
await vote(earlier,verifiers[0],false);await vote(earlier,verifiers[1],false);
await send(instruction(program,[...fk,meta(task(0),true),meta(later.sub),meta(task(1),true)],7));
assert.equal((await state(task(0),decodeTask)).state,2);assert.equal((await state(task(1),decodeTask)).state,1);assert.equal((await state(factory,decodeFactory)).acceptedUnpaid,100n);
checks.push('ordered threshold acceptance atomically opens the next predefined task without a speed gate');
const dest=await createAccount(c,miner,a.mint,miner.publicKey),wrong=await createAccount(c,rogue,a.mint,rogue.publicKey);
const payKeys=[meta(a.global),...fk,meta(task(0),true),meta(later.sub),meta(a.pool(0),true),meta(dest,true),meta(TOKEN_PROGRAM_ID)];
await reject('public payout cannot redirect the fixed beneficiary',()=>send(instruction(program,payKeys.map((k,n)=>n===5?meta(wrong,true):k),9)));
await reject('failed token settlement leaves accepted entitlement retryable',()=>send([ComputeBudgetProgram.setComputeUnitLimit({units:100}),instruction(program,payKeys,9)]));
assert.equal((await state(factory,decodeFactory)).acceptedUnpaid,100n);assert.equal((await state(task(0),decodeTask)).paid,false);
await send(instruction(program,payKeys,9));await send(instruction(program,payKeys,9));assert.equal((await getAccount(c,dest)).amount,100n);checks.push('public payment retries exactly once and never blocks task progression');
const second=await prepare(1,miner,'second-valid');
await send(instruction(program,[...fk,meta(project.publicKey,false,true)],10,{paused:true}),[project]);
await vote(second,verifiers[0]);await vote(second,verifiers[1]);assert.equal((await state(task(1),decodeTask)).state,2);assert.equal((await state(factory,decodeFactory)).activeIndex,0xffffffff);assert.equal((await state(task(2),decodeTask)).state,0);
await reject('pause stops new task activation',()=>send(instruction(program,[...fk,meta(task(2),true)],3)));
await send(instruction(program,[...fk,meta(project.publicKey,false,true)],10,{paused:false}),[project]);
await send(instruction(program,[...fk,meta(task(2),true)],3));checks.push('pause preserves verified entitlements and permissionless resume');
await reject('active task cannot expire early',()=>send(instruction(program,[...fk,meta(task(2),true)],8)));
const waitDeadline=async()=>{const t=await state(task(2),decodeTask);while(await time()<=Number(t.verifyDeadline))await new Promise(r=>setTimeout(r,300));};
await waitDeadline();await send(instruction(program,[...fk,meta(task(2),true)],8));assert.equal((await state(task(2),decodeTask)).attempt,1);
await waitDeadline();await send(instruction(program,[...fk,meta(task(2),true)],8));const failed=await state(task(2),decodeTask);assert.equal(failed.state,3);assert(failed.winner.every(b=>b===0));assert.equal((await state(factory,decodeFactory)).state,2);checks.push('bounded retries retain the same scientific task and record failure without a reward');
await send(instruction(program,[meta(a.global,true),...fk,meta(project.publicKey,false,true)],11),[project]);
let closed=await state(factory,decodeFactory);assert.equal(closed.budget,350n);assert.equal(closed.refunded,150n);assert.equal(closed.acceptedUnpaid,100n);assert.equal((await state(a.global,decodeGlobal)).available[0],originalAvailable-200n);
await send(instruction(program,[meta(a.global),...fk,meta(task(1),true),meta(second.sub),meta(a.pool(0),true),meta(dest,true),meta(TOKEN_PROGRAM_ID)],9));closed=await state(factory,decodeFactory);assert.equal(closed.paid,200n);assert.equal(closed.acceptedUnpaid,0n);assert.equal((await getAccount(c,dest)).amount,200n);checks.push('closing returns only unused budget; accepted rewards remain payable');
assert(maxPacket<=1232);assert(maxCU<=1_400_000);
await writeFile(root+'/localnet-acceptance.json',JSON.stringify({schema:'flybrainlabs-task-factory-v4-sbf-acceptance',passed:true,environment:'isolated-localnet-scientific-fixtures',programId:program.toBase58(),genesisHash:genesis,checks,checkCount:checks.length,maxPacket,maxCU,transactions:trace.length,trace,publicChainTransactions:0,productionFunds:false,completeModelScientificAcceptance:false,independentOperators:false},null,2)+'\n');
console.log(JSON.stringify({passed:true,checks:checks.length,transactions:trace.length,maxPacket,maxCU,publicChainTransactions:0}));
