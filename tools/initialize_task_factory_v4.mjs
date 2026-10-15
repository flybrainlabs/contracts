#!/usr/bin/env node
// Operator-only upload of an approved catalog. Tasks stay paused until the
// separately requested activation transaction; nodes never run this tool.
import {readFile,writeFile,mkdir,open,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {Keypair,PublicKey} from '@solana/web3.js';
import {compileCatalog} from './compile_task_catalog_v4.mjs';
import {DevnetJournal} from './devnet_journal.mjs';
import {addresses,instruction,meta,system,decodeFactory,decodeTask,hex,fromHex,encodeDescriptor,STATE,NONE} from './task_factory_v4_client.mjs';
import {decodeGlobal} from './economy_v2_client.mjs';

const [planPath,compiledDirectory,authorityKeyPath,privateDirectory,reportPath,mode]=process.argv.slice(2);
if(!reportPath||mode&&!['--apply','--activate'].includes(mode))throw new Error('Usage: initialize_task_factory_v4.mjs APPROVED_PLAN COMPILED_DIRECTORY AUTHORITY_KEY PRIVATE_DIRECTORY PUBLIC_REPORT [--apply|--activate]');
async function main(){
  const plan=JSON.parse(await readFile(planPath,'utf8'));if(plan.cluster!=='devnet'||plan.genesisHash!=='EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG')throw new Error('Only the approved Devnet catalog is supported');
  const base=resolve(planPath,'..'),compiled=await compileCatalog(plan,async p=>JSON.parse(await readFile(resolve(base,p),'utf8'))),artifact=JSON.parse(await readFile(join(compiledDirectory,'catalog.json'),'utf8'));
  if(artifact.catalogRoot!==compiled.root||artifact.budgetAtoms!==compiled.budget||artifact.count!==compiled.descriptors.length||artifact.factoryId!==plan.factoryId||JSON.stringify(artifact.instructions)!==JSON.stringify(compiled.instructions.map(hex)))throw new Error('Reviewed compiled catalog changed');
  const payer=Keypair.fromSecretKey(Uint8Array.from(JSON.parse(await readFile(authorityKeyPath,'utf8')))),program=new PublicKey(plan.programId),a=addresses(program),factory=a.factory(fromHex(plan.factoryId)),dir=resolve(privateDirectory);
  if(dir.startsWith(resolve(process.cwd())+'/'))throw new Error('Private journal must be outside the checkout');await mkdir(dir,{recursive:true,mode:0o700});
  const journal=await new DevnetJournal(join(dir,'catalog-journal.json'),payer,{maxTransactions:48,maxFees:500000}).initialize({scope:'approved-preset-task-factory-v4',programId:plan.programId,factory:factory.toBase58(),catalogRoot:compiled.root,budgetAtoms:compiled.budget}),c=journal.connection;
  const info=address=>c.getAccountInfo(address,'finalized'),readFactory=async()=>{const value=await info(factory);if(!value)return null;if(!value.owner.equals(program))throw new Error('Factory owner mismatch');const f=decodeFactory(value.data);if(hex(f.id)!==plan.factoryId||hex(f.catalogRoot)!==compiled.root||f.count!==compiled.descriptors.length||f.budget!==BigInt(compiled.budget)||new PublicKey(f.authority).toBase58()!==payer.publicKey.toBase58()||hex(f.policy)!==plan.policyHash||new PublicKey(f.mint).toBase58()!==plan.mint)throw new Error('Frozen factory scope mismatch');return f;};
  const global=decodeGlobal((await info(a.global)).data);if(new PublicKey(global.network).toBase58()!==plan.genesisHash||new PublicKey(global.authority).toBase58()!==payer.publicKey.toBase58()||!global.issued||hex(global.policy)!==plan.policyHash||new PublicKey(global.mint).toBase58()!==plan.mint)throw new Error('Existing issuance/authority/policy mismatch');
  let f=await readFactory();for(let index=f?.uploaded||0;index<compiled.descriptors.length;index++)if(await info(a.science(compiled.descriptors[index].scientificIdentity)))throw new Error('Scientific identity already registered outside the uploaded prefix');
  const rents=await Promise.all([768,2048,128].map(size=>c.getMinimumBalanceForRentExemption(size,'finalized'))),rentQuote=(f?0:rents[0])+(compiled.descriptors.length-(f?.uploaded||0))*(rents[1]+rents[2]);if(rentQuote>250000000||await c.getBalance(payer.publicKey,'finalized')<rentQuote+500000)throw new Error('Finite Devnet account rent or fee allowance exceeded');
  const report={schema:'flybrainlabs-task-factory-initialization-v4',programId:plan.programId,factory:factory.toBase58(),factoryId:plan.factoryId,catalogRoot:compiled.root,count:compiled.descriptors.length,budgetAtoms:compiled.budget,rewardAtoms:compiled.descriptors.map(x=>String(x.rewardAtoms)),approvalEvidenceHash:plan.approval.evidenceHash,accountRentQuoteLamports:String(rentQuote),accountRentCeilingLamports:'250000000',mode:mode||'preflight',mainnet:false,formalIssuance:false};
  if(!mode){console.log(JSON.stringify({...report,preflightPassed:true,chainTransactions:0}));return;}
  const lockPath=join(dir,'catalog.lock'),lock=await open(lockPath,'wx',0o600);
  try{
    if(mode==='--apply'){
      if(!f){if(global.available[0]<BigInt(compiled.budget))throw new Error('Available existing miner pool is insufficient');
        await journal.run('initialize-and-pause',[instruction(program,[meta(a.global,true),meta(factory,true),meta(payer.publicKey,true,true),system()],0,{id:fromHex(plan.factoryId),catalogRoot:fromHex(compiled.root),count:compiled.descriptors.length,budget:BigInt(compiled.budget),deadline:BigInt(plan.deadline)}),instruction(program,[meta(factory,true),meta(payer.publicKey,false,true)],10,{paused:true})],[],async()=>!!await readFactory());
      }
      f=await readFactory();if(!f.paused)throw new Error('Upload requires the factory to remain paused');
      for(let index=0;index<compiled.descriptors.length;index++){
        const descriptor=compiled.descriptors[index],task=a.task(factory,index);f=await readFactory();
        if(f.uploaded<=index)await journal.run('append-'+index,instruction(program,[meta(factory,true),meta(task,true),meta(a.science(descriptor.scientificIdentity),true),meta(payer.publicKey,true,true),system()],1,{descriptor}),[],async()=>!!await info(task));
        const value=await info(task);if(!value?.owner.equals(program)||hex(encodeDescriptor(decodeTask(value.data).descriptor).result())!==hex(encodeDescriptor(descriptor).result()))throw new Error('Uploaded descriptor differs from the approved catalog');
      }
      await journal.run('seal',instruction(program,[meta(factory,true),meta(payer.publicKey,false,true)],2),[],async()=>(await readFactory()).state!==STATE.DRAFT);f=await readFactory();
      if(!f.paused||f.activeIndex!==NONE||f.uploaded!==f.count||f.rewardSum!==BigInt(compiled.rewardSum)||hex(f.uploadedRoot)!==compiled.root)throw new Error('Sealed paused catalog verification failed');
    }else{
      if(!f||f.uploaded!==f.count||hex(f.uploadedRoot)!==compiled.root||f.state===STATE.DRAFT)throw new Error('Complete sealed catalog required before activation');
      if(f.activeIndex===NONE&&f.nextIndex===0)await journal.run('unpause-and-open-first',[instruction(program,[meta(factory,true),meta(payer.publicKey,false,true)],10,{paused:false}),instruction(program,[meta(factory,true),meta(a.task(factory,0),true)],3)],[],async()=>{const v=await readFactory();return !v.paused&&v.nextIndex>0;});f=await readFactory();
      if(f.paused||f.nextIndex<1)throw new Error('Activation did not finalize');
    }
    const output={...report,status:mode==='--apply'?'sealed-paused-awaiting-service-switch':'active-contract-task-factory',paused:f.paused,uploaded:f.uploaded,activeIndex:f.activeIndex,nextIndex:f.nextIndex,remainingAtoms:String(f.remaining),reservedAtoms:String(f.reserved),paidAtoms:String(f.paid),receipts:journal.publicReceipts(),commitment:'finalized'};
    await writeFile(reportPath,JSON.stringify(output,null,2)+'\n');console.log(JSON.stringify(output));
  }finally{await lock.close();await unlink(lockPath);}
}
main().catch(error=>{console.error('Approved factory initialization stopped: '+error.message);process.exitCode=1;});
