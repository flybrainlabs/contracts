//! Isolated V5 account family. Scientific correctness is attested by the frozen
//! verifier quorum, not proved by the token settlement program.
use super::*;
use flybrainlabs_mining_v3::scientific_round_v5 as v5;
use v5::{Claim, Operation, Round};
const ROUND_TAG: &[u8;8]=b"FBSRV005";
const CLAIM_TAG: &[u8;8]=b"FBSCV005";
const MARKER_TAG: &[u8;8]=b"FBSMV005";
fn check<T>(r:v5::Result<T>)->Result<T,ProgramError>{r.map_err(|_|bad())}
fn read<T:BorshDeserialize>(a:&AccountInfo,p:&Pubkey,tag:&[u8;8],size:usize)->Result<T,ProgramError>{
    require(a.owner==p && a.data_len()==size)?;
    let d=a.try_borrow_data()?;require(&d[..8]==tag)?;
    let mut bytes=&d[8..];let v=T::deserialize(&mut bytes).map_err(|_|bad())?;
    require(bytes.iter().all(|b|*b==0))?;Ok(v)
}
#[inline(never)]
fn round(a:&AccountInfo,p:&Pubkey)->Result<Box<Round>,ProgramError>{
    let r:Round=read(a,p,ROUND_TAG,v5::ROUND_SIZE)?;
    require(*a.key==Pubkey::find_program_address(&[b"science-round-v5",&r.specification.id],p).0)?;
    check(r.accounting())?;Ok(Box::new(r))
}
fn claim(a:&AccountInfo,p:&Pubkey,ra:&AccountInfo)->Result<Claim,ProgramError>{
    let c:Claim=read(a,p,CLAIM_TAG,v5::CLAIM_SIZE)?;
    require(c.round==ra.key.to_bytes() && c.role<=2 && ((c.role==0 && c.assignment==255)||(c.role>0 && c.assignment<2))
        && *a.key==Pubkey::find_program_address(&[b"science-claim-v5",ra.key.as_ref(),&c.miner,&c.commitment],p).0)?;
    Ok(c)
}
#[inline(never)]
fn initialize(p:&Pubkey,a:&[AccountInfo],specification:v5::Specification,now:u64)->ProgramResult{
    require(a.len()==4)?;check(specification.validate(now))?;
    let mut g=global(&a[0],p)?;admin(&g,&a[2])?;require(g.issued)?;
    let budget=specification.reward_atoms.checked_mul(10).ok_or(bad())?;
    g.available[0]=g.available[0].checked_sub(budget).ok_or(bad())?;
    create(p,&a[1],&a[2],&a[3],v5::ROUND_SIZE,p,&[b"science-round-v5",&specification.id])?;
    let r=Box::new(Round{specification,genesis:g.network,policy:g.policy,mint:g.mint,verifiers:g.settlers,
        challenge:v5::ZERO,source_slot:0,queues:std::array::from_fn(|_|Vec::new()),cursors:[0;3],winners:[v5::ZERO;10],
        remaining:budget,accepted_unpaid:0,paid:0,refunded:0,closed:false});
    check(r.accounting())?;save(&a[1],ROUND_TAG,&r)?;save(&a[0],b"FLYGV002",&g)
}
#[inline(never)]
pub fn process(p:&Pubkey,a:&[AccountInfo],data:&[u8])->ProgramResult{
    let op=Operation::try_from_slice(data).map_err(|_|bad())?;
    let now=Clock::get()?.slot;
    match op {
        Operation::Initialize{specification}=>initialize(p,a,specification,now),
        Operation::Commit{commitment,beneficiary,role,assignment}=>{
            require(a.len()==4 && a[2].is_signer && commitment!=v5::ZERO && beneficiary!=v5::ZERO)?;
            let mut r=round(&a[0],p)?;
            let c=Claim{round:a[0].key.to_bytes(),miner:a[2].key.to_bytes(),beneficiary,commitment,role,assignment,
                input:v5::ZERO,manifest:v5::ZERO,result:v5::ZERO,computation:v5::ZERO,state:v5::COMMITTED,
                complete_votes:0,positive_votes:0,invalid_votes:0,reports:[v5::ZERO;3]};
            check(r.commit(a[1].key.to_bytes(),&c,now))?;
            create(p,&a[1],&a[2],&a[3],v5::CLAIM_SIZE,p,&[b"science-claim-v5",a[0].key.as_ref(),&c.miner,&commitment])?;
            save(&a[1],CLAIM_TAG,&c)?;save(&a[0],ROUND_TAG,&r)
        }
        Operation::Challenge=>{
            require(a.len()==2 && *a[1].key==solana_program::sysvar::slot_hashes::ID
                && *a[1].owner==solana_program::sysvar::ID)?;
            let mut r=round(&a[0],p)?;
            // SlotHashes contains the actual retained fork. Choose the FIRST
            // produced slot at/after the frozen target; callers cannot choose.
            // This is a chain beacon with proposer bias, not unbiased randomness.
            let d=a[1].try_borrow_data()?;require(d.len()>=8)?;
            let len=u64::from_le_bytes(d[..8].try_into().map_err(|_|bad())?) as usize;
            require(len<=512 && d.len()>=8+40*len)?;
            let mut source:Option<(u64,v5::Key)>=None;
            for row in d[8..8+40*len].chunks_exact(40){
                let slot=u64::from_le_bytes(row[..8].try_into().map_err(|_|bad())?);
                if slot>=r.specification.challenge_slot && slot<=now && source.is_none_or(|s|slot<s.0){
                    source=Some((slot,row[8..40].try_into().map_err(|_|bad())?));
                }
            }
            let (slot,hash)=source.ok_or(bad())?;check(r.challenge(slot,hash,now))?;
            save(&a[0],ROUND_TAG,&r)
        }
        Operation::Reveal{input,salt,manifest,result}=>{
            require(a.len()==3 && a[2].is_signer)?;
            let r=round(&a[0],p)?;let mut c=claim(&a[1],p,&a[0])?;
            require(a[2].key.to_bytes()==c.miner)?;
            check(c.reveal(p.to_bytes(),&r,input,salt,manifest,result,now))?;
            save(&a[1],CLAIM_TAG,&c)
        }
        Operation::Vote{complete,passed,report,ed25519_index}=>{
            require(a.len()==3)?;let r=round(&a[0],p)?;let mut c=claim(&a[1],p,&a[0])?;
            require(!r.closed && now<=r.specification.verify_end && r.challenge!=v5::ZERO)?;
            let msg=v5::vote_message(p.to_bytes(),&r,a[1].key.to_bytes(),&c,complete,passed,report);
            let signer=signature(&a[2],ed25519_index,&msg)?;
            let index=r.verifiers.iter().position(|k|*k==signer).ok_or(bad())?;
            check(c.vote(index,complete,passed,report))?;save(&a[1],CLAIM_TAG,&c)
        }
        Operation::Resolve=>{
            require(a.len()==5 && a[3].is_signer)?;
            let mut r=round(&a[0],p)?;let mut c=claim(&a[1],p,&a[0])?;
            if [v5::ACCEPTED,v5::PAID].contains(&c.state){return Ok(());}
            let expected=Pubkey::find_program_address(&[b"science-compute-v5",&c.computation],p).0;
            require(*a[2].key==expected)?;
            let duplicate=if a[2].owner==p {
                let x:(v5::Key,v5::Key)=read(&a[2],p,MARKER_TAG,v5::MARKER_SIZE)?;
                require(x.0==c.computation)?;true
            }else{require(*a[2].owner==system::ID && a[2].data_is_empty())?;false};
            if check(r.resolve(a[1].key.to_bytes(),&mut c,duplicate,now))? {
                create(p,&a[2],&a[3],&a[4],v5::MARKER_SIZE,p,&[b"science-compute-v5",&c.computation])?;
                save(&a[2],MARKER_TAG,&(c.computation,a[1].key.to_bytes()))?;
            }
            save(&a[1],CLAIM_TAG,&c)?;save(&a[0],ROUND_TAG,&r)
        }
        Operation::Pay=>{
            require(a.len()==6)?;let g=global(&a[0],p)?;let mut r=round(&a[1],p)?;let mut c=claim(&a[2],p,&a[1])?;
            require(g.issued && r.genesis==g.network && r.policy==g.policy && r.mint==g.mint
                && r.winners.contains(&a[2].key.to_bytes()) && *a[3].key==pool_key(p,0))?;
            token(&a[3],&g.mint,&a[0].key.to_bytes())?;token(&a[4],&g.mint,&c.beneficiary)?;
            let amount=check(r.pay(&mut c))?;
            if amount>0 {transfer(p,&a[0],&a[3],&a[4],&a[5],amount)?;
                save(&a[2],CLAIM_TAG,&c)?;save(&a[1],ROUND_TAG,&r)?;}
            Ok(())
        }
        Operation::Close=>{
            require(a.len()==2)?;let mut g=global(&a[0],p)?;let mut r=round(&a[1],p)?;
            require(r.genesis==g.network && r.policy==g.policy && r.mint==g.mint)?;
            let refund=check(r.close(now))?;g.available[0]=g.available[0].checked_add(refund).ok_or(bad())?;
            require(g.available[0]<=POOLS[0])?;save(&a[1],ROUND_TAG,&r)?;save(&a[0],b"FLYGV002",&g)
        }
    }
}
