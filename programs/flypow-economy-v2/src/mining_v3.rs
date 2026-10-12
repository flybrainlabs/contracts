//! Funded discovery extension. Result creation and token payment share one
//! atomic instruction; scientific replay is threshold-attested off chain.
use super::*;
use flybrainlabs_mining_v3::{
    Binding, FinancialIntent, Grant, RESULT_SIZE, ResultRecord, intent_bytes,
};

const CONFIG_SIZE: usize = 512;
const WALLET_SIZE: usize = 128;
const GRANT_SIZE: usize = 512;
const STAGE_SIZE: usize = 256;
const UNIT_SIZE: usize = 1536;
const INTENT_SIZE: usize = 1024;
const STAGING_SIZE: usize = 1280;

#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct Config {
    pub relayer: Key,
    pub close_authority: Key,
    pub rent_recipient: Key,
    pub retention_seconds: u64,
    pub max_grant_lifetime: u64,
    pub max_deposit: u64,
}
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct WalletGeneration {
    pub miner: Key,
    pub generation: u64,
}
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct MiningStage {
    pub id: Key,
    pub policy: Key,
    pub budget: u64,
    pub remaining: u64,
    pub pending_units: u64,
    pub pending_atoms: u64,
    pub accepted_unpaid: u64,
    pub paid: u64,
    pub deadline: i64,
    pub closed: bool,
}
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct Unit {
    pub economic_task: Key,
    pub unit: Key,
    pub stage: Key,
    pub amount: u64,
    pub accepted: bool,
    pub paid: bool,
    pub released: bool,
    pub intent: Key,
    pub staging: Key,
    pub beneficiary: Key,
    pub original_grant: Key,
    pub funding_grant: Key,
    pub funding_generation: u64,
    pub prepared_hash: Key,
    pub result_hash: Key,
    pub actual_rent: u64,
    pub accepted_at: u64,
    pub paid_slot: u64,
    pub paid_at: i64,
    pub closed: bool,
    pub closed_at: i64,
    pub close_votes: u8,
}
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct Intent {
    pub value: FinancialIntent,
    pub hash: Key,
    pub original_grant: Key,
}
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct Staging {
    pub unit: Key,
    pub intent: Key,
    pub expected_hash: Key,
    pub written: u16,
    pub sealed: bool,
    pub votes: u8,
    pub bytes: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub enum Operation {
    PrepareAdoptionVaults,
    Configure {
        relayer: Key,
        close_authority: Key,
        rent_recipient: Key,
        retention_seconds: u64,
        max_grant_lifetime: u64,
        max_deposit: u64,
    },
    OpenStage {
        id: Key,
        budget: u64,
        deadline: i64,
    },
    ReserveUnit {
        economic_task: Key,
        amount: u64,
    },
    CreateGrant {
        id: Key,
        generation: u64,
        session: Key,
        deposit: u64,
        max_spend: u64,
        max_result_rent: u64,
        max_results: u64,
        expires_at: i64,
    },
    RenewGrant {
        generation: u64,
        session: Key,
        deposit: u64,
        max_spend: u64,
        max_result_rent: u64,
        max_results: u64,
        expires_at: i64,
    },
    RevokeGrant,
    WithdrawGrant {
        amount: u64,
    },
    CloseGrant,
    RegisterIntent {
        intent: FinancialIntent,
        ed25519_index: u16,
    },
    CreateStaging {
        expected_hash: Key,
    },
    WriteStaging {
        offset: u16,
        bytes: Vec<u8>,
    },
    SealStaging,
    VoteWinner {
        ed25519_index: u16,
    },
    RebindFunding {
        generation: u64,
    },
    Settle,
    VoteClosure {
        ed25519_index: u16,
    },
    CloseResult,
    ReleaseUnit,
    CloseStage,
}

fn load_v3<T: BorshDeserialize>(
    a: &AccountInfo,
    p: &Pubkey,
    tag: &[u8; 8],
    size: usize,
) -> Result<T, ProgramError> {
    require(a.owner == p && a.data_len() == size)?;
    let d = a.try_borrow_data()?;
    require(&d[..8] == tag)?;
    let mut rest = &d[8..];
    let result = T::deserialize(&mut rest).map_err(|_| bad())?;
    require(rest.iter().all(|b| *b == 0))?;
    Ok(result)
}
fn configuration(a: &AccountInfo, p: &Pubkey) -> Result<Config, ProgramError> {
    require(*a.key == Pubkey::find_program_address(&[b"mining-config-v3"], p).0)?;
    load_v3(a, p, b"FBMCV003", CONFIG_SIZE)
}
fn stage(a: &AccountInfo, p: &Pubkey) -> Result<MiningStage, ProgramError> {
    let s: MiningStage = load_v3(a, p, b"FBMSV003", STAGE_SIZE)?;
    require(*a.key == Pubkey::find_program_address(&[b"mining-stage-v3", &s.id], p).0)?;
    Ok(s)
}
fn unit(a: &AccountInfo, p: &Pubkey) -> Result<Unit, ProgramError> {
    let u: Unit = load_v3(a, p, b"FBMUV003", UNIT_SIZE)?;
    require(
        *a.key == Pubkey::find_program_address(&[b"slot-v2", &u.economic_task, &[0], &u.unit], p).0
            && u.unit == [0; 32],
    )?;
    Ok(u)
}
fn grant(a: &AccountInfo, p: &Pubkey) -> Result<Grant, ProgramError> {
    let g: Grant = load_v3(a, p, b"FBMGV003", GRANT_SIZE)?;
    g.validate().map_err(|_| bad())?;
    require(*a.key == Pubkey::find_program_address(&[b"grant-v3", &g.miner, &g.id], p).0)?;
    Ok(g)
}
fn intent(a: &AccountInfo, p: &Pubkey) -> Result<Intent, ProgramError> {
    let i: Intent = load_v3(a, p, b"FBMIV003", INTENT_SIZE)?;
    require(
        *a.key
            == Pubkey::find_program_address(
                &[
                    b"intent-v3",
                    &i.value.unit_account,
                    &i.value.miner,
                    &i.value.commitment,
                ],
                p,
            )
            .0,
    )?;
    require(i.hash == solana_program::hash::hash(&intent_bytes(&i.value)).to_bytes())?;
    Ok(i)
}
fn staging(a: &AccountInfo, p: &Pubkey) -> Result<Staging, ProgramError> {
    let s: Staging = load_v3(a, p, b"FBMBV003", STAGING_SIZE)?;
    require(
        *a.key == Pubkey::find_program_address(&[b"staging-v3", &s.unit, &s.intent], p).0
            && s.bytes.len() == RESULT_SIZE
            && s.written as usize <= RESULT_SIZE,
    )?;
    Ok(s)
}
fn relayer(c: &Config, a: &AccountInfo) -> ProgramResult {
    require(a.is_signer && a.key.to_bytes() == c.relayer)
}
fn distinct(accounts: &[&AccountInfo]) -> ProgramResult {
    for (i, a) in accounts.iter().enumerate() {
        require(!accounts[..i].iter().any(|b| a.key == b.key))?;
    }
    Ok(())
}
fn writable(accounts: &[&AccountInfo]) -> ProgramResult {
    require(accounts.iter().all(|a| a.is_writable))
}
fn move_lamports(source: &AccountInfo, destination: &AccountInfo, amount: u64) -> ProgramResult {
    require(source.is_writable && destination.is_writable && source.key != destination.key)?;
    let next = source.lamports().checked_sub(amount).ok_or(bad())?;
    let dest = destination.lamports().checked_add(amount).ok_or(bad())?;
    **source.try_borrow_mut_lamports()? = next;
    **destination.try_borrow_mut_lamports()? = dest;
    Ok(())
}
fn issued_mint(a: &AccountInfo, g: &Global) -> ProgramResult {
    let m = mint(a, g)?;
    require(g.issued && m.mint_authority.is_none() && m.supply <= CAP)
}
fn wallet(a: &AccountInfo, p: &Pubkey, miner: &Key) -> Result<WalletGeneration, ProgramError> {
    require(*a.key == Pubkey::find_program_address(&[b"wallet-generation-v3", miner], p).0)?;
    let w: WalletGeneration = load_v3(a, p, b"FBMWV003", WALLET_SIZE)?;
    require(w.miner == *miner)?;
    Ok(w)
}
fn limits(
    c: &Config,
    now: i64,
    session: Key,
    deposit: u64,
    max_spend: u64,
    max_rent: u64,
    max_results: u64,
    expires: i64,
) -> ProgramResult {
    require(
        session != [0; 32]
            && deposit <= c.max_deposit
            && max_spend > 0
            && max_rent > 0
            && max_rent <= max_spend
            && max_results > 0
            && max_results <= 64
            && expires > now
            && expires.checked_sub(now).ok_or(bad())? as u64 <= c.max_grant_lifetime,
    )
}
fn close_account(account: &AccountInfo, destination: &AccountInfo) -> ProgramResult {
    move_lamports(account, destination, account.lamports())?;
    account.resize(0)?;
    account.assign(&system::ID);
    Ok(())
}
fn create_v3<'a>(
    p: &Pubkey,
    target: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    sy: &AccountInfo<'a>,
    size: usize,
    seeds: &[&[u8]],
) -> ProgramResult {
    writable(&[target, payer])?;
    distinct(&[target, payer])?;
    create(p, target, payer, sy, size, p, seeds)
}
fn grant_custody(ga: &AccountInfo, g: &Grant) -> ProgramResult {
    require(
        ga.lamports()
            >= g.management_rent
                .checked_add(g.available().map_err(|_| bad())?)
                .ok_or(bad())?,
    )
}
fn result_key(p: &Pubkey, u: &AccountInfo) -> Pubkey {
    Pubkey::find_program_address(&[b"result-v3", u.key.as_ref()], p).0
}
fn ata_key(mint: &Pubkey, owner: &Pubkey) -> Result<Pubkey, ProgramError> {
    let ata: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        .parse()
        .map_err(|_| bad())?;
    Ok(Pubkey::find_program_address(
        &[owner.as_ref(), spl_token::ID.as_ref(), mint.as_ref()],
        &ata,
    )
    .0)
}
pub fn closure_message(p: &Pubkey, g: &Global, ua: &AccountInfo, u: &Unit) -> Key {
    let mut bytes = b"flybrainlabs/mining-v3/finalized-archive-close\0".to_vec();
    bytes.extend(
        borsh::to_vec(&(
            p.to_bytes(),
            g.network,
            g.policy,
            ua.key.to_bytes(),
            u.result_hash,
            u.prepared_hash,
            u.paid_slot,
            u.paid_at,
        ))
        .unwrap(),
    );
    solana_program::hash::hash(&bytes).to_bytes()
}
pub fn certificate_message(
    p: &Pubkey,
    g: &Global,
    u: &AccountInfo,
    i: &Intent,
    s: &Staging,
) -> Key {
    let mut bytes = b"flybrainlabs/mining-v3/winner\0".to_vec();
    bytes.extend(
        borsh::to_vec(&(
            p.to_bytes(),
            g.network,
            g.policy,
            u.key.to_bytes(),
            i.hash,
            s.expected_hash,
        ))
        .unwrap(),
    );
    solana_program::hash::hash(&bytes).to_bytes()
}
fn settler_policy(p: &Pubkey, g: &Global) -> Key {
    let mut bytes = b"flybrainlabs/mining-v3/settler-policy\0".to_vec();
    bytes.extend(borsh::to_vec(&(p.to_bytes(), g.network, g.policy, g.settlers, 2u8)).unwrap());
    solana_program::hash::hash(&bytes).to_bytes()
}
fn prepared(
    record: &ResultRecord,
    p: &Pubkey,
    g: &Global,
    ua: &AccountInfo,
    u: &Unit,
    ia: &AccountInfo,
    i: &Intent,
) -> ProgramResult {
    record.validate_prepared().map_err(|_| bad())?;
    for (field, key) in [
        (Binding::Genesis, g.network),
        (Binding::Program, p.to_bytes()),
        (Binding::Policy, g.policy),
        (Binding::Stage, u.stage),
        (Binding::EconomicTask, u.economic_task),
        (Binding::Unit, u.unit),
        (Binding::Round, i.value.round),
        (Binding::Candidate, i.value.candidate),
        (Binding::GoalResult, i.value.goal_result),
        (Binding::Manifest, i.value.manifest),
        (Binding::Miner, i.value.miner),
        (Binding::Beneficiary, i.value.beneficiary),
        (Binding::OriginalGrantId, i.value.grant),
        (Binding::OriginalSession, i.value.session),
        (Binding::PermanentUnit, ua.key.to_bytes()),
        (Binding::OriginalGrant, i.original_grant),
    ] {
        require(*record.get(field) == key)?;
    }
    require(
        record.original_generation == i.value.grant_generation
            && record.reward_atoms == u.amount
            && i.value.unit_account == ua.key.to_bytes()
            && ia.key.to_bytes() != [0; 32]
            && *record.get(Binding::SettlerPolicy) == settler_policy(p, g),
    )
}

#[inline(never)]
fn global_heap(a: &AccountInfo, p: &Pubkey) -> Result<Box<Global>, ProgramError> {
    Ok(Box::new(global(a, p)?))
}
#[inline(never)]
fn stage_heap(a: &AccountInfo, p: &Pubkey) -> Result<Box<MiningStage>, ProgramError> {
    Ok(Box::new(stage(a, p)?))
}
#[inline(never)]
fn unit_heap(a: &AccountInfo, p: &Pubkey) -> Result<Box<Unit>, ProgramError> {
    Ok(Box::new(unit(a, p)?))
}
#[inline(never)]
fn grant_heap(a: &AccountInfo, p: &Pubkey) -> Result<Box<Grant>, ProgramError> {
    Ok(Box::new(grant(a, p)?))
}
#[inline(never)]
fn intent_heap(a: &AccountInfo, p: &Pubkey) -> Result<Box<Intent>, ProgramError> {
    Ok(Box::new(intent(a, p)?))
}
#[inline(never)]
fn staging_heap(a: &AccountInfo, p: &Pubkey) -> Result<Box<Staging>, ProgramError> {
    Ok(Box::new(staging(a, p)?))
}
#[inline(never)]
fn result_heap(bytes: &[u8]) -> Result<Box<ResultRecord>, ProgramError> {
    Ok(Box::new(ResultRecord::decode(bytes).map_err(|_| bad())?))
}

#[inline(never)]
fn write_result(
    account: &AccountInfo,
    prepared_bytes: &[u8],
    rent: u64,
    accepted: u64,
    clock: &Clock,
) -> Result<Key, ProgramError> {
    // The sealed canonical summary has already passed prepared(). Only these
    // four runtime-owned fields change inside the atomic settlement boundary.
    require(
        prepared_bytes.len() == RESULT_SIZE && prepared_bytes[848..880].iter().all(|b| *b == 0),
    )?;
    let mut bytes = account.try_borrow_mut_data()?;
    require(bytes.len() == RESULT_SIZE)?;
    bytes.copy_from_slice(prepared_bytes);
    for (offset, value) in [
        (848, rent),
        (856, accepted),
        (864, clock.slot),
        (872, clock.unix_timestamp as u64),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    Ok(solana_program::hash::hash(&bytes).to_bytes())
}

pub fn process(p: &Pubkey, a: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let operation = Operation::try_from_slice(data).map_err(|_| bad())?;
    match operation {
        Operation::PrepareAdoptionVaults => handle_prepareadoptionvaults(p, a),
        Operation::Configure {
            relayer,
            close_authority,
            rent_recipient,
            retention_seconds,
            max_grant_lifetime,
            max_deposit,
        } => handle_configure(
            p,
            a,
            relayer,
            close_authority,
            rent_recipient,
            retention_seconds,
            max_grant_lifetime,
            max_deposit,
        ),
        Operation::OpenStage {
            id,
            budget,
            deadline,
        } => handle_openstage(p, a, id, budget, deadline),
        Operation::ReserveUnit {
            economic_task,
            amount,
        } => handle_reserveunit(p, a, economic_task, amount),
        Operation::CreateGrant {
            id,
            generation,
            session,
            deposit,
            max_spend,
            max_result_rent,
            max_results,
            expires_at,
        } => handle_creategrant(
            p,
            a,
            id,
            generation,
            session,
            deposit,
            max_spend,
            max_result_rent,
            max_results,
            expires_at,
        ),
        Operation::RenewGrant {
            generation,
            session,
            deposit,
            max_spend,
            max_result_rent,
            max_results,
            expires_at,
        } => handle_renewgrant(
            p,
            a,
            generation,
            session,
            deposit,
            max_spend,
            max_result_rent,
            max_results,
            expires_at,
        ),
        Operation::RevokeGrant => handle_revokegrant(p, a),
        Operation::WithdrawGrant { amount } => handle_withdrawgrant(p, a, amount),
        Operation::CloseGrant => handle_closegrant(p, a),
        Operation::RegisterIntent {
            intent: request,
            ed25519_index,
        } => handle_registerintent(p, a, request, ed25519_index),
        Operation::CreateStaging { expected_hash } => handle_createstaging(p, a, expected_hash),
        Operation::WriteStaging { offset, bytes } => handle_writestaging(p, a, offset, bytes),
        Operation::SealStaging => handle_sealstaging(p, a),
        Operation::VoteWinner { ed25519_index } => handle_votewinner(p, a, ed25519_index),
        Operation::RebindFunding { generation } => handle_rebindfunding(p, a, generation),
        Operation::Settle => handle_settle(p, a),
        Operation::VoteClosure { ed25519_index } => handle_voteclosure(p, a, ed25519_index),
        Operation::CloseResult => handle_closeresult(p, a),
        Operation::ReleaseUnit => handle_releaseunit(p, a),
        Operation::CloseStage => handle_closestage(p, a),
    }
}

#[inline(never)]
fn handle_prepareadoptionvaults(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 8 && *a[6].key == spl_token::ID)?;
    initializer(p, &a[4], &a[7])?;
    require(*a[0].owner == spl_token::ID)?;
    let m = Mint::unpack(&a[0].try_borrow_data()?)?;
    require(
        m.supply == CAP
            && m.decimals == 8
            && m.mint_authority.is_none()
            && m.freeze_authority.is_none(),
    )?;
    let global_key = Pubkey::find_program_address(&[b"global-v2"], p).0;
    distinct(&[&a[0], &a[1], &a[2], &a[3], &a[4]])?;
    for i in 0..3u8 {
        let target = &a[1 + i as usize];
        require(*target.key == pool_key(p, i))?;
        if !target.data_is_empty() {
            token(target, &a[0].key.to_bytes(), &global_key.to_bytes())?;
            continue;
        }
        writable(&[target, &a[4]])?;
        create(
            p,
            target,
            &a[4],
            &a[5],
            Token::LEN,
            &spl_token::ID,
            &[b"pool-v2", &[i]],
        )?;
        invoke(
            &spl_token::instruction::initialize_account3(
                &spl_token::ID,
                target.key,
                a[0].key,
                &global_key,
            )?,
            &[target.clone(), a[0].clone(), a[6].clone()],
        )?;
    }
    Ok(())
}

#[inline(never)]
fn handle_configure(
    p: &Pubkey,
    a: &[AccountInfo],
    relayer: Key,
    close_authority: Key,
    rent_recipient: Key,
    retention_seconds: u64,
    max_grant_lifetime: u64,
    max_deposit: u64,
) -> ProgramResult {
    require(a.len() == 4)?;
    let g = global_heap(&a[0], p)?;
    admin(&g, &a[2])?;
    require(
        g.issued
            && relayer != [0; 32]
            && relayer != g.authority
            && close_authority == g.authority
            && rent_recipient == g.authority
            && retention_seconds >= 60
            && retention_seconds <= 31_536_000
            && max_grant_lifetime > 0
            && max_grant_lifetime <= 86_400
            && max_deposit > 0
            && max_deposit <= 1_000_000_000,
    )?;
    create_v3(p, &a[1], &a[2], &a[3], CONFIG_SIZE, &[b"mining-config-v3"])?;
    save(
        &a[1],
        b"FBMCV003",
        &Config {
            relayer,
            close_authority,
            rent_recipient,
            retention_seconds,
            max_grant_lifetime,
            max_deposit,
        },
    )
}

#[inline(never)]
fn handle_openstage(
    p: &Pubkey,
    a: &[AccountInfo],
    id: Key,
    budget: u64,
    deadline: i64,
) -> ProgramResult {
    require(
        a.len() == 5 && id != [0; 32] && budget > 0 && deadline > Clock::get()?.unix_timestamp,
    )?;
    let mut g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    admin(&g, &a[3])?;
    require(g.issued)?;
    g.available[0] = g.available[0].checked_sub(budget).ok_or(bad())?;
    create_v3(
        p,
        &a[2],
        &a[3],
        &a[4],
        STAGE_SIZE,
        &[b"mining-stage-v3", &id],
    )?;
    save(
        &a[2],
        b"FBMSV003",
        &MiningStage {
            id,
            policy: g.policy,
            budget,
            remaining: budget,
            pending_units: 0,
            pending_atoms: 0,
            accepted_unpaid: 0,
            paid: 0,
            deadline,
            closed: false,
        },
    )?;
    save(&a[0], b"FLYGV002", &g)
}

#[inline(never)]
fn handle_reserveunit(
    p: &Pubkey,
    a: &[AccountInfo],
    economic_task: Key,
    amount: u64,
) -> ProgramResult {
    require(a.len() == 6 && economic_task != [0; 32] && amount > 0)?;
    let g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    admin(&g, &a[4])?;
    let mut s = stage_heap(&a[2], p)?;
    require(!s.closed && Clock::get()?.unix_timestamp <= s.deadline && s.policy == g.policy)?;
    s.remaining = s.remaining.checked_sub(amount).ok_or(bad())?;
    s.pending_units = s.pending_units.checked_add(1).ok_or(bad())?;
    s.pending_atoms = s.pending_atoms.checked_add(amount).ok_or(bad())?;
    create_v3(
        p,
        &a[3],
        &a[4],
        &a[5],
        UNIT_SIZE,
        &[b"slot-v2", &economic_task, &[0], &[0; 32]],
    )?;
    save(
        &a[3],
        b"FBMUV003",
        &Unit {
            economic_task,
            unit: [0; 32],
            stage: a[2].key.to_bytes(),
            amount,
            accepted: false,
            paid: false,
            released: false,
            intent: [0; 32],
            staging: [0; 32],
            beneficiary: [0; 32],
            original_grant: [0; 32],
            funding_grant: [0; 32],
            funding_generation: 0,
            prepared_hash: [0; 32],
            result_hash: [0; 32],
            actual_rent: 0,
            accepted_at: 0,
            paid_slot: 0,
            paid_at: 0,
            closed: false,
            closed_at: 0,
            close_votes: 0,
        },
    )?;
    save(&a[2], b"FBMSV003", &s)
}

#[inline(never)]
fn handle_creategrant(
    p: &Pubkey,
    a: &[AccountInfo],
    id: Key,
    generation: u64,
    session: Key,
    deposit: u64,
    max_spend: u64,
    max_result_rent: u64,
    max_results: u64,
    expires_at: i64,
) -> ProgramResult {
    require(a.len() == 7 && id != [0; 32] && a[4].is_signer && deposit > 0)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[5])?;
    let miner = a[4].key.to_bytes();
    let now = Clock::get()?.unix_timestamp;
    limits(
        &c,
        now,
        session,
        deposit,
        max_spend,
        max_result_rent,
        max_results,
        expires_at,
    )?;
    require(max_spend <= deposit)?;
    let mut w = if a[2].data_is_empty() {
        create_v3(
            p,
            &a[2],
            &a[5],
            &a[6],
            WALLET_SIZE,
            &[b"wallet-generation-v3", &miner],
        )?;
        WalletGeneration {
            miner,
            generation: 0,
        }
    } else {
        wallet(&a[2], p, &miner)?
    };
    require(w.generation.checked_add(1) == Some(generation))?;
    w.generation = generation;
    distinct(&[&a[2], &a[3], &a[4], &a[5]])?;
    create_v3(
        p,
        &a[3],
        &a[5],
        &a[6],
        GRANT_SIZE,
        &[b"grant-v3", &miner, &id],
    )?;
    let management_rent = Rent::get()?.minimum_balance(GRANT_SIZE);
    invoke(
        &sys::transfer(a[4].key, a[3].key, deposit),
        &[a[4].clone(), a[3].clone(), a[6].clone()],
    )?;
    let grant = Grant {
        id,
        miner,
        beneficiary: miner,
        session,
        policy: g.policy,
        generation,
        deposited: deposit,
        spent: 0,
        withdrawn: 0,
        successes: 0,
        max_spend,
        max_result_rent,
        max_results,
        expires_at,
        revoked: false,
        management_rent,
    };
    grant.validate().map_err(|_| bad())?;
    grant_custody(&a[3], &grant)?;
    save(&a[3], b"FBMGV003", &grant)?;
    save(&a[2], b"FBMWV003", &w)
}

#[inline(never)]
fn handle_renewgrant(
    p: &Pubkey,
    a: &[AccountInfo],
    generation: u64,
    session: Key,
    deposit: u64,
    max_spend: u64,
    max_result_rent: u64,
    max_results: u64,
    expires_at: i64,
) -> ProgramResult {
    require(a.len() == 7)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[5])?;
    let mut current = grant_heap(&a[3], p)?;
    require(a[4].is_signer && a[4].key.to_bytes() == current.miner && current.policy == g.policy)?;
    limits(
        &c,
        Clock::get()?.unix_timestamp,
        session,
        deposit,
        max_spend,
        max_result_rent,
        max_results,
        expires_at,
    )?;
    let mut w = wallet(&a[2], p, &current.miner)?;
    require(
        w.generation.checked_add(1) == Some(generation)
            && max_spend >= current.spent
            && max_results >= current.successes,
    )?;
    if deposit > 0 {
        invoke(
            &sys::transfer(a[4].key, a[3].key, deposit),
            &[a[4].clone(), a[3].clone(), a[6].clone()],
        )?;
    }
    current.deposited = current.deposited.checked_add(deposit).ok_or(bad())?;
    require(
        max_spend
            <= current
                .deposited
                .checked_sub(current.withdrawn)
                .ok_or(bad())?,
    )?;
    current.generation = generation;
    current.session = session;
    current.max_spend = max_spend;
    current.max_result_rent = max_result_rent;
    current.max_results = max_results;
    current.expires_at = expires_at;
    current.revoked = false;
    w.generation = generation;
    current.validate().map_err(|_| bad())?;
    grant_custody(&a[3], &current)?;
    save(&a[3], b"FBMGV003", &current)?;
    save(&a[2], b"FBMWV003", &w)
}

#[inline(never)]
fn handle_revokegrant(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 3)?;
    let g = global_heap(&a[0], p)?;
    let mut current = grant_heap(&a[1], p)?;
    require(current.policy == g.policy && a[2].is_signer && a[2].key.to_bytes() == current.miner)?;
    current.revoked = true;
    save(&a[1], b"FBMGV003", &current)
}

#[inline(never)]
fn handle_withdrawgrant(p: &Pubkey, a: &[AccountInfo], amount: u64) -> ProgramResult {
    require(a.len() == 3 && amount > 0)?;
    let g = global_heap(&a[0], p)?;
    let mut current = grant_heap(&a[1], p)?;
    require(current.policy == g.policy && a[2].is_signer && a[2].key.to_bytes() == current.miner)?;
    grant_custody(&a[1], &current)?;
    current.withdraw(amount).map_err(|_| bad())?;
    move_lamports(&a[1], &a[2], amount)?;
    grant_custody(&a[1], &current)?;
    save(&a[1], b"FBMGV003", &current)
}

#[inline(never)]
fn handle_closegrant(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 5)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    let current = grant_heap(&a[2], p)?;
    require(
        current.policy == g.policy
            && a[3].is_signer
            && a[3].key.to_bytes() == current.miner
            && a[4].key.to_bytes() == c.rent_recipient,
    )?;
    grant_custody(&a[2], &current)?;
    let refund = a[2]
        .lamports()
        .checked_sub(current.management_rent)
        .ok_or(bad())?;
    move_lamports(&a[2], &a[3], refund)?;
    close_account(&a[2], &a[4])
}

#[inline(never)]
fn handle_registerintent(
    p: &Pubkey,
    a: &[AccountInfo],
    request: FinancialIntent,
    ed25519_index: u16,
) -> ProgramResult {
    require(a.len() == 8)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[5])?;
    let u = unit_heap(&a[2], p)?;
    let message = solana_program::hash::hash(&intent_bytes(&request)).to_bytes();
    require(
        request.genesis == g.network
            && request.program == p.to_bytes()
            && request.policy == g.policy
            && request.economic_task == u.economic_task
            && request.unit == u.unit
            && request.unit_account == a[2].key.to_bytes()
            && request.reward_atoms == u.amount
            && request.round != [0; 32]
            && request.candidate != [0; 32]
            && request.commitment != [0; 32]
            && request.goal_result != [0; 32]
            && request.manifest != [0; 32]
            && request.scientific_identity != [0; 32],
    )?;
    if !a[4].data_is_empty() {
        let old = intent_heap(&a[4], p)?;
        return require(old.hash == message && old.original_grant == a[3].key.to_bytes());
    }
    require(!u.paid && !u.released)?;
    let current = grant_heap(&a[3], p)?;
    require(
        current.policy == g.policy
            && request.grant == current.id
            && request.miner == current.miner
            && request.beneficiary == current.beneficiary
            && request.session == current.session
            && request.grant_generation == current.generation
            && request.max_result_rent <= current.max_result_rent
            && request.max_result_rent > 0,
    )?;
    current
        .check(Clock::get()?.unix_timestamp, request.grant_generation, 0)
        .map_err(|_| bad())?;
    require(signature(&a[7], ed25519_index, &message)? == current.session)?;
    create_v3(
        p,
        &a[4],
        &a[5],
        &a[6],
        INTENT_SIZE,
        &[
            b"intent-v3",
            a[2].key.as_ref(),
            &request.miner,
            &request.commitment,
        ],
    )?;
    save(
        &a[4],
        b"FBMIV003",
        &Intent {
            value: request,
            hash: message,
            original_grant: a[3].key.to_bytes(),
        },
    )
}

#[inline(never)]
fn handle_createstaging(p: &Pubkey, a: &[AccountInfo], expected_hash: Key) -> ProgramResult {
    require(a.len() == 7 && expected_hash != [0; 32])?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[5])?;
    let u = unit_heap(&a[2], p)?;
    let i = intent_heap(&a[3], p)?;
    require(
        !u.released && i.value.unit_account == a[2].key.to_bytes() && i.value.policy == g.policy,
    )?;
    if !a[4].data_is_empty() {
        let old = staging_heap(&a[4], p)?;
        return require(
            old.expected_hash == expected_hash
                && old.unit == a[2].key.to_bytes()
                && old.intent == a[3].key.to_bytes(),
        );
    }
    require(!u.paid)?;
    create_v3(
        p,
        &a[4],
        &a[5],
        &a[6],
        STAGING_SIZE,
        &[b"staging-v3", a[2].key.as_ref(), a[3].key.as_ref()],
    )?;
    save(
        &a[4],
        b"FBMBV003",
        &Staging {
            unit: a[2].key.to_bytes(),
            intent: a[3].key.to_bytes(),
            expected_hash,
            written: 0,
            sealed: false,
            votes: 0,
            bytes: vec![0; RESULT_SIZE],
        },
    )
}

#[inline(never)]
fn handle_writestaging(
    p: &Pubkey,
    a: &[AccountInfo],
    offset: u16,
    bytes: Vec<u8>,
) -> ProgramResult {
    require(a.len() == 4 && !bytes.is_empty() && bytes.len() <= 512)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[3])?;
    require(g.issued)?;
    let mut s = staging_heap(&a[2], p)?;
    let end = (offset as usize).checked_add(bytes.len()).ok_or(bad())?;
    require(end <= RESULT_SIZE)?;
    if offset < s.written {
        return require(end <= s.written as usize && s.bytes[offset as usize..end] == bytes);
    }
    require(!s.sealed && s.votes == 0 && offset == s.written)?;
    s.bytes[offset as usize..end].copy_from_slice(&bytes);
    s.written = end as u16;
    save(&a[2], b"FBMBV003", &s)
}

#[inline(never)]
fn handle_sealstaging(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 6)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[5])?;
    let u = unit_heap(&a[2], p)?;
    let i = intent_heap(&a[3], p)?;
    let mut s = staging_heap(&a[4], p)?;
    require(
        s.unit == a[2].key.to_bytes()
            && s.intent == a[3].key.to_bytes()
            && s.written as usize == RESULT_SIZE
            && solana_program::hash::hash(&s.bytes).to_bytes() == s.expected_hash,
    )?;
    let record = result_heap(&s.bytes)?;
    prepared(&record, p, &g, &a[2], &u, &a[3], &i)?;
    s.sealed = true;
    save(&a[4], b"FBMBV003", &s)
}

#[inline(never)]
fn handle_votewinner(p: &Pubkey, a: &[AccountInfo], ed25519_index: u16) -> ProgramResult {
    require(a.len() == 7)?;
    let g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    let mut stage = stage_heap(&a[2], p)?;
    let mut u = unit_heap(&a[3], p)?;
    let i = intent_heap(&a[4], p)?;
    let mut s = staging_heap(&a[5], p)?;
    require(
        u.stage == a[2].key.to_bytes()
            && stage.policy == g.policy
            && s.unit == a[3].key.to_bytes()
            && s.intent == a[4].key.to_bytes()
            && s.sealed
            && !u.released,
    )?;
    let record = result_heap(&s.bytes)?;
    prepared(&record, p, &g, &a[3], &u, &a[4], &i)?;
    let key = signature(
        &a[6],
        ed25519_index,
        &certificate_message(p, &g, &a[3], &i, &s),
    )?;
    let index = g.settlers.iter().position(|k| *k == key).ok_or(bad())?;
    let bit = 1u8 << index;
    if u.accepted {
        require(
            u.intent == a[4].key.to_bytes()
                && u.staging == a[5].key.to_bytes()
                && u.prepared_hash == s.expected_hash,
        )?;
    } else {
        require(!stage.closed && Clock::get()?.unix_timestamp <= stage.deadline)?;
    }
    s.votes |= bit;
    if s.votes.count_ones() >= 2 && !u.accepted {
        stage.pending_units = stage.pending_units.checked_sub(1).ok_or(bad())?;
        stage.pending_atoms = stage.pending_atoms.checked_sub(u.amount).ok_or(bad())?;
        stage.accepted_unpaid = stage.accepted_unpaid.checked_add(u.amount).ok_or(bad())?;
        u.accepted = true;
        u.intent = a[4].key.to_bytes();
        u.staging = a[5].key.to_bytes();
        u.beneficiary = i.value.beneficiary;
        u.original_grant = i.original_grant;
        u.funding_grant = i.original_grant;
        u.funding_generation = i.value.grant_generation;
        u.prepared_hash = s.expected_hash;
        u.accepted_at = Clock::get()?.unix_timestamp.try_into().map_err(|_| bad())?;
        save(&a[2], b"FBMSV003", &stage)?;
        save(&a[3], b"FBMUV003", &u)?;
    }
    save(&a[5], b"FBMBV003", &s)
}

#[inline(never)]
fn handle_rebindfunding(p: &Pubkey, a: &[AccountInfo], generation: u64) -> ProgramResult {
    require(a.len() == 6)?;
    let g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    let mut u = unit_heap(&a[2], p)?;
    let i = intent_heap(&a[3], p)?;
    let next = grant_heap(&a[4], p)?;
    require(
        u.accepted
            && !u.paid
            && !u.released
            && u.intent == a[3].key.to_bytes()
            && a[5].is_signer
            && a[5].key.to_bytes() == u.beneficiary
            && next.miner == u.beneficiary
            && next.beneficiary == u.beneficiary
            && next.policy == g.policy
            && next.generation == generation
            && i.value.beneficiary == u.beneficiary,
    )?;
    next.check(Clock::get()?.unix_timestamp, generation, 0)
        .map_err(|_| bad())?;
    u.funding_grant = a[4].key.to_bytes();
    u.funding_generation = generation;
    save(&a[2], b"FBMUV003", &u)
}

#[inline(never)]
fn handle_settle(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 16)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    relayer(&c, &a[12])?;
    let mut stage = stage_heap(&a[2], p)?;
    let mut u = unit_heap(&a[3], p)?;
    require(
        u.stage == a[2].key.to_bytes()
            && stage.policy == g.policy
            && u.accepted
            && !u.released
            && *a[7].key == result_key(p, &a[3])
            && a[11].key.to_bytes() == u.beneficiary
            && *a[13].key == system::ID
            && *a[14].key == spl_token::ID
            && *a[9].key == pool_key(p, 0)
            && *a[10].key == ata_key(a[8].key, a[11].key)?,
    )?;
    issued_mint(&a[8], &g)?;
    distinct(&[
        &a[0], &a[1], &a[2], &a[3], &a[4], &a[5], &a[6], &a[7], &a[8], &a[9], &a[10], &a[11],
        &a[12],
    ])?;
    require(
        a[4].key.to_bytes() == u.intent
            && a[5].key.to_bytes() == u.staging
            && a[6].key.to_bytes() == u.funding_grant,
    )?;
    if u.paid {
        return Ok(());
    }
    writable(&[&a[2], &a[3], &a[6], &a[7], &a[9], &a[10], &a[12]])?;
    require(
        a[4].key.to_bytes() == u.intent
            && a[5].key.to_bytes() == u.staging
            && a[6].key.to_bytes() == u.funding_grant,
    )?;
    let i = intent_heap(&a[4], p)?;
    let s = staging_heap(&a[5], p)?;
    let mut current = grant_heap(&a[6], p)?;
    require(
        s.sealed
            && s.votes.count_ones() >= 2
            && s.expected_hash == u.prepared_hash
            && s.intent == u.intent
            && s.unit == a[3].key.to_bytes()
            && current.miner == u.beneficiary
            && current.beneficiary == u.beneficiary
            && current.policy == g.policy
            && current.generation == u.funding_generation,
    )?;
    let record = result_heap(&s.bytes)?;
    prepared(&record, p, &g, &a[3], &u, &a[4], &i)?;
    require(a[7].data_is_empty() && *a[7].owner == system::ID)?;
    let rent = Rent::get()?
        .minimum_balance(RESULT_SIZE)
        .saturating_sub(a[7].lamports());
    let clock = Clock::get()?;
    current
        .check(clock.unix_timestamp, u.funding_generation, rent)
        .map_err(|_| bad())?;
    if u.funding_grant == i.original_grant && u.funding_generation == i.value.grant_generation {
        require(rent <= i.value.max_result_rent)?;
    }
    grant_custody(&a[6], &current)?;
    token(&a[9], &g.mint, &a[0].key.to_bytes())?;
    let ata_program: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        .parse()
        .map_err(|_| bad())?;
    require(*a[15].key == ata_program)?;
    if a[10].data_is_empty() {
        let ix = solana_program::instruction::Instruction {
            program_id: ata_program,
            accounts: vec![
                solana_program::instruction::AccountMeta::new(*a[12].key, true),
                solana_program::instruction::AccountMeta::new(*a[10].key, false),
                solana_program::instruction::AccountMeta::new_readonly(*a[11].key, false),
                solana_program::instruction::AccountMeta::new_readonly(*a[8].key, false),
                solana_program::instruction::AccountMeta::new_readonly(*a[13].key, false),
                solana_program::instruction::AccountMeta::new_readonly(*a[14].key, false),
            ],
            data: vec![1],
        };
        invoke(
            &ix,
            &[
                a[12].clone(),
                a[10].clone(),
                a[11].clone(),
                a[8].clone(),
                a[13].clone(),
                a[14].clone(),
                a[15].clone(),
            ],
        )?;
    }
    token(&a[10], &g.mint, &u.beneficiary)?;
    let bump = Pubkey::find_program_address(&[b"result-v3", a[3].key.as_ref()], p).1;
    invoke_signed(
        &sys::allocate(a[7].key, RESULT_SIZE as u64),
        &[a[7].clone(), a[13].clone()],
        &[&[b"result-v3", a[3].key.as_ref(), &[bump]]],
    )?;
    invoke_signed(
        &sys::assign(a[7].key, p),
        &[a[7].clone(), a[13].clone()],
        &[&[b"result-v3", a[3].key.as_ref(), &[bump]]],
    )?;
    // Complete all CPI before direct program-owned lamport changes. Solana
    // synchronizes CPI balances independently; the whole outer instruction is
    // still one atomic result/rent/token business boundary.
    transfer(p, &a[0], &a[9], &a[10], &a[14], u.amount)?;
    if rent > 0 {
        move_lamports(&a[6], &a[7], rent)?;
    }
    let result_hash = write_result(&a[7], &s.bytes, rent, u.accepted_at, &clock)?;
    current
        .consume(clock.unix_timestamp, u.funding_generation, rent)
        .map_err(|_| bad())?;
    grant_custody(&a[6], &current)?;
    stage.accepted_unpaid = stage.accepted_unpaid.checked_sub(u.amount).ok_or(bad())?;
    stage.paid = stage.paid.checked_add(u.amount).ok_or(bad())?;
    u.paid = true;
    u.result_hash = result_hash;
    u.actual_rent = rent;
    u.paid_slot = clock.slot;
    u.paid_at = clock.unix_timestamp;
    save(&a[6], b"FBMGV003", &current)?;
    save(&a[3], b"FBMUV003", &u)?;
    save(&a[2], b"FBMSV003", &stage)
}

#[inline(never)]
fn handle_voteclosure(p: &Pubkey, a: &[AccountInfo], ed25519_index: u16) -> ProgramResult {
    require(a.len() == 4)?;
    let g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    let mut u = unit_heap(&a[2], p)?;
    require(u.paid && !u.closed)?;
    let key = signature(&a[3], ed25519_index, &closure_message(p, &g, &a[2], &u))?;
    let index = g.settlers.iter().position(|k| *k == key).ok_or(bad())?;
    u.close_votes |= 1u8 << index;
    save(&a[2], b"FBMUV003", &u)
}

#[inline(never)]
fn handle_closeresult(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 6)?;
    let g = global_heap(&a[0], p)?;
    let c = configuration(&a[1], p)?;
    let mut u = unit_heap(&a[2], p)?;
    require(
        a[4].is_signer
            && a[4].key.to_bytes() == c.close_authority
            && a[5].key.to_bytes() == c.rent_recipient
            && u.paid
            && u.close_votes.count_ones() >= 2
            && *a[3].key == result_key(p, &a[2]),
    )?;
    if u.closed {
        return require(a[3].data_is_empty() && *a[3].owner == system::ID);
    }
    require(
        a[3].owner == p
            && a[3].data_len() == RESULT_SIZE
            && solana_program::hash::hash(&a[3].try_borrow_data()?).to_bytes() == u.result_hash,
    )?;
    let now = Clock::get()?.unix_timestamp;
    require(
        now >= u
            .paid_at
            .checked_add(c.retention_seconds.try_into().map_err(|_| bad())?)
            .ok_or(bad())?
            && g.issued,
    )?;
    close_account(&a[3], &a[5])?;
    u.closed = true;
    u.closed_at = now;
    save(&a[2], b"FBMUV003", &u)
}

#[inline(never)]
fn handle_releaseunit(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 5)?;
    let g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    admin(&g, &a[4])?;
    let mut stage = stage_heap(&a[2], p)?;
    let mut u = unit_heap(&a[3], p)?;
    require(
        u.stage == a[2].key.to_bytes()
            && stage.policy == g.policy
            && !stage.closed
            && !u.accepted
            && !u.paid
            && Clock::get()?.unix_timestamp > stage.deadline,
    )?;
    if u.released {
        return Ok(());
    }
    u.released = true;
    stage.pending_units = stage.pending_units.checked_sub(1).ok_or(bad())?;
    stage.pending_atoms = stage.pending_atoms.checked_sub(u.amount).ok_or(bad())?;
    stage.remaining = stage.remaining.checked_add(u.amount).ok_or(bad())?;
    require(stage.remaining <= stage.budget)?;
    save(&a[3], b"FBMUV003", &u)?;
    save(&a[2], b"FBMSV003", &stage)
}

#[inline(never)]
fn handle_closestage(p: &Pubkey, a: &[AccountInfo]) -> ProgramResult {
    require(a.len() == 4)?;
    let mut g = global_heap(&a[0], p)?;
    configuration(&a[1], p)?;
    admin(&g, &a[3])?;
    let mut stage = stage_heap(&a[2], p)?;
    require(
        stage.policy == g.policy
            && !stage.closed
            && stage.pending_units == 0
            && stage.pending_atoms == 0
            && stage
                .remaining
                .checked_add(stage.accepted_unpaid)
                .and_then(|n| n.checked_add(stage.paid))
                == Some(stage.budget),
    )?;
    g.available[0] = g.available[0].checked_add(stage.remaining).ok_or(bad())?;
    require(g.available[0] <= POOLS[0])?;
    stage.remaining = 0;
    stage.closed = true;
    save(&a[0], b"FLYGV002", &g)?;
    save(&a[2], b"FBMSV003", &stage)
}
