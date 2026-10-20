// Real SBF, canonical Ed25519 votes and SPL token payments in an isolated VM.
// The science fixture is a real six-cell simulation; no public chain is used.
import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir,open,stat} from 'node:fs/promises';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash,randomUUID} from 'node:crypto';
import {LiteSVM,Clock,FailedTransactionMetadata} from 'litesvm';
import {getTransactionDecoder} from '@solana/kit';
import {Keypair,PublicKey,Transaction,Ed25519Program,SystemProgram} from '@solana/web3.js';
import {getAssociatedTokenAddressSync,createAssociatedTokenAccountIdempotentInstruction,TOKEN_PROGRAM_ID,unpackAccount} from '@solana/spl-token';
import {ScientificReaderV5,ScientificClientV5,addresses,instruction,meta,system,slotHashes,instructionSysvar} from '../web/scientific-client-v5.mjs';
import {hex,hash,text,Writer,contentRoot,inputIdentity,inputCommitment,backgroundSeeds,decodeRound,decodeClaim,computationIdentity,uploadMessage} from '../web/scientific-round-v5.mjs';
import {computationContext} from '../web/scientific-round-v5.mjs';
import {canonical} from '../web/target-protocol-v2.mjs';
import {decodeGlobal} from '../tools/economy_v2_client.mjs';
import {ExactTrialWorker} from '../tools/exact_trial_worker.mjs';
import {ScientificContentStore,ScientificServiceV5} from '../tools/scientific_service_v5.mjs';
import {FactoryPoolQueueV4} from '../tools/factory_pool_queue_v4.mjs';
import {FactoryContentStoreV4} from '../web/factory-content-store-v4.mjs';
import {MiningWalletV4,newMiningMnemonic} from '../web/mining-wallet-v4.mjs';
import {ScientificWalletPolicyV5} from '../web/scientific-wallet-policy-v5.mjs';
import pg from 'pg';
import {verifyScientificClaim,scientificVoteInstructions} from '../tools/scientific_verifier_v5.mjs';
import {scientificFixture} from '../tools/scientific_fixture_v5.mjs';
if(process.env.SCIENCE_V5_TRACE_FETCH==='1'){
  const transport=globalThis.fetch;
  globalThis.fetch=async(...args)=>{try{return await transport(...args);}catch(error){const url=new URL(String(args[0]));throw new Error('Isolated HTTP failure at '+url.origin+url.pathname+' ('+(error.cause?.code||error.cause?.message||'unavailable')+')',{cause:error});}};
}
const ServiceRuntime=process.env.SCIENCE_V5_NODE_BUNDLE?(await import(pathToFileURL(process.env.SCIENCE_V5_NODE_BUNDLE).href)).ScientificServiceV5:ScientificServiceV5;
const artifacts=process.env.SCIENCE_V5_ARTIFACTS||'/workspace/.private/scientific-mining-20261020/sbf-acceptance',output=join(artifacts,randomUUID());await mkdir(output,{recursive:true});
const binary=process.env.SCIENCE_V5_SBF||'/workspace/.private/scientific-mining-20261020/sbf/flypow_economy_v2_program.so',native=process.env.SCIENCE_V5_NATIVE||'target/release/exact_trial',vector=JSON.parse(await readFile('tests/vectors/factory-user-rpc-v4.json','utf8')),svm=new LiteSVM(),project=Keypair.generate(),verifiers=Array.from({length:3},()=>Keypair.generate()),miner=Keypair.generate(),rogue=Keypair.generate();
svm.addProgramFromFile(vector.deployment.programId,binary);
for(const [address,v]of Object.entries(vector.accounts))svm.setAccount({address,lamports:BigInt(v.lamports),data:new Uint8Array(Buffer.from(v.data[0],'base64')),programAddress:v.owner,executable:!!v.executable});
svm.setClock(new Clock(BigInt(vector.slot),0n,0n,0n,BigInt(vector.clock)));
const d={schema:'flybrain-scientific-deployment-v5',enabled:true,cluster:'localnet',...Object.fromEntries(['programId','genesisHash','policyHash','mint'].map(k=>[k,vector.deployment[k]]))},program=new PublicKey(d.programId),a=addresses(program);
d.mint=a.mint.toBase58(); // This VM creates its own isolated mint through Init.
for(const who of [project,miner,rogue,...verifiers])svm.airdrop(who.publicKey.toBase58(),5000000000n);
const connection={async getGenesisHash(){return d.genesisHash;},async getAccountInfo(key){const v=svm.getAccount(key.toBase58());return v.exists?{owner:new PublicKey(v.programAddress),data:Buffer.from(v.data),lamports:Number(v.lamports)}:null;},async getSlot(){return Number(svm.getClock().slot);},async getBalance(key){return Number(svm.getBalance(key.toBase58())||0n);},async getMinimumBalanceForRentExemption(size){return Number(svm.minimumBalanceForRentExemption(BigInt(size)));},async getFeeForMessage(){return {value:5000};}};
const reader=new ScientificReaderV5(connection,d),contentStore=new ScientificContentStore(join(output,'content')),fixture=await scientificFixture(join(output,'model'));
const worker=new ExactTrialWorker({binary:native,graph:fixture.graphPath,model:fixture.modelPath}),checks=[],transactions=[];
const state=(key,decode)=>decode(Buffer.from(svm.getAccount(key.toBase58()).data));
function send(ixs,who=miner,{fail=false}={}){svm.expireBlockhash();const tx=new Transaction({feePayer:who.publicKey,recentBlockhash:svm.latestBlockhash()}).add(...(Array.isArray(ixs)?ixs:[ixs]));tx.sign(who);const result=svm.sendTransaction(getTransactionDecoder().decode(tx.serialize()));if(fail){assert(result instanceof FailedTransactionMetadata,'Expected rejection');return;}if(result instanceof FailedTransactionMetadata)throw new Error('SBF failed: '+result.err()+' '+result.meta().logs().join('\n'));transactions.push({bytes:tx.serialize().length,computeUnits:String(result.computeUnitsConsumed())});return result;}
const warp=slot=>{const clock=svm.getClock();clock.slot=BigInt(slot);svm.setClock(clock);};
const loader=new PublicKey('BPFLoaderUpgradeab1e11111111111111111111111'),programData=PublicKey.findProgramAddressSync([program.toBytes()],loader)[0],pd=new Uint8Array(45);new DataView(pd.buffer).setUint32(0,3,true);pd[12]=1;pd.set(project.publicKey.toBytes(),13);
svm.setAccount({address:programData.toBase58(),lamports:svm.minimumBalanceForRentExemption(45n),data:pd,programAddress:loader.toBase58(),executable:false});
const economic=(await import('../tools/economy_v2_client.mjs')).instruction,init=new Writer().key(new PublicKey(d.genesisHash)).key(d.policyHash);verifiers.forEach(k=>init.key(k.publicKey));
send(economic(program,[meta(a.global,true),meta(a.mint,true),...[0,1,2].map(n=>meta(a.pool(n),true)),meta(project.publicKey,true,true),system(),meta(TOKEN_PROGRAM_ID),meta(programData)],0,init.result()),project);
send(economic(program,[meta(a.global,true),meta(a.mint,true),...[0,1,2].map(n=>meta(a.pool(n),true)),meta(TOKEN_PROGRAM_ID),meta(project.publicKey,false,true)],1),project);
const start=BigInt(vector.slot)+10n,specification={id:hash(text('isolated-scientific-round-v5')),content:fixture.contentRoot,computationContext:fixture.context,analysis:fixture.analysis,auditInputs:fixture.auditInputs,rewardAtoms:20n,commitStart:start,commitEnd:start+100n,challengeSlot:start+105n,revealEnd:start+700n,verifyEnd:start+900n,backgrounds:2},round=a.round(specification.id);d.rounds=[round.toBase58()];
await contentStore.put(hex(specification.content),canonical(fixture.content));
const before=state(a.global,decodeGlobal).available[0];
send(instruction(program,[meta(a.global,true),meta(round,true),meta(rogue.publicKey,true,true),system()],0,{specification}),rogue,{fail:true});
send(instruction(program,[meta(a.global,true),meta(round,true),meta(project.publicKey,true,true),system()],0,{specification}),project);
assert.equal(state(a.global,decodeGlobal).available[0],before-200n);checks.push('Only the authority reserves exactly ten prospectively funded places');
const claims=[];
function prepare(role,assignment,input,who=miner){const r=state(round,decodeRound),c={round:round.toBytes(),miner:who.publicKey.toBytes(),beneficiary:who.publicKey.toBytes(),role,assignment},salt=hash(text('fixture-salt-'+claims.length)),commitment=inputCommitment(program,r,c,inputIdentity(input),salt),address=a.claim(round,who.publicKey,commitment);const item={role,assignment,input,who,salt,commitment,address};claims.push(item);return item;}
function commit(item){send(instruction(program,[meta(round,true),meta(item.address,true),meta(item.who.publicKey,true,true),system()],1,{commitment:item.commitment,beneficiary:item.who.publicKey,role:item.role,assignment:item.assignment}),item.who);}
const bad=prepare(0,255,fixture.input(100),rogue);send(instruction(program,[meta(round,true),meta(bad.address,true),meta(rogue.publicKey,true,true),system()],1,{commitment:bad.commitment,beneficiary:rogue.publicKey,role:0,assignment:255}),rogue,{fail:true});
warp(start);commit(bad);
for(const current of [100,125,150,175,200,225])commit(prepare(0,255,fixture.input(current)));
const duplicate=prepare(0,255,fixture.input(100));commit(duplicate);
for(let role=1;role<=2;role++)for(let i=0;i<2;i++)commit(prepare(role,i,fixture.content.auditInputs[(role-1)*2+i]));
checks.push('Twelve commitments preserve category order; six discoveries plus four audit places are finite');
warp(specification.challengeSlot+1n);svm.setSlotHashes([{slot:specification.challengeSlot+1n,hash:Keypair.generate().publicKey.toBase58()},{slot:specification.challengeSlot,hash:Keypair.generate().publicKey.toBase58()}]);
send(instruction(program,[meta(round,true),slotHashes()],2));const challenged=await reader.round(round);assert.equal(challenged.sourceSlot,specification.challengeSlot);
send(instruction(program,[meta(round,true),meta(a.claim(round,miner.publicKey,hash(text('late'))),true),meta(miner.publicKey,true,true),system()],1,{commitment:hash(text('late')),beneficiary:miner.publicKey,role:0,assignment:255}),miner,{fail:true});checks.push('Challenge selects the first authenticated produced slot after commitments close');
for(const [index,item]of claims.entries()){
  const r=await reader.round(round),seeds=backgroundSeeds(r,item.role,item.assignment),trials=[];
  for(let j=0;j<2;j++){
    const responsePath=join(output,'archive-'+index+'-'+j+'.bin'),result=await worker.run({goal:fixture.goal,input:item.input,seedHex:seeds[j],responsePath}),bytes=new Uint8Array(await readFile(responsePath));
    if(index===0&&j===0)bytes[48]^=1; // Correct hash/metadata cannot hide a changed full archive.
    const root=hex(hash(bytes));await contentStore.put(root,bytes);trials.push({index:j,seedHex:seeds[j],resultRoot:root,bytes:bytes.length,metrics:result.metrics});
  }
  const manifest={schema:'flybrain-scientific-manifest-v5',round:round.toBase58(),input:item.input,role:item.role,assignment:item.assignment,computation:hex(computationIdentity(r,item.role,item.assignment,inputIdentity(item.input))),analysis:hex(r.specification.analysis),trials},manifestRoot=contentRoot(manifest);await contentStore.put(hex(manifestRoot),canonical(manifest));
  const reveal={input:inputIdentity(item.input),salt:item.salt,manifest:manifestRoot,result:contentRoot(trials)};
  if(index===1)send(instruction(program,[meta(round),meta(item.address,true),meta(item.who.publicKey,false,true)],3,{...reveal,salt:hash(text('wrong'))}),item.who,{fail:true});
  send(instruction(program,[meta(round),meta(item.address,true),meta(item.who.publicKey,false,true)],3,reveal),item.who);
  item.verified=await verifyScientificClaim({reader,roundAddress:round,claimAddress:item.address,contentStore,worker,outputDir:join(output,'checks')});
  assert.equal(item.verified.report.complete,index!==0);if(item.role===2)assert.equal(item.verified.report.passed,false);
}
// Verify a later candidate first: it still cannot overtake the earlier claim.
for(const signer of verifiers.slice(0,2))send(scientificVoteInstructions(reader,claims[1].address,claims[1].verified,signer));
const resolve=item=>instruction(program,[meta(round,true),meta(item.address,true),meta(a.computation(state(item.address,decodeClaim).computation),true),meta(miner.publicKey,true,true),system()],5);
send(resolve(claims[1]),miner,{fail:true});
for(const [i,item]of claims.entries()){
  if(i!==1)for(const signer of verifiers.slice(0,2))send(scientificVoteInstructions(reader,item.address,item.verified,signer));
  send(resolve(item));
}
const accepted=await reader.round(round);assert.equal(accepted.acceptedUnpaid,200n);assert(accepted.winners.every(k=>k.some(x=>x)));assert.equal(state(bad.address,decodeClaim).state,3);assert.equal(state(duplicate.address,decodeClaim).state,4);
checks.push('A modified archive is rejected by the actual native checker; later verified work cannot overtake it');checks.push('The identical computation cannot earn twice; two negative stress tests earn for correct assigned execution');
send(instruction(program,[meta(a.global,true),meta(round,true)],7));assert.equal((await reader.round(round)).closed,true);
const destination=getAssociatedTokenAddressSync(new PublicKey(d.mint),miner.publicKey);send(createAssociatedTokenAccountIdempotentInstruction(miner.publicKey,destination,miner.publicKey,new PublicKey(d.mint)));
for(const key of accepted.winners){const claim=new PublicKey(key),keys=[meta(a.global),meta(round,true),meta(claim,true),meta(a.pool(0),true),meta(destination,true),meta(TOKEN_PROGRAM_ID)];send(instruction(program,keys,6));send(instruction(program,keys,6));}
const paid=await reader.round(round),token=svm.getAccount(destination.toBase58());assert.equal(unpackAccount(destination,{owner:TOKEN_PROGRAM_ID,data:Buffer.from(token.data)}).amount,200n);assert.equal(paid.paid,200n);assert.equal(paid.acceptedUnpaid,0n);assert.equal(state(a.global,decodeGlobal).available[0],before-200n);
checks.push('All ten entitlements survive closure and SPL payout retries transfer each amount exactly once');
const browserCases=[];
async function browserCase(name,deployment,address,content,input,paths,expected){
  const graphFile=await open(paths.graph,'r'),header=Buffer.alloc(20);await graphFile.read(header,0,20,0);await graphFile.close();
  const snapshot=svm.getAccount(address.toBase58()),config={neuron_count:content.goal.neuronCount,steps:256,time_step_us:1000,observe_start:0,parameter_version:'lif-point-v0-dev',graph_sha256:content.goal.graphRoot,model_sha256:content.goal.modelDataRoot,model_root_sha256:content.goal.modelRoot,registry_sha256:content.goal.registryRoot,graph_bytes:(await stat(paths.graph)).size,model_bytes:(await stat(paths.model)).size,directed_connection_count:Number(header.readBigUInt64LE(12)),max_archive_bytes:content.goal.maxTrialBytes};
  const value={scope:'Isolated SBF chain-read fixture, actual model files and native expected bytes; no public finality claim',deployment:{...deployment,rounds:[address.toBase58()]},roundAddress:address.toBase58(),roundAccount:{owner:program.toBase58(),data:Array.from(snapshot.data)},content,input,role:0,assignment:255,config,graphPath:paths.graph,modelPath:paths.model,expected};
  const file=join(output,'browser-'+name+'.json');await writeFile(file,JSON.stringify(value,null,2)+'\n');browserCases.push(file);
}
await browserCase('six-cell',d,round,fixture.content,fixture.input(100),{graph:fixture.graphPath,model:fixture.modelPath},{root:claims[1].verified.report.measurements[0].resultRoot,metrics:claims[1].verified.report.measurements[0].metrics});
if(process.env.SCIENCE_V5_FULL_MODEL_DIR&&process.env.SCIENCE_V5_FULL_GOAL){
  const fullGoal=JSON.parse(await readFile(process.env.SCIENCE_V5_FULL_GOAL,'utf8')),input={stimulatedBodyId:fullGoal.stimulusSet[0].bodyId,startStep:fullGoal.inputDomain.starts[0],endStep:fullGoal.inputDomain.starts[0]+fullGoal.inputDomain.durations[0],current:fullGoal.inputDomain.currents[0]},fullContent={...fixture.content,goal:fullGoal,auditInputs:Array.from({length:4},()=>input),auditPlans:fixture.content.auditPlans.map(p=>({...p,referenceEvidence:fullGoal.calibrationHash})),scope:'Complete retained model execution acceptance; no biological conclusion'};
  const next=svm.getClock().slot+10n,spec={...specification,id:hash(text('full-model-browser-round-v5')),content:contentRoot(fullContent),computationContext:computationContext(fullGoal),analysis:contentRoot({goal:fullGoal,coverage:fullContent.offTargetCoverage,evaluator:'complete-selective-v5'}),auditInputs:fullContent.auditInputs.map(inputIdentity),commitStart:next,commitEnd:next+100n,challengeSlot:next+105n,revealEnd:next+700n,verifyEnd:next+900n},address=a.round(spec.id);
  send(instruction(program,[meta(a.global,true),meta(address,true),meta(project.publicKey,true,true),system()],0,{specification:spec}),project);
  warp(spec.challengeSlot+1n);svm.setSlotHashes([{slot:spec.challengeSlot,hash:Keypair.generate().publicKey.toBase58()}]);send(instruction(program,[meta(address,true),slotHashes()],2));
  const fullModel=process.env.SCIENCE_V5_FULL_MODEL_DIR,paths={graph:join(fullModel,'graph-v1.bin'),model:join(fullModel,'model-v0.bin')},nativeFull=new ExactTrialWorker({binary:native,graph:paths.graph,model:paths.model});
  const seed=backgroundSeeds(await reader.round(address),0,255)[0],result=await nativeFull.run({goal:fullGoal,input,seedHex:seed,responsePath:join(output,'full-model-expected.bin')});nativeFull.close();
  await browserCase('full-model',d,address,fullContent,input,paths,{root:result.resultRoot,metrics:result.metrics});
}
let durableAcceptance=false;
if(process.env.FLYBRAIN_POOL_TEST_SOCKET){
  const socket=process.env.FLYBRAIN_POOL_TEST_SOCKET;if(!socket.startsWith('/var/tmp/flybrainlabs-postgres-acceptance-'))throw new Error('Isolated database required');
  const database='science_v5_'+randomUUID().replaceAll('-',''),options={host:socket,port:55439,user:'pool_acceptance',database:'postgres'},admin=new pg.Pool(options);await admin.query('CREATE DATABASE '+database);await admin.end();
  const queue=await new FactoryPoolQueueV4(new pg.Pool({...options,database})).initialize(),start2=svm.getClock().slot+10n,spec2={...specification,id:hash(text('durable-isolated-scientific-round-v5')),commitStart:start2,commitEnd:start2+100n,challengeSlot:start2+105n,revealEnd:start2+700n,verifyEnd:start2+900n},round2=a.round(spec2.id);
  send(instruction(program,[meta(a.global,true),meta(round2,true),meta(project.publicKey,true,true),system()],0,{specification:spec2}),project);
  const deployment={...d,rounds:[round2.toBase58()]},keyStore={record:null,async load(){return this.record;},async save(v){this.record=structuredClone(v);}},vault=await new MiningWalletV4({deployment,store:keyStore}).initialize();
  await vault.save(newMiningMnemonic(),Buffer.from(crypto.getRandomValues(new Uint8Array(16))).toString('hex'),{confirmed:true,restored:true});svm.airdrop(vault.address,1000000000n);
  const policy=new ScientificWalletPolicyV5({deployment,connection,address:vault.address});vault.policy=policy;await policy.approve();const provider=vault.provider(),account=provider.accounts[0],wallet={publicKey:new PublicKey(account.publicKey)},statuses=new Map();
  const base58=bytes=>{const alphabet='123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';let n=BigInt('0x'+Buffer.from(bytes).toString('hex')),s='';while(n){s=alphabet[Number(n%58n)]+s;n/=58n;}for(const b of bytes){if(b)break;s='1'+s;}return s;};
  function submit(raw){const tx=Transaction.from(raw),result=svm.sendTransaction(getTransactionDecoder().decode(raw));if(result instanceof FailedTransactionMetadata)throw new Error('Durable SBF transaction failed: '+result.meta().logs().join('\n'));const signature=base58(tx.signature);statuses.set(signature,{confirmationStatus:'finalized',slot:Number(svm.getClock().slot),err:null});transactions.push({bytes:raw.length,computeUnits:String(result.computeUnitsConsumed())});return signature;}
  const serviceConnection={...connection,async getLatestBlockhash(){svm.expireBlockhash();return {blockhash:svm.latestBlockhash(),lastValidBlockHeight:Number(svm.getClock().slot)+1000};},async getSignatureStatuses(keys){return {value:keys.map(k=>statuses.get(k)||null)};},async getBlockHeight(){return Number(svm.getClock().slot);},async sendRawTransaction(raw){return submit(raw);}};
  const sendDedicated=async ixs=>{svm.expireBlockhash();const tx=new Transaction({feePayer:wallet.publicKey,recentBlockhash:svm.latestBlockhash()}).add(...ixs),[signed]=await provider.features['solana:signTransaction'].signTransaction({account,chain:'solana:localnet',transaction:tx.serialize({requireAllSignatures:false,verifySignatures:false})});return submit(signed.signedTransaction);};
  await assert.rejects(policy.check(new Transaction({feePayer:wallet.publicKey,recentBlockhash:svm.latestBlockhash()}).add(SystemProgram.transfer({fromPubkey:wallet.publicKey,toPubkey:rogue.publicKey,lamports:1}))));
  const config={deployment,rounds:[round2.toBase58()],maxTransactionsPerSigner:128,maxFeeLamportsPerSigner:1000000,outputDir:join(output,'durable-verifier')};let calls=0,restarted=false;
  const nativeWorker=()=>new ExactTrialWorker({binary:native,graph:fixture.graphPath,model:fixture.modelPath});
  const makeService=async()=>new ServiceRuntime({config,connection:serviceConnection,queue,signers:verifiers.slice(0,2),worker:nativeWorker(),contentStore,verify:async args=>{calls++;return verifyScientificClaim(args);}}).initialize();
  let service=await makeService(),server=service.serve({port:0});await new Promise(resolve=>server.once('listening',resolve));const port=server.address().port,source='http://127.0.0.1:'+port+'/api/science/v5/content/',store=new FactoryContentStoreV4({sources:[source]});
  const client=new ScientificClientV5({connection,deployment,wallet,send:sendDedicated,contentStore:store}),records=new Map(),journal={async get(k){return records.get(k)||null;},async put(k,v){records.set(k,structuredClone(v));}};
  let implementation=await readFile('web/scientific-mining-v5.mjs','utf8');implementation=implementation.replace("'./target-protocol-v2.mjs'",JSON.stringify(pathToFileURL(join(process.cwd(),'web/target-protocol-v2.mjs')).href)).replace("'/assets/vendor/generated/scientific-v5.mjs'",JSON.stringify(pathToFileURL(join(process.cwd(),'web/scientific-client-v5.mjs')).href));const runnerFile=join(output,'miner-orchestration.mjs');await writeFile(runnerFile,implementation);const {ScientificMiningRunnerV5}=await import(pathToFileURL(runnerFile).href);
  let trials=0,paused=false,runner;
  const compute=async({task})=>{const path=join(output,'miner-real-'+trials+++'.bin'),result=await worker.run({goal:fixture.goal,input:task.input,seedHex:task.seedHex,responsePath:path});return {archive:new Blob([await readFile(path)]),goalMetrics:result.metrics,resultHash:result.resultRoot,responseBytes:result.bytes};};
  const publish=async({round,claim,root,bytes})=>{const message=uploadMessage({genesis:deployment.genesisHash,program:deployment.programId,round,claim,root,bytes:bytes.size}),[signed]=await provider.features['solana:signMessage'].signMessage({account,message});const response=await fetch(source+root+'.bin',{method:'PUT',headers:{'X-Scientific-Round':round,'X-Scientific-Claim':claim,'X-Scientific-Signature':hex(signed.signature)},body:bytes});assert.equal(response.status,201);};
  const wait=async()=>{
    if(!paused){paused=true;runner.stop();return;}
    if(svm.getClock().slot<spec2.challengeSlot){warp(spec2.challengeSlot+1n);svm.setSlotHashes([{slot:spec2.challengeSlot,hash:Keypair.generate().publicKey.toBase58()}]);}
    await service.tick();const c=[...records.values()][0];
    if(c&&!restarted&&svm.getAccount(c.claim).exists&&decodeClaim(Buffer.from(svm.getAccount(c.claim).data)).completeVotes){
      const closed=new Promise(resolve=>server.once('close',resolve));service.stop();await closed;service=await makeService();server=service.serve({port});await new Promise(resolve=>server.once('listening',resolve));restarted=true;
    }
    await new Promise(resolve=>setTimeout(resolve,25));
  };
  warp(start2);runner=new ScientificMiningRunnerV5({client,compute,publish,journal,wait});await assert.rejects(runner.run(round2.toBase58(),{role:2,assignment:0}),e=>e.code==='SCIENCE_PAUSED');
  assert.equal([...records.values()][0].phase,'committed');runner=new ScientificMiningRunnerV5({client,compute,publish,journal,wait});
  const result=await Promise.race([runner.run(round2.toBase58(),{role:2,assignment:0}),new Promise((_,reject)=>setTimeout(()=>reject(new Error('Durable acceptance timeout')),45000))]);
  assert.equal(result.phase,'paid');assert.equal(calls,1);assert(restarted);assert.equal(trials,2);assert.equal((await reader.round(round2)).paid,20n);
  vault.lock();await assert.rejects(provider.features['solana:signMessage'].signMessage({account,message:uploadMessage({genesis:d.genesisHash,program:d.programId,round:round2.toBase58(),claim:result.claim,root:result.manifestRoot,bytes:10})}));
  service.stop();await queue.close();durableAcceptance=true;
  checks.push('The scoped mnemonic wallet, real miner orchestration, signed HTTP evidence, PostgreSQL recovery and node settlement run end to end');
  checks.push('Pausing after commitment resumes without a second claim; a verifier restart reuses one checked report for the disclosed single operator');
}
worker.close();const report={schema:'flybrain-scientific-round-v5-sbf-acceptance',passed:true,scope:'Isolated LiteSVM, real six-cell Rust simulation, Ed25519 quorum and SPL transfers; no public transactions or biological acceptance',checks,transactions:transactions.length,maxPacket:Math.max(...transactions.map(x=>x.bytes)),maxComputeUnits:Math.max(...transactions.map(x=>Number(x.computeUnits))),programSha256:createHash('sha256').update(await readFile(binary)).digest('hex'),publicChainTransactions:0,tenPlacesPaid:true,durableAcceptance,productionFunds:false,independentOperators:false};await writeFile(join(output,'acceptance.json'),JSON.stringify(report,null,2)+'\n');await writeFile(join(artifacts,'latest.json'),JSON.stringify({output,browserCases,...report},null,2)+'\n');console.log(JSON.stringify(report));
