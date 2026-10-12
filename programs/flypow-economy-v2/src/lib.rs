//! Separate V2 global issuance and finite-budget settlement. Never upgrades or
//! interprets V1 receipts. Complete model replay remains off chain.
#![allow(unexpected_cfgs)]
use borsh::{BorshDeserialize,BorshSerialize};
use flypow_economic_core::{CAP,POOLS};
use solana_program::{account_info::AccountInfo,clock::Clock,entrypoint::ProgramResult,program::{invoke,invoke_signed},program_error::ProgramError,program_pack::Pack,pubkey::Pubkey,rent::Rent,sysvar::{Sysvar,instructions::load_instruction_at_checked}};
use solana_system_interface::{instruction as sys,program as system};
use spl_token::state::{Mint,Account as Token};
type Key=[u8;32];
mod mining_v3;
#[derive(BorshSerialize,BorshDeserialize,Clone)]pub struct Global {pub authority:Key,pub network:Key,pub policy:Key,pub settlers:[Key;3],pub mint:Key,pub issued:bool,pub available:[u64;3]}
#[derive(BorshSerialize,BorshDeserialize,Clone)]pub struct LegacyStage {pub id:Key,pub policy:Key,pub pool:u8,pub budget:u64,pub remaining:u64,pub deadline:i64,pub closed:bool}
#[derive(BorshSerialize,BorshDeserialize,Clone)]pub struct Stage {pub id:Key,pub policy:Key,pub pool:u8,pub budget:u64,pub remaining:u64,pub deadline:i64,pub closed:bool,pub unaccepted_count:u64,pub unaccepted_atoms:u64}
#[derive(BorshSerialize,BorshDeserialize,Clone)]pub struct Slot {pub economic_task:Key,pub unit:Key,pub kind:u8,pub stage:Key,pub pool:u8,pub beneficiary:Key,pub contributor:Key,pub amount:u64,pub paid:u64,pub accepted:bool,pub released:bool,pub evidence:Key}
#[derive(BorshSerialize,BorshDeserialize,Clone)]pub struct Payment {pub slot:Key,pub id:Key,pub amount:u64,pub beneficiary:Key}
#[derive(BorshSerialize,BorshDeserialize)]pub enum Instruction {
    Init{network:Key,policy:Key,settlers:[Key;3]},
    Issue,
    OpenStage{id:Key,pool:u8,budget:u64,deadline:i64},
    ReserveTask{economic_task:Key,unit:Key,kind:u8,beneficiary:Key,contributor:Key,amount:u64},
    Accept{evidence:Key,judgement:u8,ed25519_indices:[u16;2]},
    Claim{payment_id:Key,amount:u64},
    ReleaseUnaccepted,
    CloseStage,
    Adopt{network:Key,policy:Key,settlers:[Key;3]},
    MigrateStage, // tag 9: admin supplies the complete non-released slot inventory.
    MiningV3{data:Vec<u8>}, // tag 10: versioned funded discovery; old layouts stay intact.

}
#[cfg(not(feature="no-entrypoint"))]solana_program::entrypoint!(process_instruction);
fn bad()->ProgramError{ProgramError::InvalidArgument}
fn require(b:bool)->ProgramResult{if b{Ok(())}else{Err(bad())}}
fn load<T:BorshDeserialize>(a:&AccountInfo,p:&Pubkey,tag:&[u8;8])->Result<T,ProgramError>{require(a.owner==p)?;let d=a.try_borrow_data()?;let expected=if tag==b"FLYSV002"||tag==b"FLYSV003"{256}else if tag==b"FLYPV002"{160}else{512};require(d.len()==expected&&&d[..8]==tag)?;let mut remaining=&d[8..];let v=T::deserialize(&mut remaining).map_err(|_|bad())?;require(remaining.iter().all(|b|*b==0))?;Ok(v)}
fn save<T:BorshSerialize>(a:&AccountInfo,tag:&[u8;8],v:&T)->ProgramResult{let bytes=borsh::to_vec(v).map_err(|_|bad())?;let mut d=a.try_borrow_mut_data()?;require(d.len()>=bytes.len()+8)?;d.fill(0);d[..8].copy_from_slice(tag);d[8..bytes.len()+8].copy_from_slice(&bytes);Ok(())}
fn global(a:&AccountInfo,p:&Pubkey)->Result<Global,ProgramError>{require(*a.key==Pubkey::find_program_address(&[b"global-v2"],p).0)?;load(a,p,b"FLYGV002")}
fn stage(a:&AccountInfo,p:&Pubkey)->Result<Stage,ProgramError>{let s:Stage=load(a,p,b"FLYSV003")?;require(*a.key==Pubkey::find_program_address(&[b"stage-v2",&s.id],p).0)?;Ok(s)}
fn stage_for_claim(a:&AccountInfo,p:&Pubkey)->Result<Stage,ProgramError>{
    let d=a.try_borrow_data()?;let legacy=d.len()>=8&&&d[..8]==b"FLYSV002";drop(d);
    if !legacy{return stage(a,p);}
    let old:LegacyStage=load(a,p,b"FLYSV002")?;require(*a.key==Pubkey::find_program_address(&[b"stage-v2",&old.id],p).0)?;
    // Unknown counters are a sentinel only in this read-only claim view. Never
    // save them or permit a legacy reservation/close from this representation.
    Ok(Stage{id:old.id,policy:old.policy,pool:old.pool,budget:old.budget,remaining:old.remaining,deadline:old.deadline,closed:old.closed,unaccepted_count:u64::MAX,unaccepted_atoms:u64::MAX})
}
fn slot(a:&AccountInfo,p:&Pubkey)->Result<Slot,ProgramError>{let s:Slot=load(a,p,b"FLYRV002")?;require(*a.key==Pubkey::find_program_address(&[b"slot-v2",&s.economic_task,&[s.kind],&s.unit],p).0)?;Ok(s)}
fn create<'a>(p:&Pubkey,target:&AccountInfo<'a>,payer:&AccountInfo<'a>,system_a:&AccountInfo<'a>,size:usize,owner:&Pubkey,seeds:&[&[u8]])->ProgramResult {
    require(payer.is_signer&&*system_a.key==system::ID)?;
    let(key,bump)=Pubkey::find_program_address(seeds,p);
    require(key==*target.key&&target.data_is_empty()&&*target.owner==system::ID)?;
    let mut signed=seeds.to_vec();let b=[bump];signed.push(&b);
    let required=Rent::get()?.minimum_balance(size);
    let topup=required.saturating_sub(target.lamports());
    if topup>0 {invoke(&sys::transfer(payer.key,target.key,topup),&[payer.clone(),target.clone(),system_a.clone()])?;}
    invoke_signed(&sys::allocate(target.key,size as u64),&[target.clone(),system_a.clone()],&[&signed])?;
    invoke_signed(&sys::assign(target.key,owner),&[target.clone(),system_a.clone()],&[&signed])
}
// Only the current loader upgrade authority can initialize the singleton.
// The ProgramData PDA/owner/layout is verified, rather than accepting a caller
// supplied authority. Immutable deployments must initialize before revocation.
fn initializer(p:&Pubkey,payer:&AccountInfo,pd:&AccountInfo)->ProgramResult {
    let loader:Pubkey="BPFLoaderUpgradeab1e11111111111111111111111".parse().map_err(|_|bad())?;
    require(payer.is_signer&&pd.owner==&loader&&*pd.key==Pubkey::find_program_address(&[p.as_ref()],&loader).0)?;
    let d=pd.try_borrow_data()?;
    require(d.len()>=45&&d[..4]==3u32.to_le_bytes()&&d[12]==1&&d[13..45]==payer.key.to_bytes())
}
fn admin(g:&Global,a:&AccountInfo)->ProgramResult{require(a.is_signer&&a.key.to_bytes()==g.authority)}
fn token(a:&AccountInfo,mint:&Key,owner:&Key)->Result<Token,ProgramError>{require(*a.owner==spl_token::ID)?;let t=Token::unpack(&a.try_borrow_data()?)?;require(t.mint.to_bytes()==*mint&&t.owner.to_bytes()==*owner&&t.delegate.is_none()&&t.close_authority.is_none())?;Ok(t)}
fn mint(a:&AccountInfo,g:&Global)->Result<Mint,ProgramError>{require(a.key.to_bytes()==g.mint&&*a.owner==spl_token::ID)?;let m=Mint::unpack(&a.try_borrow_data()?)?;require(m.decimals==8&&m.freeze_authority.is_none())?;Ok(m)}
fn pool_key(p:&Pubkey,i:u8)->Pubkey{Pubkey::find_program_address(&[b"pool-v2",&[i]],p).0}
fn transfer<'a>(p:&Pubkey,ga:&AccountInfo<'a>,source:&AccountInfo<'a>,dest:&AccountInfo<'a>,ta:&AccountInfo<'a>,amount:u64)->ProgramResult{require(*ta.key==spl_token::ID)?;let bump=Pubkey::find_program_address(&[b"global-v2"],p).1;invoke_signed(&spl_token::instruction::transfer(&spl_token::ID,source.key,dest.key,ga.key,&[],amount)?,&[source.clone(),dest.clone(),ga.clone(),ta.clone()],&[&[b"global-v2",&[bump]]])}
/// Signature domain also binds the program, chain identity, policy and frozen
/// slot. Two keys can be one operator; that trust is disclosed separately.
pub fn acceptance_message(p:&Pubkey,g:&Global,s:&Slot,evidence:Key,judgement:u8)->Vec<u8>{let mut out=b"Flybrain/economy-v2/accept\0".to_vec();out.extend(borsh::to_vec(&(p.to_bytes(),g.network,g.policy,s.stage,s.economic_task,s.unit,s.kind,s.contributor,s.beneficiary,s.amount,evidence,judgement)).unwrap());solana_program::hash::hash(&out).to_bytes().to_vec()}
fn signature(sysvar:&AccountInfo,index:u16,message:&[u8])->Result<Key,ProgramError>{require(*sysvar.key==solana_program::sysvar::instructions::ID)?;let current=solana_program::sysvar::instructions::load_current_index_checked(sysvar)?;require(index<current)?;let ix=load_instruction_at_checked(index as usize,sysvar)?;require(ix.program_id==solana_program::ed25519_program::ID&&ix.accounts.is_empty())?;let d=&ix.data;require(d.len()>=16&&d[0]==1&&d[1]==0)?;let u=|o:usize|u16::from_le_bytes([d[o],d[o+1]]);require(u(4)==u16::MAX&&u(8)==u16::MAX&&u(14)==u16::MAX)?;let so=u(2) as usize;let ko=u(6) as usize;let mo=u(10) as usize;let len=u(12) as usize;require(so>=16&&ko>=16&&mo>=16&&len==message.len()&&so+64<=d.len()&&ko+32<=d.len()&&mo+len<=d.len()&&&d[mo..mo+len]==message)?;require((so+64<=ko||ko+32<=so)&&(so+64<=mo||mo+len<=so)&&(ko+32<=mo||mo+len<=ko))?;Ok(d[ko..ko+32].try_into().map_err(|_|bad())?)}
pub fn process_instruction(p:&Pubkey,a:&[AccountInfo],data:&[u8])->ProgramResult{
    let instruction=Instruction::try_from_slice(data).map_err(|_|bad())?;
    match instruction {
        Instruction::MiningV3{data}=>mining_v3::process(p,a,&data),
        Instruction::Init{network,policy,settlers}=>{
            require(a.len()==9&&network!=[0;32]&&policy!=[0;32]&&settlers.iter().all(|k|*k!=[0;32])&&settlers[0]!=settlers[1]&&settlers[0]!=settlers[2]&&settlers[1]!=settlers[2])?;
            let (ga,ma,payer,sy,ta)=(&a[0],&a[1],&a[5],&a[6],&a[7]);require(*ta.key==spl_token::ID)?;initializer(p,payer,&a[8])?;
            create(p,ga,payer,sy,512,p,&[b"global-v2"])?;create(p,ma,payer,sy,Mint::LEN,&spl_token::ID,&[b"mint-v2"])?;
            invoke(&spl_token::instruction::initialize_mint2(&spl_token::ID,ma.key,ga.key,None,8)?,&[ma.clone(),ta.clone()])?;
            for i in 0..3u8{let pa=&a[2+i as usize];create(p,pa,payer,sy,Token::LEN,&spl_token::ID,&[b"pool-v2",&[i]])?;invoke(&spl_token::instruction::initialize_account3(&spl_token::ID,pa.key,ma.key,ga.key)?,&[pa.clone(),ma.clone(),ta.clone()])?;}
            save(ga,b"FLYGV002",&Global{authority:payer.key.to_bytes(),network,policy,settlers,mint:ma.key.to_bytes(),issued:false,available:[0;3]})
        },
        Instruction::Adopt{network,policy,settlers}=>{
            require(a.len()==8&&network!=[0;32]&&policy!=[0;32]&&settlers.iter().all(|k|*k!=[0;32])&&settlers[0]!=settlers[1]&&settlers[0]!=settlers[2]&&settlers[1]!=settlers[2])?;
            initializer(p,&a[5],&a[7])?;let g=Global{authority:a[5].key.to_bytes(),network,policy,settlers,mint:a[1].key.to_bytes(),issued:true,available:POOLS};let m=mint(&a[1],&g)?;require(m.supply==CAP&&m.mint_authority.is_none())?;
            for i in 0..3{require(*a[2+i].key==pool_key(p,i as u8)&&token(&a[2+i],&g.mint,&a[0].key.to_bytes())?.amount==POOLS[i])?;}
            create(p,&a[0],&a[5],&a[6],512,p,&[b"global-v2"])?;save(&a[0],b"FLYGV002",&g)
        },
        Instruction::Issue=>{
            require(a.len()==7)?;let mut g=global(&a[0],p)?;admin(&g,&a[6])?;require(!g.issued&&*a[5].key==spl_token::ID)?;let m=mint(&a[1],&g)?;require(m.supply==0&&m.mint_authority==Some(*a[0].key).into())?;
            for i in 0..3{require(*a[2+i].key==pool_key(p,i as u8)&&token(&a[2+i],&g.mint,&a[0].key.to_bytes())?.amount==0)?;}
            let bump=Pubkey::find_program_address(&[b"global-v2"],p).1;
            for i in 0..3{invoke_signed(&spl_token::instruction::mint_to(&spl_token::ID,a[1].key,a[2+i].key,a[0].key,&[],POOLS[i])?,&[a[1].clone(),a[2+i].clone(),a[0].clone(),a[5].clone()],&[&[b"global-v2",&[bump]]])?;}
            invoke_signed(&spl_token::instruction::set_authority(&spl_token::ID,a[1].key,None,spl_token::instruction::AuthorityType::MintTokens,a[0].key,&[])?,&[a[1].clone(),a[0].clone(),a[5].clone()],&[&[b"global-v2",&[bump]]])?;
            g.issued=true;g.available=POOLS;save(&a[0],b"FLYGV002",&g)
        },
        Instruction::OpenStage{id,pool,budget,deadline}=>{
            require(a.len()==4&&pool<3&&budget>0&&id!=[0;32]&&deadline>Clock::get()?.unix_timestamp)?;let mut g=global(&a[0],p)?;admin(&g,&a[2])?;require(g.issued)?;g.available[pool as usize]=g.available[pool as usize].checked_sub(budget).ok_or(bad())?;
            create(p,&a[1],&a[2],&a[3],256,p,&[b"stage-v2",&id])?;save(&a[1],b"FLYSV003",&Stage{id,policy:g.policy,pool,budget,remaining:budget,deadline,closed:false,unaccepted_count:0,unaccepted_atoms:0})?;save(&a[0],b"FLYGV002",&g)
        },
        Instruction::ReserveTask{economic_task,unit,kind,beneficiary,contributor,amount}=>{
            require(a.len()==5&&economic_task!=[0;32]&&beneficiary!=[0;32]&&contributor!=[0;32]&&kind<5&&amount>0&&(kind!=0||unit==[0;32]))?;
            let g=global(&a[0],p)?;admin(&g,&a[3])?;let mut s=stage(&a[1],p)?;require(g.issued&&!s.closed&&s.policy==g.policy&&Clock::get()?.unix_timestamp<=s.deadline&&s.pool==if kind==2{1}else if kind==4{2}else{0})?;
            s.remaining=s.remaining.checked_sub(amount).ok_or(bad())?;s.unaccepted_count=s.unaccepted_count.checked_add(1).ok_or(bad())?;s.unaccepted_atoms=s.unaccepted_atoms.checked_add(amount).ok_or(bad())?;create(p,&a[2],&a[3],&a[4],512,p,&[b"slot-v2",&economic_task,&[kind],&unit])?;
            save(&a[2],b"FLYRV002",&Slot{economic_task,unit,kind,stage:a[1].key.to_bytes(),pool:s.pool,beneficiary,contributor,amount,paid:0,accepted:false,released:false,evidence:[0;32]})?;save(&a[1],b"FLYSV003",&s)
        },
        Instruction::Accept{evidence,judgement,ed25519_indices}=>{
            require(a.len()==4&&evidence!=[0;32]&&[1,2].contains(&judgement))?;let g=global(&a[0],p)?;let mut s=stage(&a[1],p)?;let mut r=slot(&a[2],p)?;require(r.stage==a[1].key.to_bytes()&&!r.released&&s.policy==g.policy&&(judgement==1||r.kind==2))?;
            let message=acceptance_message(p,&g,&r,evidence,judgement);let k1=signature(&a[3],ed25519_indices[0],&message)?;let k2=signature(&a[3],ed25519_indices[1],&message)?;require(k1!=k2&&g.settlers.contains(&k1)&&g.settlers.contains(&k2))?;
            if r.accepted{return require(r.evidence==evidence);}require(!s.closed&&Clock::get()?.unix_timestamp<=s.deadline)?;r.accepted=true;r.evidence=evidence;s.unaccepted_count=s.unaccepted_count.checked_sub(1).ok_or(bad())?;s.unaccepted_atoms=s.unaccepted_atoms.checked_sub(r.amount).ok_or(bad())?;save(&a[1],b"FLYSV003",&s)?;save(&a[2],b"FLYRV002",&r)
        },
        Instruction::Claim{payment_id,amount}=>{
            require(a.len()==9&&amount>0&&payment_id!=[0;32])?;let g=global(&a[0],p)?;let s=stage_for_claim(&a[1],p)?;let mut r=slot(&a[2],p)?;require(g.issued&&s.policy==g.policy&&r.stage==a[1].key.to_bytes()&&s.pool==r.pool&&r.accepted&&!r.released&&*a[3].key==pool_key(p,r.pool))?;
            token(&a[3],&g.mint,&a[0].key.to_bytes())?;token(&a[4],&g.mint,&r.beneficiary)?;
            require(*a[5].key==Pubkey::find_program_address(&[b"payment-v2",a[2].key.as_ref(),&payment_id],p).0)?;
            if !a[5].data_is_empty(){let paid:Payment=load(&a[5],p,b"FLYPV002")?;return require(paid.id==payment_id&&paid.slot==a[2].key.to_bytes()&&paid.amount==amount&&paid.beneficiary==r.beneficiary);}
            r.paid=r.paid.checked_add(amount).ok_or(bad())?;require(r.paid<=r.amount)?;create(p,&a[5],&a[6],&a[7],160,p,&[b"payment-v2",a[2].key.as_ref(),&payment_id])?;
            transfer(p,&a[0],&a[3],&a[4],&a[8],amount)?;save(&a[5],b"FLYPV002",&Payment{slot:a[2].key.to_bytes(),id:payment_id,amount,beneficiary:r.beneficiary})?;save(&a[2],b"FLYRV002",&r)
        },
        Instruction::ReleaseUnaccepted=>{
            require(a.len()==4)?;let g=global(&a[0],p)?;admin(&g,&a[3])?;let mut s=stage(&a[1],p)?;let mut r=slot(&a[2],p)?;require(r.stage==a[1].key.to_bytes()&&!r.accepted&&!r.released&&!s.closed)?;s.remaining=s.remaining.checked_add(r.amount).ok_or(bad())?;require(s.remaining<=s.budget)?;s.unaccepted_count=s.unaccepted_count.checked_sub(1).ok_or(bad())?;s.unaccepted_atoms=s.unaccepted_atoms.checked_sub(r.amount).ok_or(bad())?;r.released=true;save(&a[2],b"FLYRV002",&r)?;save(&a[1],b"FLYSV003",&s)
        },
        Instruction::MigrateStage=>{
            require(a.len()>=3)?;let g=global(&a[0],p)?;admin(&g,&a[2])?;
            let old:LegacyStage=load(&a[1],p,b"FLYSV002")?;
            require(old.policy==g.policy&&!old.closed&&old.pool<3&&*a[1].key==Pubkey::find_program_address(&[b"stage-v2",&old.id],p).0)?;
            let mut allocated=0u64;let mut count=0u64;let mut pending=0u64;
            for (i,account) in a[3..].iter().enumerate(){
                require(!a[3..3+i].iter().any(|x|x.key==account.key))?;
                let r=slot(account,p)?;require(r.stage==a[1].key.to_bytes()&&r.pool==old.pool&&!r.released&&r.amount>0)?;
                allocated=allocated.checked_add(r.amount).ok_or(bad())?;
                if !r.accepted{count=count.checked_add(1).ok_or(bad())?;pending=pending.checked_add(r.amount).ok_or(bad())?;}
            }
            // remaining + every non-released reservation must equal the frozen
            // budget. Omitting even one positive slot cannot pass this check.
            require(old.remaining.checked_add(allocated)==Some(old.budget))?;
            save(&a[1],b"FLYSV003",&Stage{id:old.id,policy:old.policy,pool:old.pool,budget:old.budget,remaining:old.remaining,deadline:old.deadline,closed:false,unaccepted_count:count,unaccepted_atoms:pending})
        },
        Instruction::CloseStage=>{
            require(a.len()==3)?;let mut g=global(&a[0],p)?;admin(&g,&a[2])?;let mut s=stage(&a[1],p)?;require(!s.closed&&s.policy==g.policy&&s.unaccepted_count==0&&s.unaccepted_atoms==0)?;g.available[s.pool as usize]=g.available[s.pool as usize].checked_add(s.remaining).ok_or(bad())?;require(g.available[s.pool as usize]<=POOLS[s.pool as usize])?;s.remaining=0;s.closed=true;save(&a[1],b"FLYSV003",&s)?;save(&a[0],b"FLYGV002",&g)
        }
    }
}
