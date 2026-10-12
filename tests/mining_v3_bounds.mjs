// Extends the retained real SBF bank with independent quota/expiry/nonce cases.
import assert from 'node:assert/strict';
import {Connection,Keypair,PublicKey,Transaction,sendAndConfirmTransaction} from '@solana/web3.js';
import {readFile,writeFile} from 'node:fs/promises';
import {addresses,meta,instruction,system,decodeGrant,decodeGeneration,decodeUnit,hash,text,jsonValue} from '../tools/mining_v3_client.mjs';
import {decodeGlobal as globalDecoder} from '../tools/economy_v2_client.mjs';
const root='/workspace/.flypow-secrets/wallet-mining-v3-local',c=new Connection('http://127.0.0.1:8899','confirmed'),load=async name=>Keypair.fromSecretKey(Uint8Array.from(JSON.parse(await readFile(root+'/'+name+'.json','utf8'))));
const [project,relayer,miner,session,rogue]=await Promise.all(['project','relayer','miner','session','rogue'].map(load)),report=JSON.parse(await readFile('/workspace/.flypow-publish/wallet-mining-v3/local/acceptance.json','utf8')),pid=new PublicKey(report.programId),a=addresses(pid),g=globalDecoder((await c.getAccountInfo(a.global)).data),mint=new PublicKey(g.mint),stage=a.miningStage(hash(text('finite-local-sbf-stage'))),rent=BigInt(await c.getMinimumBalanceForRentExemption(1024)),checks=[];
const pending=[a.unit(hash(text('concurrent-final-credit'))),a.unit(hash(text('concurrent-over-quota')))];let unit;for(const p of pending)if(!decodeUnit((await c.getAccountInfo(p)).data).paid)unit=p;assert(unit);
const u=decodeUnit((await c.getAccountInfo(unit)).data),id=hash(text('bound-isolated-grant-'+Date.now())),grant=a.grant(miner.publicKey,id),keys=[meta(a.global),meta(a.config),meta(a.generation(miner.publicKey),true),meta(grant,true),meta(miner.publicKey,true,true),meta(relayer.publicKey,true,true),system()];
async function send(ix,signers=[relayer]){return sendAndConfirmTransaction(c,new Transaction().add(...(Array.isArray(ix)?ix:[ix])),signers,{commitment:'confirmed'});}
async function reject(name,fn){await assert.rejects(fn);checks.push(name);}
const grantState=async()=>decodeGrant((await c.getAccountInfo(grant)).data),now=()=>Math.floor(Date.now()/1000);
const generation=async()=>decodeGeneration((await c.getAccountInfo(a.generation(miner.publicKey))).data).generation+1n;
const initial={id,generation:await generation(),session:session.publicKey,deposit:rent*3n,maxSpend:rent*3n,maxResultRent:rent-1n,maxResults:3n,expiresAt:BigInt(now()+500)};
await send(instruction(pid,keys,4,initial),[relayer,miner]);
async function rebind(){const v=await grantState();return send(instruction(pid,[meta(a.global),meta(a.config),meta(unit,true),meta(new PublicKey(u.intent)),meta(grant),meta(miner.publicKey,false,true)],14,{generation:v.generation}),[relayer,miner]);}
const settleKeys=[meta(a.global),meta(a.config),meta(stage,true),meta(unit,true),meta(new PublicKey(u.intent)),meta(new PublicKey(u.staging)),meta(grant,true),meta(a.result(unit),true),meta(mint),meta(a.pool(0),true),meta(a.ata(mint,miner.publicKey),true),meta(miner.publicKey),meta(relayer.publicKey,true,true),system(),meta('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA'),meta('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL')],settle=()=>send(instruction(pid,settleKeys,15));
await reject('relayer cannot rebind the original miner right',()=>send(instruction(pid,[meta(a.global),meta(a.config),meta(unit,true),meta(new PublicKey(u.intent)),meta(grant),meta(relayer.publicKey,false,true)],14,{generation:initial.generation})));
await rebind();await reject('single-result limit rejects rent despite ample escrow and cumulative quota',settle);
async function renew(changes={}){const v=await grantState();return send(instruction(pid,keys,5,{generation:await generation(),session:session.publicKey,deposit:0n,maxSpend:rent*3n,maxResultRent:rent,maxResults:3n,expiresAt:BigInt(now()+500),...changes}),[relayer,miner]);}
await renew();await reject('renewed grant requires explicit rebind of the old contribution',settle);await rebind();
await send(instruction(pid,[meta(a.global),meta(grant,true),meta(miner.publicKey,true,true)],7,{amount:rent*2n+1n}),[relayer,miner]);
await reject('insufficient escrow principal rejects rent with other limits sufficient',settle);assert.equal((await grantState()).available,rent-1n);assert.equal(await c.getAccountInfo(a.result(unit)),null);checks.push('failed quota consumption preserves accepted permanent scientific/economic right');
await renew({deposit:rent*2n+1n});await rebind();await send(instruction(pid,[meta(a.global),meta(grant,true),meta(miner.publicKey,false,true)],6),[relayer,miner]);await reject('revocation rejects an already re-bound financial intent',settle);
await renew({expiresAt:BigInt(now()+3)});await rebind();await new Promise(done=>setTimeout(done,4100));await reject('actual on-chain clock rejects an expired grant',settle);
await renew();await rebind();await settle();await settle();assert.equal((await grantState()).successes,1n);assert.equal((await grantState()).spent,rent);checks.push('original accepted right settles once after explicit funding recovery');
await reject('project cannot withdraw a different wallet unused escrow',()=>send(instruction(pid,[meta(a.global),meta(grant,true),meta(project.publicKey,true,true)],7,{amount:1n}),[project]));
await reject('withdrawal cannot exceed remaining principal',async()=>send(instruction(pid,[meta(a.global),meta(grant,true),meta(miner.publicKey,true,true)],7,{amount:(await grantState()).available+1n}),[relayer,miner]));
await reject('cannot create a new result by using a different payout address',()=>send(instruction(pid,settleKeys.map((m,i)=>i===7?meta(a.result(a.unit(hash(text('fake-task')))),true):m),15)));
await writeFile('/workspace/.flypow-publish/wallet-mining-v3/local/bounds.json',JSON.stringify({schema:'flybrainlabs-mining-v3-boundary-sbf',programId:pid.toBase58(),checks,checkCount:checks.length,finalGrant:jsonValue(await grantState()),productionFunds:false},null,2)+'\n');console.log({passed:true,checks:checks.length,realSbf:true});
