// Real deployed SBF + System/SPL/Ed25519 programs. Synthetic fixtures are explicitly test-only.
import assert from 'node:assert/strict';
import { Keypair,PublicKey,SystemProgram,Transaction,sendAndConfirmTransaction,SYSVAR_CLOCK_PUBKEY } from '@solana/web3.js';
import { getMint,getAccount,getOrCreateAssociatedTokenAccount } from '@solana/spl-token';
import { readFile,writeFile } from 'node:fs/promises';
import { connection,loadKey,loadProgram,setup,send,meta,TOKEN,Writer,merkle,hash,text,encodeRules,encodeReveal,commit,reveal,vote,finalize,close,refund,claim,readReceipt,accountReader,decodeCampaign,decodePage } from '../tools/campaign_tools.mjs';
import { taskForIndex } from '../web/catalog-task.mjs';
let checks=0;const evidence=[];
async function rejected(label,fn){await assert.rejects(fn);checks++;evidence.push(label);}
async function eq1024(ctx,p,label){const a=await connection.getAccountInfo(p.sub);assert.equal(a.data.length,1024);await readReceipt(ctx,p.sub);checks++;evidence.push(label);}
async function chainTime(){const a=await connection.getAccountInfo(SYSVAR_CLOCK_PUBKEY);return new DataView(a.data.buffer,a.data.byteOffset,a.data.length).getBigInt64(32,true);}
async function waitAfter(ts){while(await chainTime()<=BigInt(ts))await new Promise(r=>setTimeout(r,200));}
const payer=await loadKey('deploy/localnet/payer.json'),program=await loadProgram(),verifiers=Array.from({length:3},()=>Keypair.generate()),miner=Keypair.generate();
const u=new URL(connection.rpcEndpoint);assert.ok(['127.0.0.1','localhost'].includes(u.hostname),'tests are loopback only');
await sendAndConfirmTransaction(connection,new Transaction().add(SystemProgram.transfer({fromPubkey:payer.publicKey,toPubkey:miner.publicKey,lamports:100_000_000})),[payer]);
async function fixture(ttl=30,deadline=600,extra={}){
  const c={campaign_id:Array.from(hash(text('SYNTHETIC-LOCALNET-TEST-ONLY'),crypto.getRandomValues(new Uint8Array(32)))),source_root:Array(32).fill(1),registry_root:Array(32).fill(2),graph_root:Array(32).fill(3),model_root:Array(32).fill(4),experiment_root:Array(32).fill(5),public_seed:Array(32).fill(6),model_neuron_count:4,simulation_steps:32,time_step_us:1000,targets:[{body_id:42,model_index:1}],stimuli:[{start_step:4,end_step:8,current:10},{start_step:4,end_step:8,current:20}],repeats:1};
  const tasks=await Promise.all([0,1].map(k=>taskForIndex({campaign:c},k))),tree=merkle(tasks);
  const r={catalog:c,manifest_root:tree.root,readout_root:hash(text('synthetic-readout')),genesis:new PublicKey(await connection.getGenesisHash()).toBytes(),verifiers:verifiers.map(v=>v.publicKey.toBytes()),developer:payer.publicKey.toBytes(),community:payer.publicKey.toBytes(),mint:new Uint8Array(32),roi_count:1,readout_schema_id:1,verifier_policy_id:1,simulation_config_id:1,observe_start:0,submit_deadline:Number(await chainTime())+deadline,reveal_ttl:ttl,verify_ttl:ttl,archive_ttl:ttl,min_reveal_slots:2,advance:5000,queue_fee:0,verify_fee:1000,rejection_slash:0,archive_retention_seconds:3600,miner_pool_whole:10,validator_pool_whole:3,community_pool_whole:2,network:0};
  Object.assign(r,extra);const ctx=await setup(r,tree);return {ctx,tasks,tree};
}
function zero(task){const root=hash(text('SYNTHETIC-ZERO-RESPONSE'),text(task.taskId));return {resultRoot:root,previewRoot:hash(text('SYNTHETIC-PREVIEW'),root),archiveRoot:hash(text('SYNTHETIC-MANIFEST'),root),global:Array(8).fill(0n),roi:Array(16).fill(0n),time:Array(32).fill(0),top:Array.from({length:8},()=>({bodyId:0n,delta:0,firstStep:0})),topCount:0};}
const {ctx,tasks,tree}=await fixture();const [t0,t1]=tasks,x0=zero(t0),x1=zero(t1);
await rejected('quote cap below dynamic rent rejects without creating a submission',()=>commit(ctx,t0,tree.proof(0),x0.resultRoot,miner,miner.publicKey,miner.publicKey,1n));
await rejected('wrong canonical body/index/proof rejects',()=>commit(ctx,{...t0,bodyId:43},tree.proof(0),x0.resultRoot,miner));
const candidates=[];for(let i=0;i<8;i++)candidates.push(await commit(ctx,t0,tree.proof(0),x0.resultRoot,miner));
for(const p of candidates)await eq1024(ctx,p,'commit account has exactly 1024 bytes');
await rejected('bounded queue rejects ninth live candidate',()=>commit(ctx,t0,tree.proof(0),x0.resultRoot,miner));
const p=candidates[1];
await rejected('wrong program-owned result PDA cannot be substituted',()=>send(program,p.keys.map((k,i)=>i===2?meta(ctx.a.epoch(42),true):k),6,p.body,[miner]));
await rejected('system-owned account cannot substitute for receipt on reveal',()=>send(program,[meta(ctx.a.campaign),meta(ctx.a.result(t0.taskId),true),meta(miner.publicKey,true),meta(miner.publicKey,false,true)],7,encodeReveal({...x0,salt:p.salt}),[miner]));
await rejected('copied reveal / wrong salt cannot reveal a different commitment',()=>reveal(ctx,p,{...x0,salt:candidates[0].salt,resultRoot:hash(text('wrong-result'))}));
await reveal(ctx,p,x0);await eq1024(ctx,p,'reveal account stays 1024');
await rejected('revealed scientific fields cannot be edited by a second reveal',()=>reveal(ctx,p,{...x0,previewRoot:hash(text('changed-preview'))}));
await vote(ctx,p,verifiers[0],0);await eq1024(ctx,p,'partial vote account stays 1024');
await rejected('same authorized key cannot vote twice',()=>vote(ctx,p,verifiers[0],0));
await rejected('signed cross-domain message cannot replay',()=>vote(ctx,p,verifiers[1],1,true,m=>{const copy=m.slice();copy[24]^=1;return copy;}));
await rejected('unauthorized verifier cannot substitute for policy key',()=>vote(ctx,p,Keypair.generate(),1));
await vote(ctx,p,verifiers[1],1);await eq1024(ctx,p,'timely quorum account stays 1024');
await rejected('older live commitment retains priority',()=>finalize(ctx,p));
const early=await readReceipt(ctx,candidates[0].sub);await waitAfter(early.lifecycle.revealDeadlineTs);
await close(ctx,candidates[0]);await refund(ctx,candidates[0]);
await rejected('loser refund is single-use',()=>refund(ctx,candidates[0]));
await finalize(ctx,p);await eq1024(ctx,p,'finalized zero-response receipt stays 1024 and earns its task quota');
await rejected('same scientific task cannot be claimed by another wallet/salt',()=>commit(ctx,t0,tree.proof(0),x0.resultRoot,miner));
await sendAndConfirmTransaction(connection,new Transaction().add(SystemProgram.transfer({fromPubkey:payer.publicKey,toPubkey:p.sub,lamports:10000})),[payer]);
assert.equal((await readReceipt(ctx,p.sub)).funding.refundableAdvanceRemainingLamports,4000n);checks++;evidence.push('third-party donation does not inflate refundable advance');
await close(ctx,p);assert.equal(await connection.getAccountInfo(p.sub),null);checks++;
await rejected('permanent result blocks reopening after whole-account close',()=>commit(ctx,t0,tree.proof(0),x0.resultRoot,miner));
await rejected('accepted principal cannot refund before campaign success/failure',()=>refund(ctx,p));
await rejected('developer surplus cannot withdraw before SUCCESS',()=>send(program,[meta(ctx.a.campaign),meta(ctx.a.settlement(42),true),meta(payer.publicKey,true)],15));
for(const q of candidates.slice(2)){await close(ctx,q);await refund(ctx,q);}
const q=await commit(ctx,t1,tree.proof(1),x1.resultRoot,miner);await reveal(ctx,q,x1);await vote(ctx,q,verifiers[0],0);await vote(ctx,q,verifiers[1],1);await finalize(ctx,q);await close(ctx,q);
await send(program,[meta(ctx.a.campaign,true),meta(ctx.a.epoch(42),true)],11);
await rejected('epoch seal cannot increment campaign completion twice',()=>send(program,[meta(ctx.a.campaign,true),meta(ctx.a.epoch(42),true)],11));
await send(program,[meta(ctx.a.campaign,true)],12);assert.equal(decodeCampaign((await connection.getAccountInfo(ctx.a.campaign)).data).state,2);checks++;
const tge=await send(program,[meta(ctx.a.campaign,true),meta(ctx.a.mint,true),meta(TOKEN),...[0,1,2].map(i=>meta(ctx.a.tokens(i),true))],16);evidence.push({tge});
const minted=await getMint(connection,ctx.a.mint);assert.equal(minted.supply,1500000000n);assert.equal(minted.mintAuthority,null);assert.equal(minted.freezeAuthority,null);checks++;
await rejected('TGE is single-use after atomic mint authority revocation',()=>send(program,[meta(ctx.a.campaign,true),meta(ctx.a.mint,true),meta(TOKEN),...[0,1,2].map(i=>meta(ctx.a.tokens(i),true))],16));
const ata=await getOrCreateAssociatedTokenAccount(connection,payer,ctx.a.mint,miner.publicKey),wrong=await getOrCreateAssociatedTokenAccount(connection,payer,ctx.a.mint,payer.publicKey);
await rejected('wrong beneficiary ATA rejects without consuming entitlement',()=>claim(ctx,p,0,0,miner.publicKey,wrong.address,500000000n));
await rejected('wrong mint or data owner cannot consume entitlement',()=>send(program,[meta(ctx.a.campaign),meta(ctx.a.entitlement(p.sub),true),meta(ctx.a.epoch(42)),meta(ctx.a.tokens(0),true),meta(ata.address,true),meta(miner.publicKey),meta(TOKEN)],17,new Writer().u8(0).u8(0).u64(1n).result()));
const beforeRollback=(await connection.getAccountInfo(ctx.a.entitlement(p.sub))).data;
await rejected('failed SPL transfer CPI rolls back without consuming quota',()=>send(program,[meta(ctx.a.campaign),meta(ctx.a.entitlement(p.sub),true),meta(ctx.a.mint),meta(ctx.a.tokens(0),true),meta(ata.address),meta(miner.publicKey),meta(TOKEN)],17,new Writer().u8(0).u8(0).u64(1n).result()));
assert.deepEqual((await connection.getAccountInfo(ctx.a.entitlement(p.sub))).data,beforeRollback);checks++;
await claim(ctx,p,0,0,miner.publicKey,ata.address,250000000n);await claim(ctx,p,0,0,miner.publicKey,ata.address,250000000n);await claim(ctx,q,0,0,miner.publicKey,ata.address,500000000n);
assert.equal((await getAccount(connection,ata.address)).amount,1000000000n);checks++;
await rejected('miner cannot claim beyond remaining quota',()=>claim(ctx,p,0,0,miner.publicKey,ata.address,1n));
await claim(ctx,p,2,0,payer.publicKey,wrong.address,200000000n);
await rejected('community vault cannot pay its budget twice',()=>claim(ctx,p,2,0,payer.publicKey,wrong.address,1n));
for(const v of [p,q]){await refund(ctx,v);await send(program,[meta(ctx.a.campaign),meta(ctx.a.entitlement(v.sub),true),meta(ctx.a.settlement(42),true),meta(payer.publicKey)],14);}
for(const [task,contribution]of [[t0,p],[t1,q]])for(let i=0;i<2;i++){const vata=await getOrCreateAssociatedTokenAccount(connection,payer,ctx.a.mint,verifiers[i].publicKey),pool=task.k===0?1n:2n,share=i===0?pool/2n:pool-pool/2n;if(share)await claim(ctx,contribution,1,i,verifiers[i].publicKey,vata.address,share*100000000n);}
assert.equal((await getAccount(connection,ctx.a.tokens(0))).amount,0n);assert.equal((await getAccount(connection,ctx.a.tokens(1))).amount,0n);assert.equal((await getAccount(connection,ctx.a.tokens(2))).amount,0n);checks++;evidence.push('all three test budgets conserve supply through fixed recipient claims');
const vaultRaw=(await connection.getAccountInfo(ctx.a.settlement(42))).data,v=accountReader(vaultRaw,'SettlementVault');v.key();v.u64();assert.equal(v.u64(),0n);checks++;
await send(program,[meta(ctx.a.campaign),meta(ctx.a.settlement(42),true),meta(payer.publicKey,true)],15);
console.log('PASS: task uniqueness, exact receipts, competition, SUCCESS, TGE, claims and conservation');
const failed=await fixture(1,12),fctx=failed.ctx,fp=await commit(fctx,failed.tasks[0],failed.tree.proof(0),zero(failed.tasks[0]).resultRoot,miner);
const final=fctx.r.submit_deadline+fctx.r.reveal_ttl+fctx.r.verify_ttl+fctx.r.archive_ttl;await waitAfter(final);await send(program,[meta(fctx.a.campaign,true)],12);
assert.equal(decodeCampaign((await connection.getAccountInfo(fctx.a.campaign)).data).state,3);checks++;
await rejected('FAILED campaign cannot mint',()=>send(program,[meta(fctx.a.campaign,true),meta(fctx.a.mint,true),meta(TOKEN),...[0,1,2].map(i=>meta(fctx.a.tokens(i),true))],16));
await close(fctx,fp);const before=await connection.getBalance(miner.publicKey);await refund(fctx,fp);const after=await connection.getBalance(miner.publicKey);assert.equal(BigInt(after-before),BigInt(await connection.getMinimumBalanceForRentExemption(1024))+5000n);checks++;
await rejected('FAILED permissionless refund cannot repeat',()=>refund(fctx,fp));
console.log('PASS: FAILED has no mint and refunds are permissionless and single-use');
// Allow confirmed commit + the real two-slot wait + reveal before testing the
// short verification expiry. A two-second reveal TTL races transaction inclusion.
const timeout=await fixture(10,300,{verify_ttl:3}),tp=await commit(timeout.ctx,timeout.tasks[0],timeout.tree.proof(0),zero(timeout.tasks[0]).resultRoot,miner);
await reveal(timeout.ctx,tp,zero(timeout.tasks[0]));await vote(timeout.ctx,tp,verifiers[0],0);const half=await readReceipt(timeout.ctx,tp.sub);await waitAfter(half.lifecycle.verifyDeadlineTs);
await rejected('expired partial vote cannot revive a quorum',()=>vote(timeout.ctx,tp,verifiers[1],1));await rejected('expired partial vote cannot finalize',()=>finalize(timeout.ctx,tp));await eq1024(timeout.ctx,tp,'timeout still holds exact 1024 bytes before whole close');await close(timeout.ctx,tp);await refund(timeout.ctx,tp);
console.log('PASS: expired partial votes never revive a quorum and retain refunds');
const reject=await fixture(30,300,{rejection_slash:500}),rp=await commit(reject.ctx,reject.tasks[0],reject.tree.proof(0),zero(reject.tasks[0]).resultRoot,miner);
await reveal(reject.ctx,rp,zero(reject.tasks[0]));await vote(reject.ctx,rp,verifiers[0],0,false);await vote(reject.ctx,rp,verifiers[1],1,false);const rejectedReceipt=await readReceipt(reject.ctx,rp.sub);assert.equal(rejectedReceipt.header.status,3);assert.equal(rejectedReceipt.funding.refundableAdvanceRemainingLamports,3500n);assert.equal(rejectedReceipt.funding.serviceFeePaidLamports,1000n);assert.equal(rejectedReceipt.funding.slashPaidLamports,500n);await eq1024(reject.ctx,rp,'rejection/slash accounting retains 1024 bytes');await close(reject.ctx,rp);await refund(reject.ctx,rp);checks++;evidence.push('authorized rejection consumes only declared advance and slash atomically, never principal');
const alternateRefund=Keypair.generate().publicKey,beneficiary=Keypair.generate().publicKey,distinct=await commit(reject.ctx,reject.tasks[0],reject.tree.proof(0),zero(reject.tasks[0]).resultRoot,miner,beneficiary,alternateRefund,null,payer);
const identities=await readReceipt(reject.ctx,distinct.sub);assert.equal(identities.identity.payer,Buffer.from(payer.publicKey.toBytes()).toString('hex'));assert.equal(identities.identity.rewardBeneficiary,Buffer.from(beneficiary.toBytes()).toString('hex'));assert.equal(identities.identity.refundRecipient,Buffer.from(alternateRefund.toBytes()).toString('hex'));await waitAfter(identities.lifecycle.revealDeadlineTs);await close(reject.ctx,distinct);
await rejected('refund cannot redirect to the payer instead of fixed recipient',()=>refund(reject.ctx,distinct));
await send(program,[meta(reject.ctx.a.campaign),meta(reject.ctx.a.entitlement(distinct.sub),true),meta(reject.ctx.a.settlement(42),true),meta(alternateRefund,true)],13);
assert.equal(BigInt(await connection.getBalance(alternateRefund)),BigInt(await connection.getMinimumBalanceForRentExemption(1024))+5000n);checks++;evidence.push('payer, miner, beneficiary and refund recipient remain distinct and immutable');
const report={schema:'flypow-localnet-integration-v1',synthetic_test_only:true,program:program.toBase58(),checks,evidence};await writeFile('data/prepared/localnet-integration-report.json',JSON.stringify(report,null,2)+'\n');console.log({checks,report:'data/prepared/localnet-integration-report.json'});
