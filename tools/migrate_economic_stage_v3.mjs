// Inspect by default. Migration keeps the Stage PDA and all frozen slot bytes.
// Legacy accepted rights remain claimable without this counter migration.
import {Connection,PublicKey,Keypair,Transaction,SystemProgram,sendAndConfirmTransaction} from '@solana/web3.js';
import {readFile,writeFile} from 'node:fs/promises';import {createHash} from 'node:crypto';
import {addresses,decodeStage,decodeSlot,meta,instruction} from './economy_v2_client.mjs';
const args=process.argv.slice(2),value=k=>args[args.indexOf('--'+k)+1];
if(!['rpc','program','genesis','stage','report'].every(k=>args.includes('--'+k)))throw new Error('usage: --rpc URL --program KEY --genesis HASH --stage PDA --report AUDIT.json [--apply --authority-key KEYPAIR]');
const c=new Connection(value('rpc'),'finalized'),pid=new PublicKey(value('program')),stage=new PublicKey(value('stage')),genesis=await c.getGenesisHash();if(genesis!==value('genesis'))throw new Error('Wrong frozen chain');
const observed=await c.getAccountInfoAndContext(stage,{commitment:'finalized'});if(!observed.value?.owner.equals(pid))throw new Error('Wrong stage owner');const s=decodeStage(observed.value.data);
if(!addresses(pid).stage(s.id).equals(stage))throw new Error('Wrong stage PDA');
const inventory=await c.getProgramAccounts(pid,{withContext:true,commitment:'finalized',filters:[{dataSize:512},{memcmp:{offset:73,bytes:stage.toBase58()}}]});
const rows=inventory.value.map(a=>({address:a.pubkey.toBase58(),state:decodeSlot(a.account.data),sha256:createHash('sha256').update(a.account.data).digest('hex')}));const live=rows.filter(r=>!r.state.released),allocated=live.reduce((n,r)=>n+r.state.amount,0n),pending=live.filter(r=>!r.state.accepted),unaccepted=pending.reduce((n,r)=>n+r.state.amount,0n);
const reread=await c.getAccountInfoAndContext(stage,{commitment:'finalized',minContextSlot:inventory.context.slot});if(!reread.value?.data.equals(observed.value.data))throw new Error('Stage changed during inventory; retry actual observation');
const report={schema:'flybrain-stage-v3-migration-audit-v1',genesisHash:genesis,programId:pid.toBase58(),stagePda:stage.toBase58(),version:s.schemaVersion,rawBytesHex:observed.value.data.toString('hex'),rawSha256:createHash('sha256').update(observed.value.data).digest('hex'),observations:[{method:'getAccountInfoAndContext',contextSlot:String(observed.context.slot)},{method:'getProgramAccounts withContext',contextSlot:String(inventory.context.slot)},{method:'getAccountInfoAndContext actual reread',contextSlot:String(reread.context.slot),minContextSlotIsOnlyLowerBound:true}],slotCount:rows.length,nonReleasedCount:live.length,pendingCount:pending.length,pendingAtoms:unaccepted.toString(),completeInventoryEquation:s.remaining+allocated===s.budget,slotInventory:rows.map(r=>({address:r.address,sha256:r.sha256,amountAtoms:r.state.amount.toString(),accepted:r.state.accepted,released:r.state.released})),mutated:false};
await writeFile(value('report'),JSON.stringify(report,null,2)+'\n',{mode:0o600});
if(args.includes('--apply')){
 if(s.schemaVersion!==2||s.closed||!report.completeInventoryEquation||live.length>16)throw new Error('Migration conditions: open legacy stage, complete inventory, bounded transaction required; unknown reservations never zero');
 const key=Keypair.fromSecretKey(Uint8Array.from(JSON.parse(await readFile(value('authority-key'),'utf8')))),a=addresses(pid);const ix=instruction(pid,[meta(a.global),meta(stage,true),meta(key.publicKey,false,true),...live.map(r=>meta(r.address))],9);const signature=await sendAndConfirmTransaction(c,new Transaction().add(ix),[key],{commitment:'finalized'});report.mutated=true;report.signature=signature;await writeFile(value('report'),JSON.stringify(report,null,2)+'\n',{mode:0o600});
}
console.log(JSON.stringify({inspected:true,version:report.version,completeInventory:report.completeInventoryEquation,mutated:report.mutated}));
