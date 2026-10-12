// Only loopback localnet is enabled by this bootstrap. No key or mint is written to Git.
import { Keypair,PublicKey } from '@solana/web3.js';
import { readFile,writeFile,mkdir } from 'node:fs/promises';
import { taskForIndex } from '../web/catalog-task.mjs';
import { connection,loadKey,setup,merkle,hash,text,hex } from './campaign_tools.mjs';
import { readChainClock } from '../web/solana-client.mjs';
const out=process.argv[2];if(!out)throw new Error('Usage: node tools/setup_campaign.mjs NEW_PRIVATE_OUTPUT');
const u=new URL(connection.rpcEndpoint);if(!['127.0.0.1','localhost'].includes(u.hostname))throw new Error('Bootstrap is localnet only');
await mkdir(out);const original=JSON.parse(await readFile('web/data/task-catalog-dev.json'));const c=original.campaign;
c.campaign_id=Array.from(hash(text('FlyPoW/full-MaleCNS-aipg10-single-task-local-pilot/v1\0'),crypto.getRandomValues(new Uint8Array(32))));
c.targets=[c.targets[0]];c.stimuli=[{start_step:4,end_step:12,current:200}];c.repeats=1;
const tasks=[await taskForIndex({campaign:c},0)],tree=merkle(tasks),payer=await loadKey('deploy/localnet/payer.json'),verifiers=Array.from({length:3},()=>Keypair.generate());
for(let i=0;i<3;i++)await writeFile(`${out}/verifier-${i}.json`,JSON.stringify(Array.from(verifiers[i].secretKey)),{flag:'wx',mode:0o600});
const r={catalog:c,manifest_root:tree.root,readout_root:hash(text('FlyPoW/readout/vnc-aggregate-32bins/dev-v1\0'),Uint8Array.from(c.registry_root)),genesis:new PublicKey(await connection.getGenesisHash()).toBytes(),verifiers:verifiers.map(v=>v.publicKey.toBytes()),developer:payer.publicKey.toBytes(),community:payer.publicKey.toBytes(),mint:new Uint8Array(32),roi_count:1,readout_schema_id:1,verifier_policy_id:1,simulation_config_id:1,observe_start:0,submit_deadline:Number((await readChainClock(connection)).unixTimestamp)+1800,reveal_ttl:300,verify_ttl:600,archive_ttl:600,min_reveal_slots:2,advance:5000,queue_fee:0,verify_fee:1000,rejection_slash:0,archive_retention_seconds:604800,miner_pool_whole:100,validator_pool_whole:25,community_pool_whole:10,network:0};
const ctx=await setup(r,tree,out+'/public');
const catalog={state:'UNCALIBRATED_DEV_ONLY_NO_PAYMENT',campaign:c,graph_data_sha256:original.graph_data_sha256,model_data_sha256:original.model_data_sha256,task_manifest_root:Array.from(tree.root)};
const raw=new TextEncoder().encode(JSON.stringify(catalog,null,2)+'\n');await writeFile(out+'/catalog.json',raw,{flag:'wx'});
const cfg=JSON.parse(await readFile('web/data/research-config.json'));Object.assign(cfg,{target_body_ids:[16789],stimulus_count:1,repeat_count:1,default_stimulus_index:0,allow_wallet:true,task_catalog_sha256:hex(hash(raw)),campaign:'full-MaleCNS-one-task-LOCAL-TEST'});
await writeFile(out+'/public/research-config.json',JSON.stringify(cfg,null,2)+'\n',{flag:'wx'});await writeFile(out+'/public/task-catalog-dev.json',raw,{flag:'wx'});console.log('Local test campaign configured:',ctx.config);
