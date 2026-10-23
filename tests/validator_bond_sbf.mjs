// Actual SBF and SPL instructions in an isolated VM; no public funds or clock changes.
import assert from 'node:assert/strict';
import {LiteSVM,Clock,FailedTransactionMetadata} from 'litesvm';
import {getTransactionDecoder} from '@solana/kit';
import {Keypair,PublicKey,Transaction,SystemProgram} from '@solana/web3.js';
import {TOKEN_PROGRAM_ID,MINT_SIZE,createInitializeMint2Instruction,createMintToInstruction,createSetAuthorityInstruction,AuthorityType,getAssociatedTokenAddressSync,createAssociatedTokenAccountIdempotentInstruction,createTransferInstruction,createApproveInstruction,createRevokeInstruction,unpackAccount} from '@solana/spl-token';
import {bondInstruction,bondAddresses,decodeBond,ValidatorBondReaderV1,MINIMUM_BOND_ATOMS,LOCK_SECONDS,UNBOND_SECONDS,DEVNET_GENESIS} from '../web/validator-bond-v1.mjs';
import {createServer} from 'node:http';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {ed25519} from '@noble/curves/ed25519';
import {FactoryValidatorFleetV4} from '../tools/factory_validator_fleet_v4.mjs';
import {startValidatorWorkerV4} from '../tools/factory_validator_worker_v4.mjs';
import {canonical,hash} from '../web/target-protocol-v2.mjs';

const svm=new LiteSVM(),project=Keypair.generate(),owner=Keypair.generate(),rogue=Keypair.generate(),mint=Keypair.generate(),program=Keypair.generate().publicKey;
const elf=process.env.BOND_SBF;if(!elf)throw new Error('BOND_SBF required');
svm.addProgramFromFile(program.toBase58(),elf);
for(const k of [project,owner,rogue])svm.airdrop(k.publicKey.toBase58(),5000000000n);
let checks=0,transactions=0,failures=0,slot=1000n;
svm.setClock(new Clock(slot,0n,0n,0n,1800000000n));
const loader=new PublicKey('BPFLoaderUpgradeab1e11111111111111111111111'),a=bondAddresses(program,owner.publicKey),pd=Buffer.alloc(45);pd.writeUInt32LE(3);pd[12]=1;project.publicKey.toBuffer().copy(pd,13);
svm.setAccount({address:a.programData.toBase58(),lamports:svm.minimumBalanceForRentExemption(45n),data:pd,programAddress:loader.toBase58(),executable:false});
function send(ixs,signers=[project],fail=false){
  svm.expireBlockhash();const clock=svm.getClock();clock.slot=++slot;svm.setClock(clock);
  const tx=new Transaction({feePayer:project.publicKey,recentBlockhash:svm.latestBlockhash()}).add(...(Array.isArray(ixs)?ixs:[ixs]));
  tx.sign(...signers);const result=svm.sendTransaction(getTransactionDecoder().decode(tx.serialize()));
  if(fail){assert(result instanceof FailedTransactionMetadata,'Expected on-chain rejection');failures++;return;}
  if(result instanceof FailedTransactionMetadata)throw new Error('SBF failure '+result.err()+' '+result.meta().logs().join('\n'));
  transactions++;return result;
}
const source=getAssociatedTokenAddressSync(mint.publicKey,owner.publicKey),rogueSource=getAssociatedTokenAddressSync(mint.publicKey,rogue.publicKey);
send([SystemProgram.createAccount({fromPubkey:project.publicKey,newAccountPubkey:mint.publicKey,lamports:Number(svm.minimumBalanceForRentExemption(BigInt(MINT_SIZE))),space:MINT_SIZE,programId:TOKEN_PROGRAM_ID}),createInitializeMint2Instruction(mint.publicKey,8,project.publicKey,null),...[owner,rogue].map(k=>createAssociatedTokenAccountIdempotentInstruction(project.publicKey,getAssociatedTokenAddressSync(mint.publicKey,k.publicKey),k.publicKey,mint.publicKey)),createMintToInstruction(mint.publicKey,source,project.publicKey,MINIMUM_BOND_ATOMS*3n),createMintToInstruction(mint.publicKey,rogueSource,project.publicKey,MINIMUM_BOND_ATOMS),createSetAuthorityInstruction(mint.publicKey,project.publicKey,AuthorityType.MintTokens,null)],[project,mint]);
const d={schema:'flybrain-validator-bond-deployment-v1',cluster:'devnet',genesisHash:DEVNET_GENESIS,programId:program.toBase58(),mint:mint.publicKey.toBase58(),clearanceAuthority:project.publicKey.toBase58()};
const init=payer=>bondInstruction(program,0,{mint:mint.publicKey,payer:payer.publicKey});
send(init(rogue),[project,rogue],true);send(init(project));send(init(project),[project],true);checks++;
const account=k=>{const v=svm.getAccount(k.toBase58());return v.exists?{owner:new PublicKey(v.programAddress),data:Buffer.from(v.data),lamports:Number(v.lamports),executable:v.executable}:null;};
const connection={async getGenesisHash(){return DEVNET_GENESIS;},async getMultipleAccountsInfoAndContext(keys){return {context:{slot:Number(slot)},value:keys.map(account)};}};
const reader=new ValidatorBondReaderV1(connection,d),stake=(who=owner,extra={})=>bondInstruction(program,1,{mint:mint.publicKey,validator:who.publicKey,payer:project.publicKey,source:who===owner?source:rogueSource,...extra});
await assert.rejects(reader.check(owner.publicKey),/VALIDATOR_BOND_REQUIRED/);
send(stake(owner,{amount:MINIMUM_BOND_ATOMS-1n}),[project,owner],true);
send(stake(owner,{source:rogueSource}),[project,owner],true);checks++;
send(createApproveInstruction(source,rogue.publicKey,owner.publicKey,1n),[project,owner]);send(stake(),[project,owner],true);send(createRevokeInstruction(source,owner.publicKey),[project,owner]);checks++;
// Prefunding a PDA must not block its legitimate allocation.
send([SystemProgram.transfer({fromPubkey:project.publicKey,toPubkey:a.bond,lamports:Number(svm.minimumBalanceForRentExemption(0n))}),SystemProgram.transfer({fromPubkey:project.publicKey,toPubkey:a.vault,lamports:Number(svm.minimumBalanceForRentExemption(0n))})]);
send(stake(),[project,owner]);let b=decodeBond(account(a.bond).data);assert.equal(b.amount,MINIMUM_BOND_ATOMS);assert.equal((await reader.check(owner.publicKey)).state,'active');checks++;
send(stake(),[project,owner],true);const wrongVault=stake(rogue);wrongVault.keys[2].pubkey=a.vault;send(wrongVault,[project,rogue],true);checks++;
send(createTransferInstruction(a.vault,rogueSource,rogue.publicKey,1n),[project,rogue],true);checks++;
const exit=who=>bondInstruction(program,2,{validator:who.publicKey});
send(exit(owner),[project,owner],true);
const wrongExit=exit(owner);wrongExit.keys[2].pubkey=rogue.publicKey;send(wrongExit,[project,rogue],true);checks++;
function time(t){const c=svm.getClock();c.unixTimestamp=t;svm.setClock(c);}
time(b.activatedAt+LOCK_SECONDS-1n);send(exit(owner),[project,owner],true);
time(b.activatedAt+LOCK_SECONDS);send(exit(owner),[project,owner]);b=decodeBond(account(a.bond).data);assert.equal(b.withdrawAfter,b.activatedAt+LOCK_SECONDS+UNBOND_SECONDS);await assert.rejects(reader.check(owner.publicKey),/VALIDATOR_BOND_INACTIVE/);assert.equal((await reader.check(owner.publicKey,{allowExiting:true})).state,'exiting');checks++;
const evidence=new Uint8Array(32).fill(7),withdraw=(extra={})=>bondInstruction(program,3,{mint:mint.publicKey,validator:owner.publicKey,destination:source,clearanceAuthority:project.publicKey,generation:1n,evidence,...extra});
send(withdraw(),[project,owner],true);time(b.withdrawAfter-1n);send(withdraw(),[project,owner],true);time(b.withdrawAfter);
send(withdraw({clearanceAuthority:rogue.publicKey}),[project,owner,rogue],true);
send(withdraw({destination:rogueSource}),[project,owner],true);
send(withdraw({evidence:new Uint8Array(32)}),[project,owner],true);
send(withdraw({generation:2n}),[project,owner],true);checks++;
send(withdraw(),[project,owner]);assert.equal(decodeBond(account(a.bond).data).amount,0n);assert.equal(unpackAccount(a.vault,account(a.vault)).amount,0n);send(withdraw(),[project,owner],true);checks++;
send(stake(),[project,owner]);const second=decodeBond(account(a.bond).data);assert.equal(second.generation,2n);assert.equal(second.activatedAt,b.withdrawAfter);assert.equal((await reader.check(owner.publicKey)).generation,'2');send(withdraw(),[project,owner],true);checks++;
await assert.rejects(new ValidatorBondReaderV1({...connection,async getGenesisHash(){return Keypair.generate().publicKey.toBase58();}},d).check(owner.publicKey),/BOND_CHAIN_SCOPE/);
await assert.rejects(new ValidatorBondReaderV1(connection,{...d,mint:rogue.publicKey.toBase58()}).check(owner.publicKey));
assert.throws(()=>new ValidatorBondReaderV1(connection,{...d,cluster:'mainnet-beta'}),/BOND_DEPLOYMENT_REQUIRED/);checks++;
// Signed HTTP reports remain fixtures, while admission reads actual SBF state.
let dispatched=0,tamperBond=false;
const member={id:'bonded-validator',publicKey:Buffer.from(owner.publicKey.toBytes()).toString('hex')},http=createServer(async(req,res)=>{
  if(req.url==='/health'){res.end(JSON.stringify({validatorId:member.id,publicKey:member.publicKey,ready:true,busy:false,completed:0,staking:await reader.check(owner.publicKey)}));return;}
  dispatched++;let raw='';for await(const part of req)raw+=part;const request=JSON.parse(raw);
  const bond=await reader.check(owner.publicKey);if(tamperBond)bond.generation='999';
  const report={submission:request.submission,task:'fixture',attempt:0,fullReplay:true,completeTrials:1,valid:true,measurements:[{fullReplay:true,replayVerified:true,replayStatus:'VALID',resultRoot:'a'.repeat(64),bytes:1,steps:1,metrics:{}}]};
  const body={schema:'flybrain-validator-attestation-v1',validatorId:member.id,publicKey:member.publicKey,nonce:request.nonce,operation:request.operation,factory:request.deployment.factory,submission:request.submission,completedAt:Date.now(),report,reportHash:await hash(canonical(report)),bond};
  res.end(JSON.stringify({...body,signature:Buffer.from(ed25519.sign(canonical(body),owner.secretKey.subarray(0,32))).toString('hex')}));
});
await new Promise(ok=>http.listen(0,'127.0.0.1',ok));
const url='http://127.0.0.1:'+http.address().port,config={schema:'flybrain-validator-fleet-v1',minimumAttestations:1,requestTimeoutMs:2000,members:[{...member,url},{id:'unbonded-validator',publicKey:Buffer.from(rogue.publicKey.toBytes()).toString('hex'),url}]};
try{
 const fleet=new FactoryValidatorFleetV4(config,'fixture-token');fleet.bonds=reader;await fleet.initialize();assert.equal(fleet.summary().ready,1);
 const input={operation:'replay',submission:'fixture',deployment:{factory:'fixture'}};
 const result=await fleet.replayAll(input);assert.equal(result.report.distributedVerification.attestations.length,1);assert.equal(dispatched,1);checks++;
 tamperBond=true;await assert.rejects(fleet.replayAll(input),/FLEET_REPLAY_QUORUM/);checks++;
 time(second.activatedAt+LOCK_SECONDS);send(exit(owner),[project,owner]);const count=dispatched;
 await assert.rejects(fleet.replayAll(input),/FLEET_REPLAY_QUORUM/);assert.equal(dispatched,count);await assert.rejects(fleet.initialize(),/FLEET_QUORUM_UNAVAILABLE/);checks++;
}finally{await new Promise(ok=>http.close(ok));}
const temporary=await mkdtemp(join(tmpdir(),'fbw-bond-startup-'));
try{
 await writeFile(join(temporary,'token'),'fixture-token',{mode:0o600});await writeFile(join(temporary,'identity'),owner.secretKey.subarray(0,32),{mode:0o600});
 await assert.rejects(startValidatorWorkerV4({tokenFile:join(temporary,'token'),identityFile:join(temporary,'identity'),relayUrl:'http://127.0.0.1:1'}),/BOND_DEPLOYMENT_REQUIRED/);checks++;
}finally{await rm(temporary,{recursive:true});}
console.log(JSON.stringify({passed:true,scope:'Isolated real SBF/SPL execution and finalized-read interface; signed HTTP scientific reports are fixtures; VM clock only, no public-chain time travel',checks,successfulTransactions:transactions,rejectedTransactions:failures,minimumFbw:100000,lockDays:30,unbondDays:7,ownerAndClearanceRequired:true,noSlashingInstruction:true,unbondedStartupRejected:true,unbondedOrExitingDispatchRejected:true,signedWrongGenerationRejected:true}));
