//! Separate validator collateral. Existing issuance and settlement are untouched.
//! Withdrawal needs owner consent and a responsibility clearance co-signature.
//! This version has no confiscation, yield, policy mutation or authority bypass.
#![allow(unexpected_cfgs)]
use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, clock::Clock, entrypoint::ProgramResult,
    program::{invoke, invoke_signed}, program_error::ProgramError, program_pack::Pack,
    pubkey::Pubkey, rent::Rent, sysvar::Sysvar};
use solana_system_interface::{instruction as sys, program as system};
use spl_token::state::{Account as Token, Mint};

pub const MINIMUM: u64 = 100_000 * 100_000_000;
pub const LOCK_SECONDS: i64 = 30 * 86400;
pub const UNBOND_SECONDS: i64 = 7 * 86400;
pub const SIZE: usize = 256;
type Key = [u8; 32];

#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct Policy {
    pub network: Key, pub mint: Key, pub clearance_authority: Key,
    pub minimum: u64, pub lock_seconds: i64, pub unbond_seconds: i64,
}
#[derive(BorshSerialize, BorshDeserialize, Clone)]
pub struct Bond {
    pub policy: Key, pub validator: Key, pub amount: u64, pub generation: u64,
    pub activated_at: i64, pub exit_requested_at: i64, pub withdraw_after: i64,
    pub state: u8, pub clearance_evidence: Key,
}
#[derive(BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    Initialize { network: Key },
    Stake { amount: u64 },
    RequestExit,
    Withdraw { generation: u64, evidence: Key },
}
#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);
fn bad() -> ProgramError { ProgramError::InvalidArgument }
fn require(ok: bool) -> ProgramResult { if ok { Ok(()) } else { Err(bad()) } }
fn save<T: BorshSerialize>(a: &AccountInfo, tag: &[u8; 8], v: &T) -> ProgramResult {
    let bytes = borsh::to_vec(v).map_err(|_| bad())?;
    let mut d = a.try_borrow_mut_data()?;
    require(a.is_writable && d.len() == SIZE && bytes.len() + 8 <= SIZE)?;
    d.fill(0); d[..8].copy_from_slice(tag); d[8..8 + bytes.len()].copy_from_slice(&bytes); Ok(())
}
fn load<T: BorshDeserialize>(a: &AccountInfo, p: &Pubkey, tag: &[u8; 8]) -> Result<T, ProgramError> {
    require(a.owner == p)?; let d = a.try_borrow_data()?;
    require(d.len() == SIZE && &d[..8] == tag)?;
    let mut remaining = &d[8..]; let v = T::deserialize(&mut remaining).map_err(|_| bad())?;
    require(remaining.iter().all(|b| *b == 0))?; Ok(v)
}
fn policy(a: &AccountInfo, p: &Pubkey) -> Result<Policy, ProgramError> {
    require(*a.key == Pubkey::find_program_address(&[b"validator-policy-v1"], p).0)?;
    let v: Policy = load(a, p, b"FBWPOL01")?;
    require(v.minimum == MINIMUM && v.lock_seconds == LOCK_SECONDS && v.unbond_seconds == UNBOND_SECONDS)?;
    Ok(v)
}
fn bond(a: &AccountInfo, p: &Pubkey, policy_a: &AccountInfo) -> Result<Bond, ProgramError> {
    let b: Bond = load(a, p, b"FBWBND01")?;
    require(b.policy == policy_a.key.to_bytes() &&
        *a.key == Pubkey::find_program_address(&[b"validator-bond-v1", &b.validator], p).0)?; Ok(b)
}
fn vault(p: &Pubkey, bond_a: &AccountInfo) -> Pubkey {
    Pubkey::find_program_address(&[b"validator-vault-v1", bond_a.key.as_ref()], p).0
}
fn token(a: &AccountInfo, mint: &Pubkey, owner: &Pubkey) -> Result<Token, ProgramError> {
    require(*a.owner == spl_token::ID)?;
    let t = Token::unpack(&a.try_borrow_data()?)?;
    require(t.mint == *mint && t.owner == *owner && t.delegate.is_none() &&
        t.close_authority.is_none() && t.state == spl_token::state::AccountState::Initialized)?;
    Ok(t)
}
fn mint(a: &AccountInfo, policy: &Policy) -> ProgramResult {
    require(a.key.to_bytes() == policy.mint && *a.owner == spl_token::ID)?;
    let m = Mint::unpack(&a.try_borrow_data()?)?;
    require(m.decimals == 8 && m.freeze_authority.is_none() && m.mint_authority.is_none())
}
fn create<'a>(p: &Pubkey, target: &AccountInfo<'a>, payer: &AccountInfo<'a>, sy: &AccountInfo<'a>,
    size: usize, owner: &Pubkey, seeds: &[&[u8]]) -> ProgramResult {
    require(payer.is_signer && payer.is_writable && target.is_writable && *sy.key == system::ID)?;
    let (key, bump) = Pubkey::find_program_address(seeds, p);
    require(key == *target.key && target.data_is_empty() && *target.owner == system::ID)?;
    let rent = Rent::get()?.minimum_balance(size).saturating_sub(target.lamports());
    if rent > 0 { invoke(&sys::transfer(payer.key, target.key, rent), &[payer.clone(), target.clone(), sy.clone()])?; }
    let b = [bump]; let mut signed = seeds.to_vec(); signed.push(&b);
    invoke_signed(&sys::allocate(target.key, size as u64), &[target.clone(), sy.clone()], &[&signed])?;
    invoke_signed(&sys::assign(target.key, owner), &[target.clone(), sy.clone()], &[&signed])
}
fn initializer(p: &Pubkey, payer: &AccountInfo, pd: &AccountInfo) -> ProgramResult {
    let loader: Pubkey = "BPFLoaderUpgradeab1e11111111111111111111111".parse().map_err(|_| bad())?;
    require(payer.is_signer && pd.owner == &loader &&
        *pd.key == Pubkey::find_program_address(&[p.as_ref()], &loader).0)?;
    let d = pd.try_borrow_data()?;
    require(d.len() >= 45 && d[..4] == 3u32.to_le_bytes() && d[12] == 1 && d[13..45] == payer.key.to_bytes())
}
pub fn process_instruction(p: &Pubkey, a: &[AccountInfo], data: &[u8]) -> ProgramResult {
    match Instruction::try_from_slice(data).map_err(|_| bad())? {
        Instruction::Initialize { network } => {
            require(a.len() == 5 && network != [0; 32])?;
            initializer(p, &a[2], &a[4])?;
            let policy_v = Policy { network, mint: a[1].key.to_bytes(), clearance_authority: a[2].key.to_bytes(),
                minimum: MINIMUM, lock_seconds: LOCK_SECONDS, unbond_seconds: UNBOND_SECONDS };
            mint(&a[1], &policy_v)?;
            create(p, &a[0], &a[2], &a[3], SIZE, p, &[b"validator-policy-v1"])?;
            save(&a[0], b"FBWPOL01", &policy_v)
        },
        Instruction::Stake { amount } => {
            require(a.len() == 9 && a[5].is_signer && *a[8].key == spl_token::ID)?;
            let policy_v = policy(&a[0], p)?; mint(&a[3], &policy_v)?;
            require(amount >= policy_v.minimum && *a[2].key == vault(p, &a[1]))?;
            let source = token(&a[4], a[3].key, a[5].key)?;
            require(source.amount >= amount && a[4].key != a[2].key)?;
            let generation = if a[1].data_is_empty() {
                create(p, &a[1], &a[6], &a[7], SIZE, p, &[b"validator-bond-v1", a[5].key.as_ref()])?;
                1
            } else {
                let old = bond(&a[1], p, &a[0])?;
                require(old.validator == a[5].key.to_bytes() && old.state == 0 && old.amount == 0)?;
                old.generation.checked_add(1).ok_or(bad())?
            };
            if a[2].data_is_empty() {
                create(p, &a[2], &a[6], &a[7], Token::LEN, &spl_token::ID,
                    &[b"validator-vault-v1", a[1].key.as_ref()])?;
                invoke(&spl_token::instruction::initialize_account3(&spl_token::ID, a[2].key, a[3].key, a[1].key)?,
                    &[a[2].clone(), a[3].clone(), a[8].clone()])?;
            }
            token(&a[2], a[3].key, a[1].key)?;
            invoke(&spl_token::instruction::transfer_checked(&spl_token::ID, a[4].key, a[3].key, a[2].key, a[5].key, &[], amount, 8)?,
                &[a[4].clone(), a[3].clone(), a[2].clone(), a[5].clone(), a[8].clone()])?;
            save(&a[1], b"FBWBND01", &Bond { policy: a[0].key.to_bytes(), validator: a[5].key.to_bytes(), amount, generation,
                activated_at: Clock::get()?.unix_timestamp, exit_requested_at: 0, withdraw_after: 0, state: 1, clearance_evidence: [0; 32] })
        },
        Instruction::RequestExit => {
            require(a.len() == 3 && a[2].is_signer)?;
            let policy_v = policy(&a[0], p)?; let mut b = bond(&a[1], p, &a[0])?;
            let now = Clock::get()?.unix_timestamp;
            require(b.state == 1 && b.validator == a[2].key.to_bytes() &&
                now >= b.activated_at.checked_add(policy_v.lock_seconds).ok_or(bad())?)?;
            b.state = 2; b.exit_requested_at = now;
            b.withdraw_after = now.checked_add(policy_v.unbond_seconds).ok_or(bad())?;
            save(&a[1], b"FBWBND01", &b)
        },
        Instruction::Withdraw { generation, evidence } => {
            require(a.len() == 8 && a[5].is_signer && a[6].is_signer && *a[7].key == spl_token::ID && evidence != [0; 32])?;
            let policy_v = policy(&a[0], p)?; let mut b = bond(&a[1], p, &a[0])?; mint(&a[3], &policy_v)?;
            require(a[6].key.to_bytes() == policy_v.clearance_authority && b.validator == a[5].key.to_bytes() &&
                b.state == 2 && b.generation == generation && b.amount >= policy_v.minimum &&
                b.withdraw_after > 0 && Clock::get()?.unix_timestamp >= b.withdraw_after && *a[2].key == vault(p, &a[1]))?;
            let source = token(&a[2], a[3].key, a[1].key)?;
            token(&a[4], a[3].key, a[5].key)?; require(source.amount >= b.amount && a[2].key != a[4].key)?;
            let (_, bump) = Pubkey::find_program_address(&[b"validator-bond-v1", &b.validator], p);
            invoke_signed(&spl_token::instruction::transfer_checked(&spl_token::ID, a[2].key, a[3].key, a[4].key, a[1].key, &[], source.amount, 8)?,
                &[a[2].clone(), a[3].clone(), a[4].clone(), a[1].clone(), a[7].clone()],
                &[&[b"validator-bond-v1", &b.validator, &[bump]]])?;
            b.state = 0; b.amount = 0; b.clearance_evidence = evidence;
            save(&a[1], b"FBWBND01", &b)
        }
    }
}
