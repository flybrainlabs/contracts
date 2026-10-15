//! Separate task lifecycle extension. No existing account, grant or receipt is
//! migrated. All task descriptors/rent are provisioned before catalog sealing.
use super::*;
use flybrainlabs_mining_v3::task_factory_v4 as v4;
use v4::{Factory, Operation, Submission, Task};

const FACTORY_TAG: &[u8; 8] = b"FBTFV004";
const TASK_TAG: &[u8; 8] = b"FBTTV004";
const SUBMISSION_TAG: &[u8; 8] = b"FBTSV004";
const SCIENCE_TAG: &[u8; 8] = b"FBTIV004";
fn checked<T>(r: v4::Result<T>) -> Result<T, ProgramError> {
    r.map_err(|_| bad())
}
fn read<T: BorshDeserialize>(
    a: &AccountInfo,
    p: &Pubkey,
    tag: &[u8; 8],
    size: usize,
) -> Result<T, ProgramError> {
    require(a.owner == p && a.data_len() == size)?;
    let d = a.try_borrow_data()?;
    require(&d[..8] == tag)?;
    let mut rest = &d[8..];
    let x = T::deserialize(&mut rest).map_err(|_| bad())?;
    require(rest.iter().all(|b| *b == 0))?;
    Ok(x)
}
fn write<T: BorshSerialize>(a: &AccountInfo, tag: &[u8; 8], x: &T) -> ProgramResult {
    require(a.is_writable)?;
    save(a, tag, x)
}
fn factory(a: &AccountInfo, p: &Pubkey) -> Result<Factory, ProgramError> {
    let f: Factory = read(a, p, FACTORY_TAG, v4::FACTORY_SIZE)?;
    require(*a.key == Pubkey::find_program_address(&[b"task-factory-v4", &f.id], p).0)?;
    checked(f.accounting())?;
    Ok(f)
}
fn task(a: &AccountInfo, p: &Pubkey, factory_key: &Pubkey) -> Result<Task, ProgramError> {
    let t: Task = read(a, p, TASK_TAG, v4::TASK_SIZE)?;
    require(
        t.factory == factory_key.to_bytes()
            && t.queue.len() <= v4::QUEUE_CAPACITY
            && *a.key == task_key(p, factory_key, t.index),
    )?;
    checked(t.descriptor.validate())?;
    Ok(t)
}
fn task_key(p: &Pubkey, f: &Pubkey, index: u32) -> Pubkey {
    Pubkey::find_program_address(&[b"task-v4", f.as_ref(), &index.to_le_bytes()], p).0
}
fn submission(a: &AccountInfo, p: &Pubkey, task_key: &Pubkey) -> Result<Submission, ProgramError> {
    let s: Submission = read(a, p, SUBMISSION_TAG, v4::SUBMISSION_SIZE)?;
    require(
        s.task == task_key.to_bytes()
            && *a.key
                == Pubkey::find_program_address(
                    &[
                        b"submission-v4",
                        task_key.as_ref(),
                        &s.attempt.to_le_bytes(),
                        &s.miner,
                        &s.commitment,
                    ],
                    p,
                )
                .0,
    )?;
    Ok(s)
}
fn authority(f: &Factory, a: &AccountInfo) -> ProgramResult {
    require(a.is_signer && a.key.to_bytes() == f.authority)
}
/// Optional successor is a preallocated catalog account. Missing accounts,
/// exhausted time or pause never roll back a newly earned entitlement. Anyone
/// can supply this account in a later Advance; no unique relayer is required.
fn successor(
    p: &Pubkey,
    fa: &AccountInfo,
    f: &mut Factory,
    next: Option<&AccountInfo>,
    now: i64,
) -> ProgramResult {
    if f.state != v4::RUNNING || f.paused || f.active_index != v4::NONE || f.next_index >= f.count {
        return Ok(());
    }
    if let Some(a) = next {
        let mut t = task(a, p, fa.key)?;
        require(t.index == f.next_index && t.state == v4::SCHEDULED)?;
        if now
            .checked_add(t.descriptor.verify_seconds as i64)
            .is_some_and(|end| end <= f.deadline)
        {
            checked(t.open(f, now))?;
            write(a, TASK_TAG, &t)?;
            solana_program::msg!("TASK_V4_OPENED {} {}", t.index, t.attempt);
        }
    }
    Ok(())
}
pub fn process(p: &Pubkey, a: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let op = Operation::try_from_slice(data).map_err(|_| bad())?;
    match op {
        Operation::Initialize {
            id,
            catalog_root,
            count,
            budget,
            deadline,
        } => {
            require(
                a.len() == 4
                    && id != v4::ZERO
                    && catalog_root != v4::ZERO
                    && count > 0
                    && count <= 4096
                    && budget > 0
                    && deadline > Clock::get()?.unix_timestamp,
            )?;
            let mut g = global(&a[0], p)?;
            admin(&g, &a[2])?;
            require(g.issued)?;
            g.available[0] = g.available[0].checked_sub(budget).ok_or(bad())?;
            create(
                p,
                &a[1],
                &a[2],
                &a[3],
                v4::FACTORY_SIZE,
                p,
                &[b"task-factory-v4", &id],
            )?;
            let f = Factory {
                id,
                authority: g.authority,
                genesis: g.network,
                policy: g.policy,
                mint: g.mint,
                verifiers: g.settlers,
                catalog_root,
                uploaded_root: v4::ZERO,
                count,
                uploaded: 0,
                reward_sum: 0,
                budget,
                remaining: budget,
                reserved: 0,
                accepted_unpaid: 0,
                paid: 0,
                refunded: 0,
                next_index: 0,
                active_index: v4::NONE,
                deadline,
                state: v4::DRAFT,
                paused: false,
            };
            write(&a[1], FACTORY_TAG, &f)?;
            save(&a[0], b"FLYGV002", &g)
        }
        Operation::Append { descriptor } => {
            require(a.len() == 5)?;
            let mut f = factory(&a[0], p)?;
            authority(&f, &a[3])?;
            require(Clock::get()?.unix_timestamp < f.deadline)?;
            let index = checked(f.append(&descriptor))?;
            create(
                p,
                &a[1],
                &a[3],
                &a[4],
                v4::TASK_SIZE,
                p,
                &[b"task-v4", a[0].key.as_ref(), &index.to_le_bytes()],
            )?;
            // A permanent marker prevents paying the same scientific identity
            // in a second V4 catalog, including after expiry or catalog closure.
            create(
                p,
                &a[2],
                &a[3],
                &a[4],
                v4::SCIENCE_SIZE,
                p,
                &[b"task-science-v4", &descriptor.scientific_identity],
            )?;
            write(
                &a[2],
                SCIENCE_TAG,
                &(descriptor.scientific_identity, a[0].key.to_bytes(), index),
            )?;
            write(
                &a[1],
                TASK_TAG,
                &Task::new(a[0].key.to_bytes(), index, descriptor),
            )?;
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Seal => {
            require(a.len() == 2)?;
            let mut f = factory(&a[0], p)?;
            authority(&f, &a[1])?;
            require(Clock::get()?.unix_timestamp < f.deadline)?;
            checked(f.seal())?;
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Advance => {
            require(a.len() == 2)?;
            let mut f = factory(&a[0], p)?;
            if f.active_index != v4::NONE {
                // A stale public retry is harmless.
                let t = task(&a[1], p, a[0].key)?;
                return require(t.index == f.active_index && t.state == v4::ACTIVE);
            }
            require(f.state == v4::RUNNING && !f.paused)?;
            successor(p, &a[0], &mut f, Some(&a[1]), Clock::get()?.unix_timestamp)?;
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Commit {
            commitment,
            beneficiary,
        } => {
            require(
                a.len() == 6 && commitment != v4::ZERO && beneficiary != v4::ZERO && a[3].is_signer,
            )?;
            let f = factory(&a[0], p)?;
            let mut t = task(&a[1], p, a[0].key)?;
            let expected = Pubkey::find_program_address(
                &[
                    b"submission-v4",
                    a[1].key.as_ref(),
                    &t.attempt.to_le_bytes(),
                    a[3].key.as_ref(),
                    &commitment,
                ],
                p,
            )
            .0;
            require(*a[2].key == expected)?;
            if !a[2].data_is_empty() {
                let s = submission(&a[2], p, a[1].key)?;
                return require(
                    s.attempt == t.attempt
                        && s.miner == a[3].key.to_bytes()
                        && s.commitment == commitment
                        && s.beneficiary == beneficiary,
                );
            }
            require(f.state == v4::RUNNING && !f.paused && f.active_index == t.index)?;
            let clock = Clock::get()?;
            let sequence = checked(t.enqueue(a[2].key.to_bytes(), clock.unix_timestamp))?;
            create(
                p,
                &a[2],
                &a[4],
                &a[5],
                v4::SUBMISSION_SIZE,
                p,
                &[
                    b"submission-v4",
                    a[1].key.as_ref(),
                    &t.attempt.to_le_bytes(),
                    a[3].key.as_ref(),
                    &commitment,
                ],
            )?;
            let s = Submission {
                task: a[1].key.to_bytes(),
                attempt: t.attempt,
                sequence,
                miner: a[3].key.to_bytes(),
                beneficiary,
                commitment,
                commit_slot: clock.slot,
                evidence: v4::Evidence {
                    candidate: v4::ZERO,
                    result: v4::ZERO,
                    manifest: v4::ZERO,
                    archive: v4::ZERO,
                },
                state: v4::COMMITTED,
                valid_votes: 0,
                invalid_votes: 0,
                reports: [v4::ZERO; 3],
            };
            write(&a[2], SUBMISSION_TAG, &s)?;
            write(&a[1], TASK_TAG, &t)
        }
        Operation::Reveal { evidence, salt } => {
            require(a.len() == 4 && a[3].is_signer && salt != v4::ZERO)?;
            checked(evidence.validate())?;
            let f = factory(&a[0], p)?;
            let mut t = task(&a[1], p, a[0].key)?;
            let mut s = submission(&a[2], p, a[1].key)?;
            require(
                s.attempt == t.attempt
                    && s.miner == a[3].key.to_bytes()
                    && v4::commitment(
                        p.to_bytes(),
                        &f,
                        a[1].key.to_bytes(),
                        s.attempt,
                        s.miner,
                        s.beneficiary,
                        &evidence,
                        salt,
                    ) == s.commitment,
            )?;
            if s.state != v4::COMMITTED {
                return require(s.evidence == evidence);
            }
            let c = Clock::get()?;
            require(
                t.state == v4::ACTIVE
                    && c.unix_timestamp <= t.reveal_deadline
                    && c.slot > s.commit_slot,
            )?;
            s.evidence = evidence;
            s.state = v4::REVEALED;
            checked(t.entry(a[2].key.to_bytes()))?.state = v4::REVEALED;
            write(&a[2], SUBMISSION_TAG, &s)?;
            write(&a[1], TASK_TAG, &t)
        }
        Operation::Vote {
            valid,
            report,
            complete_trials,
            ed25519_index,
        } => {
            require(a.len() == 4 || a.len() == 5)?;
            let mut f = factory(&a[0], p)?;
            let mut t = task(&a[1], p, a[0].key)?;
            let mut s = submission(&a[2], p, a[1].key)?;
            require(
                s.attempt == t.attempt
                    && complete_trials as usize == t.descriptor.seeds.len()
                    && [v4::REVEALED, v4::VALID, v4::INVALID].contains(&s.state),
            )?;
            let msg = v4::vote_message(
                p.to_bytes(),
                &f,
                &t,
                a[2].key.to_bytes(),
                &s,
                valid,
                report,
                complete_trials,
            );
            let signer = signature(&a[3], ed25519_index, &msg)?;
            let index = f.verifiers.iter().position(|k| *k == signer).ok_or(bad())?;
            if (s.valid_votes | s.invalid_votes) & (1 << index) != 0 {
                checked(s.vote(index, valid, report))?;
                return Ok(());
            }
            let now = Clock::get()?.unix_timestamp;
            require(t.state == v4::ACTIVE && now <= t.verify_deadline)?;
            let state = checked(s.vote(index, valid, report))?;
            checked(t.entry(a[2].key.to_bytes()))?.state = state;
            write(&a[2], SUBMISSION_TAG, &s)?;
            t.prune(now);
            if state == v4::VALID
                && t.queue
                    .first()
                    .is_some_and(|e| e.submission == a[2].key.to_bytes())
            {
                checked(t.accept(&mut f, a[2].key.to_bytes(), s.beneficiary, now))?;
                solana_program::msg!("TASK_V4_ACCEPTED {} {}", t.index, t.attempt);
            }
            write(&a[1], TASK_TAG, &t)?;
            successor(p, &a[0], &mut f, a.get(4), now)?;
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Finalize => {
            require(a.len() == 3 || a.len() == 4)?;
            let mut f = factory(&a[0], p)?;
            let mut t = task(&a[1], p, a[0].key)?;
            let s = submission(&a[2], p, a[1].key)?;
            require(
                s.attempt == t.attempt && s.state == v4::VALID && s.valid_votes.count_ones() >= 2,
            )?;
            let now = Clock::get()?.unix_timestamp;
            checked(t.accept(&mut f, a[2].key.to_bytes(), s.beneficiary, now))?;
            write(&a[1], TASK_TAG, &t)?;
            successor(p, &a[0], &mut f, a.get(3), now)?;
            solana_program::msg!("TASK_V4_ACCEPTED {} {}", t.index, t.attempt);
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Expire => {
            require(a.len() == 2 || a.len() == 3)?;
            let mut f = factory(&a[0], p)?;
            let mut t = task(&a[1], p, a[0].key)?;
            let now = Clock::get()?.unix_timestamp;
            checked(t.expire(&mut f, now))?;
            write(&a[1], TASK_TAG, &t)?;
            successor(p, &a[0], &mut f, a.get(2), now)?;
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Pay => {
            require(a.len() == 7)?;
            let g = global(&a[0], p)?;
            let mut f = factory(&a[1], p)?;
            let mut t = task(&a[2], p, a[1].key)?;
            let s = submission(&a[3], p, a[2].key)?;
            require(
                g.issued
                    && f.genesis == g.network
                    && f.policy == g.policy
                    && f.mint == g.mint
                    && t.winner == a[3].key.to_bytes()
                    && t.beneficiary == s.beneficiary
                    && *a[4].key == pool_key(p, 0),
            )?;
            token(&a[4], &g.mint, &a[0].key.to_bytes())?;
            token(&a[5], &g.mint, &t.beneficiary)?;
            if checked(t.pay(&mut f))? {
                transfer(p, &a[0], &a[4], &a[5], &a[6], t.descriptor.reward_atoms)?;
                write(&a[2], TASK_TAG, &t)?;
                write(&a[1], FACTORY_TAG, &f)?;
            }
            Ok(())
        }
        Operation::Pause { paused } => {
            require(a.len() == 2)?;
            let mut f = factory(&a[0], p)?;
            authority(&f, &a[1])?;
            require(f.state != v4::CLOSED)?;
            f.paused = paused;
            write(&a[0], FACTORY_TAG, &f)
        }
        Operation::Close => {
            require(a.len() == 3)?;
            let mut g = global(&a[0], p)?;
            let mut f = factory(&a[1], p)?;
            authority(&f, &a[2])?;
            require(f.policy == g.policy && f.genesis == g.network)?;
            if f.state == v4::CLOSED {
                return Ok(());
            }
            require(
                f.active_index == v4::NONE
                    && f.reserved == 0
                    && (f.state == v4::DRAFT
                        || f.next_index == f.count
                        || Clock::get()?.unix_timestamp > f.deadline),
            )?;
            // The historical budget remains auditable after unused funds are
            // returned; accepted_unpaid is never released or invalidated.
            g.available[0] = g.available[0].checked_add(f.remaining).ok_or(bad())?;
            require(g.available[0] <= POOLS[0])?;
            f.refunded = f.refunded.checked_add(f.remaining).ok_or(bad())?;
            f.remaining = 0;
            f.state = v4::CLOSED;
            checked(f.accounting())?;
            write(&a[1], FACTORY_TAG, &f)?;
            save(&a[0], b"FLYGV002", &g)
        }
    }
}
