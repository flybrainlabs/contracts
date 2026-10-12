//! Native Solana implementation with Anchor-compatible eight-byte account discriminators.
//! The 1024-byte receipt is shared with WASM. No SQLite or server participates in settlement.
#![allow(unexpected_cfgs)]
use borsh::{BorshDeserialize, BorshSerialize};
use flypow_campaign_core as protocol;
use flypow_receipt_v1::{
    Funding, Hashes, Header, Identity, Lifecycle, SubmissionV1, Task, ACCOUNT_BYTES,
};
use flypow_task_catalog::TaskDescriptor;
use protocol::{CampaignAccount, Entitlement, EpochAccount, Key, ResultPage, Rules, Vault};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::{
        instructions::{load_current_index_checked, load_instruction_at_checked},
        Sysvar,
    },
};
use solana_system_interface::{instruction as system_ix, program as system_program};
use spl_token::state::{Account as TokenAccount, Mint};

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

/// Instruction encoding is Borsh enum (u8 tag); account order is documented in CAMPAIGN_LOCALNET_ZH.md.
#[derive(BorshSerialize, BorshDeserialize)]
// Fixed ordered Borsh instruction fields; boxing would complicate its SBF ABI.
#[allow(clippy::large_enum_variant)]
pub enum Instruction {
    Init {
        campaign_id: Key,
        rules_len: u32,
    },
    WriteRules {
        offset: u32,
        bytes: Vec<u8>,
    },
    InitEpoch {
        target_slot: u32,
    },
    InitTask {
        task: TaskDescriptor,
        proof: Vec<Key>,
    },
    InitMint,
    Open,
    Commit {
        task: TaskDescriptor,
        proof: Vec<Key>,
        commitment: Key,
        beneficiary: Key,
        refund: Key,
        expected_seq: u32,
        max_lamports: u64,
    },
    Reveal(protocol::Reveal),
    Vote {
        verifier: u8,
        accept: bool,
        ed25519_instruction: u16,
    },
    Finalize,
    Close,
    Seal,
    End,
    Refund,
    ReleasePrincipal,
    WithdrawSurplus,
    Tge,
    Claim {
        pool: u8,
        verifier: u8,
        amount_atoms: u64,
    },
}

fn err() -> ProgramError {
    ProgramError::InvalidArgument
}
fn rule<T>(r: protocol::Result<T>) -> Result<T, ProgramError> {
    r.map_err(|e| {
        ProgramError::Custom(
            100 + match e {
                protocol::Error::Config => 0,
                protocol::Error::State => 1,
                protocol::Error::Deadline => 2,
                protocol::Error::Membership => 3,
                protocol::Error::Identity => 4,
                protocol::Error::Commitment => 5,
                protocol::Error::Summary => 6,
                protocol::Error::DuplicateVote => 7,
                protocol::Error::Priority => 8,
                protocol::Error::QueueFull => 9,
                protocol::Error::Funds => 10,
                protocol::Error::Overflow => 11,
            },
        )
    })
}
fn key(a: &AccountInfo) -> Key {
    a.key.to_bytes()
}
fn check(b: bool) -> ProgramResult {
    if b {
        Ok(())
    } else {
        Err(err())
    }
}
fn signer(a: &AccountInfo) -> ProgramResult {
    if a.is_signer {
        Ok(())
    } else {
        Err(ProgramError::MissingRequiredSignature)
    }
}
fn pda(pid: &Pubkey, a: &AccountInfo, seeds: &[&[u8]]) -> Result<u8, ProgramError> {
    let (expected, bump) = Pubkey::find_program_address(seeds, pid);
    check(expected == *a.key)?;
    Ok(bump)
}
fn disc(name: &str) -> [u8; 8] {
    protocol::digest(&[b"account:", name.as_bytes()])[..8]
        .try_into()
        .unwrap()
}
fn load<T: BorshDeserialize>(pid: &Pubkey, a: &AccountInfo, name: &str) -> Result<T, ProgramError> {
    check(a.owner == pid && a.data_len() >= 8)?;
    let d = a.try_borrow_data()?;
    check(d[..8] == disc(name))?;
    let mut bytes = &d[8..];
    let v = T::deserialize(&mut bytes).map_err(|_| err())?;
    check(bytes.iter().all(|v| *v == 0))?;
    Ok(v)
}
fn store<T: BorshSerialize>(a: &AccountInfo, name: &str, v: &T) -> ProgramResult {
    check(a.is_writable)?;
    let bytes = borsh::to_vec(v).map_err(|_| err())?;
    let mut d = a.try_borrow_mut_data()?;
    check(d.len() >= 8 + bytes.len())?;
    d.fill(0);
    d[..8].copy_from_slice(&disc(name));
    d[8..8 + bytes.len()].copy_from_slice(&bytes);
    Ok(())
}
fn owned_transfer(from: &AccountInfo, to: &AccountInfo, n: u64, minimum: u64) -> ProgramResult {
    check(from.is_writable && to.is_writable && from.key != to.key)?;
    let balance = from
        .lamports()
        .checked_sub(n)
        .ok_or(ProgramError::InsufficientFunds)?;
    check(balance >= minimum)?;
    let target = to
        .lamports()
        .checked_add(n)
        .ok_or(ProgramError::ArithmeticOverflow)?;
    **from.try_borrow_mut_lamports()? = balance;
    **to.try_borrow_mut_lamports()? = target;
    Ok(())
}
fn transfer<'a>(
    payer: &AccountInfo<'a>,
    to: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    n: u64,
) -> ProgramResult {
    if n == 0 {
        return Ok(());
    }
    signer(payer)?;
    check(*system.key == system_program::id() && payer.owner == &system_program::id())?;
    invoke(
        &system_ix::transfer(payer.key, to.key, n),
        &[payer.clone(), to.clone(), system.clone()],
    )
}
fn create<'a>(
    pid: &Pubkey,
    a: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    seeds: &[&[u8]],
    size: usize,
    pool: bool,
) -> ProgramResult {
    check(
        *system.key == system_program::id()
            && a.owner == &system_program::id()
            && a.data_is_empty()
            && size <= 10_240,
    )?;
    let bump = pda(pid, a, seeds)?;
    let bs = [bump];
    let mut sign = seeds.to_vec();
    sign.push(&bs);
    let rent = Rent::get()?.minimum_balance(size);
    if pool {
        check(payer.owner == pid)?;
        owned_transfer(
            payer,
            a,
            rent,
            Rent::get()?.minimum_balance(payer.data_len()),
        )?;
    } else {
        transfer(payer, a, system, rent)?;
    }
    let mut allocate = system_ix::allocate(a.key, size as u64);
    let mut allocate_accounts = vec![a.clone(), system.clone()];
    if pool {
        // Both sides of the preceding program-owned lamport transfer must be
        // synchronized at this CPI boundary. Omitting the pool exposes only
        // the destination's credit and the runtime rejects the unbalanced CPI.
        allocate
            .accounts
            .push(solana_program::instruction::AccountMeta::new(
                *payer.key, false,
            ));
        allocate_accounts.push(payer.clone());
    }
    invoke_signed(&allocate, &allocate_accounts, &[&sign])?;
    invoke_signed(
        &system_ix::assign(a.key, pid),
        &[a.clone(), system.clone()],
        &[&sign],
    )
}
#[allow(clippy::too_many_arguments)] // Explicit account roles at the CPI boundary.
fn create_typed<'a, T: BorshSerialize>(
    pid: &Pubkey,
    a: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    seeds: &[&[u8]],
    name: &str,
    v: &T,
    pool: bool,
) -> ProgramResult {
    create(
        pid,
        a,
        payer,
        system,
        seeds,
        8 + borsh::to_vec(v).map_err(|_| err())?.len(),
        pool,
    )?;
    store(a, name, v)
}
#[inline(never)]
fn campaign(
    pid: &Pubkey,
    a: &AccountInfo,
) -> Result<(CampaignAccount, Box<Rules>, u8), ProgramError> {
    let c: CampaignAccount = load(pid, a, "CampaignConfig")?;
    let r = rule(c.rules())?;
    let bump = pda(pid, a, &[b"campaign", &r.catalog.campaign_id])?;
    rule(r.validate())?;
    Ok((c, Box::new(r), bump))
}
fn epoch(
    pid: &Pubkey,
    a: &AccountInfo,
    c: &AccountInfo,
    r: &Rules,
    body: u64,
) -> Result<EpochAccount, ProgramError> {
    pda(pid, a, &[b"epoch", c.key.as_ref(), &body.to_le_bytes()])?;
    let e: EpochAccount = load(pid, a, "NeuronEpoch")?;
    let per = (r.catalog.stimuli.len() as u64) * u64::from(r.catalog.repeats);
    let slot = r
        .catalog
        .targets
        .iter()
        .position(|t| t.body_id == body)
        .ok_or_else(err)?;
    check(
        e.campaign == key(c)
            && e.body_id == body
            && e.target_index == r.catalog.targets[slot].model_index
            && e.first_k == slot as u64 * per
            && u64::from(e.task_count) == per
            && e.bits.len() == (per as usize).div_ceil(8),
    )?;
    Ok(e)
}
fn page(
    pid: &Pubkey,
    a: &AccountInfo,
    c: &AccountInfo,
    t: Key,
) -> Result<ResultPage, ProgramError> {
    pda(pid, a, &[b"result", c.key.as_ref(), &t])?;
    let p: ResultPage = load(pid, a, "ResultPage")?;
    check(p.campaign == key(c) && p.task_id == t)?;
    Ok(p)
}
fn receipt(
    pid: &Pubkey,
    a: &AccountInfo,
    c: &AccountInfo,
    r: &Rules,
) -> Result<SubmissionV1, ProgramError> {
    check(a.owner == pid && a.data_len() == ACCOUNT_BYTES)?;
    let s = SubmissionV1::decode_account(&a.try_borrow_data()?).map_err(|_| err())?;
    let bump = pda(
        pid,
        a,
        &[
            b"submission",
            c.key.as_ref(),
            &s.hashes.task_id,
            &s.task.candidate_seq.to_le_bytes(),
        ],
    )?;
    check(s.header.bump == bump && s.identity.campaign == key(c))?;
    rule(r.check_receipt(&s))?;
    Ok(s)
}
fn save_receipt(a: &AccountInfo, s: &SubmissionV1) -> ProgramResult {
    check(a.is_writable && a.data_len() == 1024)?;
    a.try_borrow_mut_data()?
        .copy_from_slice(&s.encode_account().map_err(|_| err())?);
    Ok(())
}
fn vault(
    pid: &Pubkey,
    a: &AccountInfo,
    c: &AccountInfo,
    body: u64,
    ops: bool,
) -> Result<Vault, ProgramError> {
    if ops {
        pda(pid, a, &[b"ops", c.key.as_ref()])?;
    } else {
        pda(
            pid,
            a,
            &[b"settlement", c.key.as_ref(), &body.to_le_bytes()],
        )?;
    }
    let v: Vault = load(
        pid,
        a,
        if ops {
            "OperatingPool"
        } else {
            "SettlementVault"
        },
    )?;
    check(v.campaign == key(c) && v.body_id == body)?;
    Ok(v)
}
fn entitlement(
    pid: &Pubkey,
    a: &AccountInfo,
    c: &AccountInfo,
) -> Result<Entitlement, ProgramError> {
    let e: Entitlement = load(pid, a, "MinerEntitlement")?;
    pda(pid, a, &[b"entitlement", &e.submission])?;
    check(e.campaign == key(c))?;
    Ok(e)
}

/// Strict inline offsets: the runtime already executed the preceding Ed25519 precompile.
/// Cross-instruction offsets, multiple signatures and a substituted instruction are rejected.
fn ed25519(sysvar: &AccountInfo, index: u16, pk: Key, message: &[u8]) -> ProgramResult {
    check(*sysvar.key == solana_program::sysvar::instructions::id())?;
    check(index < load_current_index_checked(sysvar)?)?;
    let ix = load_instruction_at_checked(index as usize, sysvar)?;
    check(
        ix.program_id == solana_program::pubkey!("Ed25519SigVerify111111111111111111111111111")
            && ix.accounts.is_empty(),
    )?;
    let d = &ix.data;
    check(d.len() >= 16 && d[0] == 1 && d[1] == 0)?;
    let u = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    check(u(4) == u16::MAX && u(8) == u16::MAX && u(14) == u16::MAX)?;
    let so = u(2) as usize;
    let po = u(6) as usize;
    let mo = u(10) as usize;
    let ml = u(12) as usize;
    check(
        so >= 16
            && po >= 16
            && mo >= 16
            && so.checked_add(64).is_some_and(|e| e <= d.len())
            && po.checked_add(32).is_some_and(|e| e <= d.len())
            && mo.checked_add(ml).is_some_and(|e| e == d.len()),
    )?;
    check(d[po..po + 32] == pk && ml == message.len() && &d[mo..] == message)
}

pub fn process_instruction(pid: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let ix = Instruction::try_from_slice(data).map_err(|_| err())?;
    let ai = &mut accounts.iter();
    // Campaign is always first. All other account order is specific to the instruction.
    let ca = next_account_info(ai)?;
    if let Instruction::Init {
        campaign_id,
        rules_len,
    } = ix
    {
        let authority = next_account_info(ai)?;
        let op = next_account_info(ai)?;
        let system = next_account_info(ai)?;
        signer(authority)?;
        check((400..=6144).contains(&rules_len))?;
        let c = CampaignAccount {
            authority: key(authority),
            rules_bytes: vec![0; rules_len as usize],
            ..CampaignAccount::default()
        };
        create_typed(
            pid,
            ca,
            authority,
            system,
            &[b"campaign", &campaign_id],
            "CampaignConfig",
            &c,
            false,
        )?;
        let v = Vault {
            campaign: key(ca),
            ..Vault::default()
        };
        return create_typed(
            pid,
            op,
            authority,
            system,
            &[b"ops", ca.key.as_ref()],
            "OperatingPool",
            &v,
            false,
        );
    }
    if let Instruction::WriteRules { offset, bytes } = ix {
        let authority = next_account_info(ai)?;
        signer(authority)?;
        let mut c: CampaignAccount = load(pid, ca, "CampaignConfig")?;
        check(c.state == protocol::DRAFT && c.authority == key(authority) && bytes.len() <= 800)?;
        let end = (offset as usize).checked_add(bytes.len()).ok_or_else(err)?;
        check(end <= c.rules_bytes.len())?;
        c.rules_bytes[offset as usize..end].copy_from_slice(&bytes);
        return store(ca, "CampaignConfig", &c);
    }
    let (mut c, r, cb) = campaign(pid, ca)?;
    let clock = Clock::get()?;
    match ix {
        Instruction::InitEpoch { target_slot } => {
            let ea = next_account_info(ai)?;
            let va = next_account_info(ai)?;
            let op = next_account_info(ai)?;
            let authority = next_account_info(ai)?;
            let system = next_account_info(ai)?;
            signer(authority)?;
            check(c.state == protocol::DRAFT && c.authority == key(authority))?;
            vault(pid, op, ca, 0, true)?;
            let t = r
                .catalog
                .targets
                .get(target_slot as usize)
                .ok_or_else(err)?;
            let n = r.catalog.stimuli.len() as u32 * r.catalog.repeats;
            let e = EpochAccount {
                campaign: key(ca),
                body_id: t.body_id,
                target_index: t.model_index,
                first_k: u64::from(target_slot) * u64::from(n),
                task_count: n,
                bits: vec![0; (n as usize).div_ceil(8)],
                ..EpochAccount::default()
            };
            create_typed(
                pid,
                ea,
                op,
                system,
                &[b"epoch", ca.key.as_ref(), &t.body_id.to_le_bytes()],
                "NeuronEpoch",
                &e,
                true,
            )?;
            let v = Vault {
                campaign: key(ca),
                body_id: t.body_id,
                liability: 0,
            };
            create_typed(
                pid,
                va,
                op,
                system,
                &[b"settlement", ca.key.as_ref(), &t.body_id.to_le_bytes()],
                "SettlementVault",
                &v,
                true,
            )?;
            c.initialized_epochs += 1;
            store(ca, "CampaignConfig", &c)
        }
        Instruction::InitTask { task, proof } => {
            let ea = next_account_info(ai)?;
            let pa = next_account_info(ai)?;
            let op = next_account_info(ai)?;
            let authority = next_account_info(ai)?;
            let system = next_account_info(ai)?;
            signer(authority)?;
            check(c.state == protocol::DRAFT && c.authority == key(authority))?;
            rule(r.check_task(&task, &proof))?;
            vault(pid, op, ca, 0, true)?;
            let mut e = epoch(pid, ea, ca, &r, task.body_id)?;
            let p = ResultPage {
                campaign: key(ca),
                task_id: task.task_id,
                global_index: task.global_index,
                ..ResultPage::default()
            };
            create_typed(
                pid,
                pa,
                op,
                system,
                &[b"result", ca.key.as_ref(), &task.task_id],
                "ResultPage",
                &p,
                true,
            )?;
            e.initialized_tasks += 1;
            check(e.initialized_tasks <= e.task_count)?;
            store(ea, "NeuronEpoch", &e)
        }
        Instruction::InitMint => {
            let ma = next_account_info(ai)?;
            let authority = next_account_info(ai)?;
            let system = next_account_info(ai)?;
            let token = next_account_info(ai)?;
            signer(authority)?;
            check(
                c.state == protocol::DRAFT
                    && c.authority == key(authority)
                    && *token.key == spl_token::id()
                    && key(ma) == r.mint,
            )?;
            create_token(
                pid,
                ma,
                authority,
                system,
                &[b"mint", ca.key.as_ref()],
                Mint::LEN,
            )?;
            invoke(
                &spl_token::instruction::initialize_mint2(token.key, ma.key, ca.key, None, 8)?,
                &[ma.clone(), token.clone()],
            )?;
            for pool in 0..3u8 {
                let ta = next_account_info(ai)?;
                create_token(
                    pid,
                    ta,
                    authority,
                    system,
                    &[b"tokens", ca.key.as_ref(), &[pool]],
                    TokenAccount::LEN,
                )?;
                invoke(
                    &spl_token::instruction::initialize_account3(
                        token.key, ta.key, ma.key, ca.key,
                    )?,
                    &[ta.clone(), ma.clone(), token.clone()],
                )?;
            }
            Ok(())
        }
        Instruction::Open => {
            let authority = next_account_info(ai)?;
            let ma = next_account_info(ai)?;
            signer(authority)?;
            check(
                c.state == protocol::DRAFT
                    && c.authority == key(authority)
                    && c.initialized_epochs as usize == r.catalog.targets.len()
                    && clock.unix_timestamp < r.submit_deadline,
            )?;
            check_mint(ma, ca, &r, false)?;
            // Membership verifies BOTH canonical task derivation and the frozen Merkle root.
            // Building a whole Merkle tree in one SBF instruction would exceed heap/CU limits.
            for t in &r.catalog.targets {
                let ea = next_account_info(ai)?;
                let e = epoch(pid, ea, ca, &r, t.body_id)?;
                check(e.initialized_tasks == e.task_count)?;
            }
            c.state = protocol::RUNNING;
            store(ca, "CampaignConfig", &c)
        }
        Instruction::Commit {
            task,
            proof,
            commitment,
            beneficiary,
            refund,
            expected_seq,
            max_lamports,
        } => op_commit(
            pid,
            ca,
            ai,
            &c,
            &r,
            &clock,
            task,
            proof,
            commitment,
            beneficiary,
            refund,
            expected_seq,
            max_lamports,
        ),
        Instruction::Reveal(reveal) => op_reveal(pid, ca, ai, &c, &r, &clock, reveal),
        Instruction::Vote {
            verifier,
            accept,
            ed25519_instruction,
        } => op_vote(
            pid,
            ca,
            ai,
            &c,
            &r,
            &clock,
            verifier,
            accept,
            ed25519_instruction,
        ),
        Instruction::Finalize => op_finalize(pid, ca, ai, &c, &r, &clock),
        Instruction::Close => op_close(pid, ca, ai, &c, &r, &clock),
        Instruction::Seal => {
            let ea = next_account_info(ai)?;
            let v: EpochAccount = load(pid, ea, "NeuronEpoch")?;
            let mut e = epoch(pid, ea, ca, &r, v.body_id)?;
            check(
                c.state == protocol::RUNNING
                    && clock.unix_timestamp <= rule(r.final_deadline())?
                    && !e.sealed
                    && e.completed == e.task_count
                    && e.archived == e.task_count,
            )?;
            for i in 0..e.task_count as usize {
                check(e.bits[i / 8] & (1 << (i % 8)) != 0)?;
            }
            e.sealed = true;
            c.sealed_epochs += 1;
            store(ea, "NeuronEpoch", &e)?;
            store(ca, "CampaignConfig", &c)
        }
        Instruction::End => {
            check(c.state == protocol::RUNNING)?;
            if c.sealed_epochs as usize == r.catalog.targets.len() {
                c.state = protocol::SUCCESS;
            } else {
                check(clock.unix_timestamp > rule(r.final_deadline())?)?;
                c.state = protocol::FAILED;
            }
            store(ca, "CampaignConfig", &c)
        }
        Instruction::Refund | Instruction::ReleasePrincipal => {
            let ent = next_account_info(ai)?;
            let va = next_account_info(ai)?;
            let recipient = next_account_info(ai)?;
            let mut claim = entitlement(pid, ent, ca)?;
            let mut v = vault(pid, va, ca, claim.body_id, false)?;
            check(claim.closed)?;
            if matches!(ix, Instruction::ReleasePrincipal) {
                check(c.state == protocol::SUCCESS && claim.accepted && !claim.principal_released)?;
                v.liability = v.liability.checked_sub(claim.principal).ok_or_else(err)?;
                claim.principal_released = true;
            } else {
                check(
                    !claim.refund_paid
                        && key(recipient) == claim.refund_recipient
                        && (!claim.accepted
                            || c.state == protocol::FAILED
                            || c.state == protocol::SUCCESS),
                )?;
                let refundable_principal = if claim.accepted && c.state == protocol::SUCCESS {
                    0
                } else {
                    claim.principal
                };
                let amount = refundable_principal
                    .checked_add(claim.advance)
                    .ok_or_else(err)?;
                v.liability = v.liability.checked_sub(amount).ok_or_else(err)?;
                owned_transfer(
                    va,
                    recipient,
                    amount,
                    Rent::get()?.minimum_balance(va.data_len()) + v.liability,
                )?;
                claim.refund_paid = true;
            }
            store(ent, "MinerEntitlement", &claim)?;
            store(va, "SettlementVault", &v)
        }
        Instruction::WithdrawSurplus => {
            let va = next_account_info(ai)?;
            let developer = next_account_info(ai)?;
            check(c.state == protocol::SUCCESS && key(developer) == r.developer)?;
            let raw: Vault = load(pid, va, "SettlementVault")?;
            let v = vault(pid, va, ca, raw.body_id, false)?;
            let reserve = Rent::get()?
                .minimum_balance(va.data_len())
                .checked_add(v.liability)
                .ok_or_else(err)?;
            let surplus = va.lamports().checked_sub(reserve).ok_or_else(err)?;
            owned_transfer(va, developer, surplus, reserve)
        }
        Instruction::Tge => {
            let ma = next_account_info(ai)?;
            let token = next_account_info(ai)?;
            check(c.state == protocol::SUCCESS && !c.tge && *token.key == spl_token::id())?;
            check_mint(ma, ca, &r, false)?;
            let seed = [cb];
            let sign: &[&[u8]] = &[b"campaign", &r.catalog.campaign_id, &seed];
            for (pool, whole) in [
                r.miner_pool_whole,
                r.validator_pool_whole,
                r.community_pool_whole,
            ]
            .into_iter()
            .enumerate()
            {
                let ta = next_account_info(ai)?;
                check_token_vault(pid, ta, ca, ma, pool as u8)?;
                let amount = whole
                    .checked_mul(flypow_receipt_v1::ATOMS_PER_FLY)
                    .ok_or_else(err)?;
                invoke_signed(
                    &spl_token::instruction::mint_to(
                        token.key,
                        ma.key,
                        ta.key,
                        ca.key,
                        &[],
                        amount,
                    )?,
                    &[ma.clone(), ta.clone(), ca.clone(), token.clone()],
                    &[sign],
                )?;
            }
            invoke_signed(
                &spl_token::instruction::set_authority(
                    token.key,
                    ma.key,
                    None,
                    spl_token::instruction::AuthorityType::MintTokens,
                    ca.key,
                    &[],
                )?,
                &[ma.clone(), ca.clone(), token.clone()],
                &[sign],
            )?;
            c.tge = true;
            store(ca, "CampaignConfig", &c)
        }
        Instruction::Claim {
            pool,
            verifier,
            amount_atoms,
        } => op_claim(pid, ca, ai, &mut c, &r, cb, pool, verifier, amount_atoms),
        _ => Err(err()),
    }
}

fn create_token<'a>(
    pid: &Pubkey,
    a: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    system: &AccountInfo<'a>,
    seeds: &[&[u8]],
    size: usize,
) -> ProgramResult {
    check(
        *system.key == system_program::id()
            && a.owner == &system_program::id()
            && a.data_is_empty(),
    )?;
    let bump = pda(pid, a, seeds)?;
    let bs = [bump];
    let mut sign = seeds.to_vec();
    sign.push(&bs);
    transfer(payer, a, system, Rent::get()?.minimum_balance(size))?;
    invoke_signed(
        &system_ix::allocate(a.key, size as u64),
        &[a.clone(), system.clone()],
        &[&sign],
    )?;
    invoke_signed(
        &system_ix::assign(a.key, &spl_token::id()),
        &[a.clone(), system.clone()],
        &[&sign],
    )
}
fn check_mint(ma: &AccountInfo, ca: &AccountInfo, r: &Rules, issued: bool) -> ProgramResult {
    check(ma.owner == &spl_token::id() && key(ma) == r.mint)?;
    pda(ca.owner, ma, &[b"mint", ca.key.as_ref()])?;
    let m = Mint::unpack(&ma.try_borrow_data()?)?;
    check(
        m.decimals == 8
            && m.freeze_authority.is_none()
            && (if issued {
                m.mint_authority.is_none()
                    && m.supply
                        == rule(r.total_whole())?
                            .checked_mul(flypow_receipt_v1::ATOMS_PER_FLY)
                            .ok_or_else(err)?
            } else {
                m.supply == 0
                    && m.mint_authority == solana_program::program_option::COption::Some(*ca.key)
            }),
    )
}
fn check_token_vault(
    pid: &Pubkey,
    ta: &AccountInfo,
    ca: &AccountInfo,
    ma: &AccountInfo,
    pool: u8,
) -> ProgramResult {
    pda(pid, ta, &[b"tokens", ca.key.as_ref(), &[pool]])?;
    check(ta.owner == &spl_token::id())?;
    let t = TokenAccount::unpack(&ta.try_borrow_data()?)?;
    check(
        t.mint == *ma.key
            && t.owner == *ca.key
            && t.delegate.is_none()
            && t.close_authority.is_none(),
    )
}
fn check_ata(a: &AccountInfo, beneficiary: &AccountInfo, ma: &AccountInfo) -> ProgramResult {
    let associated = solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
    let (expected, _) = Pubkey::find_program_address(
        &[
            beneficiary.key.as_ref(),
            spl_token::id().as_ref(),
            ma.key.as_ref(),
        ],
        &associated,
    );
    check(expected == *a.key && a.owner == &spl_token::id())?;
    let t = TokenAccount::unpack(&a.try_borrow_data()?)?;
    check(t.owner == *beneficiary.key && t.mint == *ma.key)
}

#[inline(never)]
#[allow(clippy::too_many_arguments)] // Ordered protocol fields, not UI options.
fn op_commit<'a>(
    pid: &Pubkey,
    ca: &AccountInfo<'a>,
    ai: &mut std::slice::Iter<'_, AccountInfo<'a>>,
    c: &CampaignAccount,
    r: &Rules,
    clock: &Clock,
    task: TaskDescriptor,
    proof: Vec<Key>,
    commitment: Key,
    beneficiary: Key,
    refund: Key,
    expected_seq: u32,
    max_lamports: u64,
) -> ProgramResult {
    let ea = next_account_info(ai)?;
    let pa = next_account_info(ai)?;
    let sa = next_account_info(ai)?;
    let ent = next_account_info(ai)?;
    let payer = next_account_info(ai)?;
    let miner = next_account_info(ai)?;
    let op = next_account_info(ai)?;
    let developer = next_account_info(ai)?;
    let system = next_account_info(ai)?;
    signer(miner)?;
    signer(payer)?;
    check(
        c.state == protocol::RUNNING
            && clock.unix_timestamp <= r.submit_deadline
            && key(developer) == r.developer
            && beneficiary != [0; 32]
            && refund != [0; 32]
            && commitment != [0; 32],
    )?;
    rule(r.check_task(&task, &proof))?;
    epoch(pid, ea, ca, r, task.body_id)?;
    vault(pid, op, ca, 0, true)?;
    let mut p = page(pid, pa, ca, task.task_id)?;
    check(p.global_index == task.global_index && expected_seq == p.next_seq)?;
    let qi = rule(p.slot(clock.unix_timestamp))?;
    let rent = Rent::get()?.minimum_balance(1024);
    let cost = rent
        .checked_add(r.advance)
        .and_then(|v| v.checked_add(r.queue_fee))
        .ok_or(ProgramError::ArithmeticOverflow)?;
    check(cost <= max_lamports)?;
    let seq = expected_seq.to_le_bytes();
    let bump = pda(
        pid,
        sa,
        &[b"submission", ca.key.as_ref(), &task.task_id, &seq],
    )?;
    create(
        pid,
        sa,
        payer,
        system,
        &[b"submission", ca.key.as_ref(), &task.task_id, &seq],
        1024,
        false,
    )?;
    transfer(payer, sa, system, r.advance)?;
    transfer(payer, developer, system, r.queue_fee)?;
    let reveal = clock
        .unix_timestamp
        .checked_add(r.reveal_ttl)
        .ok_or_else(err)?;
    let verify = reveal.checked_add(r.verify_ttl).ok_or_else(err)?;
    let archive = verify.checked_add(r.archive_ttl).ok_or_else(err)?;
    let s = SubmissionV1 {
        header: Header {
            schema_version: 1,
            bump,
            roi_count: r.roi_count,
            time_bin_count: 32,
            summary_encoding: 1,
            readout_schema_id: r.readout_schema_id,
            verifier_policy_id: r.verifier_policy_id,
            ..Header::default()
        },
        identity: Identity {
            campaign: key(ca),
            miner: key(miner),
            payer: key(payer),
            reward_beneficiary: beneficiary,
            refund_recipient: refund,
        },
        task: Task {
            body_id: task.body_id,
            global_task_index: task.global_index,
            target_index: task.target_index,
            stimulus_index: task.stimulus_index,
            repeat_index: task.repeat_index,
            candidate_seq: expected_seq,
            model_neuron_count: r.catalog.model_neuron_count,
            simulation_steps: r.catalog.simulation_steps,
            time_step_us: r.catalog.time_step_us,
            simulation_config_id: r.simulation_config_id,
        },
        hashes: Hashes {
            task_id: task.task_id,
            commitment,
            ..Hashes::default()
        },
        lifecycle: Lifecycle {
            commit_slot: clock.slot,
            committed_unix_ts: clock.unix_timestamp,
            reveal_deadline_ts: reveal,
            verify_deadline_ts: verify,
            archive_deadline_ts: archive,
            ..Lifecycle::default()
        },
        funding: Funding {
            storage_principal_lamports: rent,
            refundable_advance_remaining_lamports: r.advance,
            service_fee_paid_lamports: r.queue_fee,
            ..Funding::default()
        },
        ..SubmissionV1::default()
    };
    save_receipt(sa, &s)?;
    let e = Entitlement {
        campaign: key(ca),
        submission: key(sa),
        miner: key(miner),
        payer: key(payer),
        task_id: task.task_id,
        global_index: task.global_index,
        beneficiary,
        refund_recipient: refund,
        body_id: task.body_id,
        ..Entitlement::default()
    };
    create_typed(
        pid,
        ent,
        op,
        system,
        &[b"entitlement", sa.key.as_ref()],
        "MinerEntitlement",
        &e,
        true,
    )?;
    p.queue[qi] = protocol::QueueEntry {
        submission: key(sa),
        seq: expected_seq,
        status: 0,
        reveal_deadline: reveal,
        verify_deadline: verify,
    };
    p.next_seq = p.next_seq.checked_add(1).ok_or_else(err)?;
    store(pa, "ResultPage", &p)
}

#[inline(never)]
fn op_reveal<'a>(
    pid: &Pubkey,
    ca: &AccountInfo<'a>,
    ai: &mut std::slice::Iter<'_, AccountInfo<'a>>,
    c: &CampaignAccount,
    r: &Rules,
    clock: &Clock,
    reveal: protocol::Reveal,
) -> ProgramResult {
    let pa = next_account_info(ai)?;
    let sa = next_account_info(ai)?;
    let miner = next_account_info(ai)?;
    signer(miner)?;
    check(c.state == protocol::RUNNING)?;
    let mut s = receipt(pid, sa, ca, r)?;
    check(key(miner) == s.identity.miner)?;
    let mut p = page(pid, pa, ca, s.hashes.task_id)?;
    check(!p.complete)?;
    rule(protocol::apply_reveal(
        &mut s,
        r,
        pid.to_bytes(),
        key(ca),
        &reveal,
        clock.slot,
        clock.unix_timestamp,
    ))?;
    rule(p.entry_mut(key(sa)))?.status = 1;
    store(pa, "ResultPage", &p)?;
    save_receipt(sa, &s)
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn op_vote<'a>(
    pid: &Pubkey,
    ca: &AccountInfo<'a>,
    ai: &mut std::slice::Iter<'_, AccountInfo<'a>>,
    c: &CampaignAccount,
    r: &Rules,
    clock: &Clock,
    verifier: u8,
    accept: bool,
    ed25519_instruction: u16,
) -> ProgramResult {
    let pa = next_account_info(ai)?;
    let sa = next_account_info(ai)?;
    let instructions = next_account_info(ai)?;
    let developer = next_account_info(ai)?;
    let op = next_account_info(ai)?;
    check(c.state == protocol::RUNNING && verifier < 3 && key(developer) == r.developer)?;
    vault(pid, op, ca, 0, true)?;
    let mut s = receipt(pid, sa, ca, r)?;
    let mut p = page(pid, pa, ca, s.hashes.task_id)?;
    check(!p.complete)?;
    let message = protocol::verification_message(pid.to_bytes(), key(ca), key(sa), r, &s, accept);
    ed25519(
        instructions,
        ed25519_instruction,
        r.verifiers[verifier as usize],
        &message,
    )?;
    let first = (s.header.accept_mask | s.header.reject_mask) == 0;
    rule(protocol::apply_vote(
        &mut s,
        r,
        verifier as usize,
        accept,
        clock.slot,
        clock.unix_timestamp,
    ))?;
    // Charge only after a real authorized full replay vote. Never spend storage principal.
    if first && r.verify_fee > 0 {
        s.funding.refundable_advance_remaining_lamports = s
            .funding
            .refundable_advance_remaining_lamports
            .checked_sub(r.verify_fee)
            .ok_or_else(err)?;
        s.funding.service_fee_paid_lamports = s
            .funding
            .service_fee_paid_lamports
            .checked_add(r.verify_fee)
            .ok_or_else(err)?;
        owned_transfer(
            sa,
            developer,
            r.verify_fee,
            s.funding.storage_principal_lamports + s.funding.refundable_advance_remaining_lamports,
        )?;
    }
    if s.header.status == 3 && r.rejection_slash > 0 {
        s.funding.refundable_advance_remaining_lamports = s
            .funding
            .refundable_advance_remaining_lamports
            .checked_sub(r.rejection_slash)
            .ok_or_else(err)?;
        s.funding.slash_paid_lamports = r.rejection_slash;
        owned_transfer(
            sa,
            op,
            r.rejection_slash,
            s.funding.storage_principal_lamports + s.funding.refundable_advance_remaining_lamports,
        )?;
    }
    rule(p.entry_mut(key(sa)))?.status = s.header.status;
    store(pa, "ResultPage", &p)?;
    save_receipt(sa, &s)
}

#[inline(never)]
fn op_finalize<'a>(
    pid: &Pubkey,
    ca: &AccountInfo<'a>,
    ai: &mut std::slice::Iter<'_, AccountInfo<'a>>,
    c: &CampaignAccount,
    r: &Rules,
    clock: &Clock,
) -> ProgramResult {
    let ea = next_account_info(ai)?;
    let pa = next_account_info(ai)?;
    let sa = next_account_info(ai)?;
    check(c.state == protocol::RUNNING)?;
    let mut s = receipt(pid, sa, ca, r)?;
    check(
        s.header.status == 2
            && s.lifecycle.accepted_slot == 0
            && clock.unix_timestamp <= s.lifecycle.archive_deadline_ts,
    )?;
    let mut e = epoch(pid, ea, ca, r, s.task.body_id)?;
    let mut p = page(pid, pa, ca, s.hashes.task_id)?;
    rule(p.priority(s.task.candidate_seq, clock.unix_timestamp))?;
    let offset = (s.task.global_task_index - e.first_k) as usize;
    check(offset < e.task_count as usize && e.bits[offset / 8] & (1 << (offset % 8)) == 0)?;
    e.bits[offset / 8] |= 1 << (offset % 8);
    e.completed += 1;
    p.complete = true;
    p.winner = key(sa);
    p.result_root = s.hashes.result_root;
    p.preview_root = s.hashes.preview_root;
    p.archive_root = s.hashes.archive_manifest_hash;
    p.summary_hash = s.hashes.summary_hash;
    s.lifecycle.accepted_slot = clock.slot;
    s.funding.miner_reward_whole = rule(protocol::whole_allocation(
        s.task.global_task_index,
        rule(r.count())?,
        r.miner_pool_whole,
    ))?;
    s.funding.validator_reward_whole = rule(protocol::whole_allocation(
        s.task.global_task_index,
        rule(r.count())?,
        r.validator_pool_whole,
    ))?;
    store(ea, "NeuronEpoch", &e)?;
    store(pa, "ResultPage", &p)?;
    save_receipt(sa, &s)
}

#[inline(never)]
fn op_close<'a>(
    pid: &Pubkey,
    ca: &AccountInfo<'a>,
    ai: &mut std::slice::Iter<'_, AccountInfo<'a>>,
    c: &CampaignAccount,
    r: &Rules,
    clock: &Clock,
) -> ProgramResult {
    let ea = next_account_info(ai)?;
    let pa = next_account_info(ai)?;
    let sa = next_account_info(ai)?;
    let ent = next_account_info(ai)?;
    let va = next_account_info(ai)?;
    let s = receipt(pid, sa, ca, r)?;
    let mut e = epoch(pid, ea, ca, r, s.task.body_id)?;
    let mut p = page(pid, pa, ca, s.hashes.task_id)?;
    let mut claim = entitlement(pid, ent, ca)?;
    check(claim.submission == key(sa) && !claim.closed)?;
    let mut v = vault(pid, va, ca, s.task.body_id, false)?;
    let winning = p.complete && p.winner == key(sa);
    let timed_out = (s.header.status == 0 && clock.unix_timestamp > s.lifecycle.reveal_deadline_ts)
        || (s.header.status == 1 && clock.unix_timestamp > s.lifecycle.verify_deadline_ts)
        || (s.header.status == 2
            && !winning
            && clock.unix_timestamp > s.lifecycle.archive_deadline_ts);
    check(
        winning
            || s.header.status == 3
            || timed_out
            || (p.complete && !winning)
            || c.state == protocol::FAILED,
    )?;
    if winning {
        check(s.lifecycle.accepted_slot != 0 && !p.archived)?;
        if c.state == protocol::RUNNING {
            check(clock.unix_timestamp <= s.lifecycle.archive_deadline_ts)?;
        }
        claim.accepted = true;
        claim.miner_whole = s.funding.miner_reward_whole;
        claim.validator_whole = rule(protocol::validator_shares(
            s.funding.validator_reward_whole,
            s.header.accept_mask,
        ))?;
        p.archived = true;
        e.archived += 1;
    }
    claim.closed = true;
    claim.principal = s.funding.storage_principal_lamports;
    claim.advance = s.funding.refundable_advance_remaining_lamports;
    v.liability = v
        .liability
        .checked_add(claim.principal)
        .and_then(|x| x.checked_add(claim.advance))
        .ok_or_else(err)?;
    check(sa.lamports() >= claim.principal.checked_add(claim.advance).ok_or_else(err)?)?;
    owned_transfer(sa, va, sa.lamports(), 0)?;
    check(
        va.lamports()
            >= Rent::get()?
                .minimum_balance(va.data_len())
                .checked_add(v.liability)
                .ok_or_else(err)?,
    )?;
    if let Ok(q) = p.entry_mut(key(sa)) {
        q.status = 5;
    }
    store(ent, "MinerEntitlement", &claim)?;
    store(va, "SettlementVault", &v)?;
    store(ea, "NeuronEpoch", &e)?;
    store(pa, "ResultPage", &p)?;
    // Whole-account close, only after permanent uniqueness, archive and refund rights are stored.
    sa.try_borrow_mut_data()?.fill(0);
    sa.resize(0)?;
    sa.assign(&system_program::id());
    Ok(())
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn op_claim<'a>(
    pid: &Pubkey,
    ca: &AccountInfo<'a>,
    ai: &mut std::slice::Iter<'_, AccountInfo<'a>>,
    c: &mut CampaignAccount,
    r: &Rules,
    cb: u8,
    pool: u8,
    verifier: u8,
    amount_atoms: u64,
) -> ProgramResult {
    let ent = next_account_info(ai)?;
    let ma = next_account_info(ai)?;
    let ta = next_account_info(ai)?;
    let recipient = next_account_info(ai)?;
    let beneficiary = next_account_info(ai)?;
    let token = next_account_info(ai)?;
    check(
        c.state == protocol::SUCCESS
            && c.tge
            && pool < 3
            && amount_atoms > 0
            && *token.key == spl_token::id(),
    )?;
    let mut claim = if pool < 2 {
        Some(entitlement(pid, ent, ca)?)
    } else {
        None
    };
    let (who, whole, claimed) = if let Some(e) = &claim {
        check(e.closed && e.accepted)?;
        if pool == 0 {
            (e.beneficiary, e.miner_whole, e.miner_claimed)
        } else {
            check(verifier < 3)?;
            (
                r.verifiers[verifier as usize],
                e.validator_whole[verifier as usize],
                e.validator_claimed[verifier as usize],
            )
        }
    } else {
        (
            r.community,
            r.community_pool_whole,
            c.community_claimed_atoms,
        )
    };
    check(
        key(beneficiary) == who
            && amount_atoms
                <= whole
                    .checked_mul(flypow_receipt_v1::ATOMS_PER_FLY)
                    .ok_or_else(err)?
                    .checked_sub(claimed)
                    .ok_or_else(err)?,
    )?;
    check_mint(ma, ca, r, true)?;
    check_token_vault(pid, ta, ca, ma, pool)?;
    check_ata(recipient, beneficiary, ma)?;
    let seed = [cb];
    let sign: &[&[u8]] = &[b"campaign", &r.catalog.campaign_id, &seed];
    invoke_signed(
        &spl_token::instruction::transfer_checked(
            token.key,
            ta.key,
            ma.key,
            recipient.key,
            ca.key,
            &[],
            amount_atoms,
            8,
        )?,
        &[
            ta.clone(),
            ma.clone(),
            recipient.clone(),
            ca.clone(),
            token.clone(),
        ],
        &[sign],
    )?;
    if let Some(e) = &mut claim {
        if pool == 0 {
            e.miner_claimed = e.miner_claimed.checked_add(amount_atoms).ok_or_else(err)?;
        } else {
            e.validator_claimed[verifier as usize] = e.validator_claimed[verifier as usize]
                .checked_add(amount_atoms)
                .ok_or_else(err)?;
        }
        store(ent, "MinerEntitlement", e)
    } else {
        c.community_claimed_atoms = c
            .community_claimed_atoms
            .checked_add(amount_atoms)
            .ok_or_else(err)?;
        store(ca, "CampaignConfig", c)
    }
}
