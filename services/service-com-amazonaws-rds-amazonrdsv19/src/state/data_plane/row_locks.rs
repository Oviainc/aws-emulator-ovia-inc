// SPDX-License-Identifier: CC0-1.0

//! The ROW LOCKS the transactions open on one engine endpoint hold, and the two refusals a
//! client reads off them.
//!
//! Both engines lock at the level of the ROW: a transaction that writes a row, or that asks
//! for it with `SELECT … FOR UPDATE`, holds that row until it ends, and another
//! transaction that wants the same row waits for it. What the second transaction is told
//! when it cannot have the row is the engine's own condition:
//!
//! * it asked not to wait (`NOWAIT`) — PostgreSQL `lock_not_available`, MySQL
//!   `ER_LOCK_NOWAIT`;
//! * waiting would close a CYCLE of transactions each waiting for the other, which no
//!   amount of waiting resolves: the engine breaks the cycle by refusing exactly ONE of
//!   them — PostgreSQL `deadlock_detected`, MySQL `ER_LOCK_DEADLOCK` — and lets the rest
//!   proceed.
//!
//! Which rows a statement locks is [`super::lock_plan`]'s reading of the statement; this
//! module owns only WHO HOLDS WHAT and who waits for whom. The waiting is done on a
//! condition variable, so a transaction that ends wakes every transaction waiting for one
//! of its rows, and every wait is bounded by the deadline the caller's statement already
//! has — the engine never parks a request forever.
//!
//! One table per engine endpoint ([`super::sql_engine::InstanceDatabase`]), so two DB
//! instances cannot see each other's locks, and the session ids the holders are named by
//! are the `connection_id` of [`super::wire_session::SessionFacts`] — a PostgreSQL backend
//! pid, a MySQL connection id.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Which row a lock is on: the database, the table (lowercased, as the engines resolve a
/// table name case-insensitively here) and the engine's own row id.
pub type RowKey = (String, String, i64);

/// How long one wait for a row sleeps before it looks at the lock table again. The wake-up
/// a released row sends is what normally ends a wait; this only bounds the wait when the
/// holder went away without one.
const WAIT_SLICE: Duration = Duration::from_millis(25);

/// How many waiting transactions a cycle search walks before it gives up: a bound on the
/// search, not on the number of transactions.
const MAX_WAIT_CHAIN: usize = 1024;

/// How many ROWS this endpoint's transactions may hold locked at once.
///
/// Every locked row is an entry in the table below, and a transaction keeps its rows until
/// it ends, so a session that never commits would otherwise grow this store with every
/// statement it runs. This is what bounds it: a statement that would take the table past the
/// ceiling is refused ([`LockOutcome::TooMany`]) instead of growing the process, which is the
/// same answer the engine gives a statement that asks for more than this endpoint serves.
pub const MAX_HELD_ROWS: usize = 200_000;

/// What a lock is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    /// A read that must not be written under it: `FOR SHARE`, `LOCK IN SHARE MODE`.
    Shared,
    /// A row this transaction is writing, or has reserved for writing: `FOR UPDATE`,
    /// `UPDATE`, `DELETE`.
    Exclusive,
}

/// What the statement asked to happen when the row is already held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPolicy {
    /// Wait for the holder, which is what a statement that names nothing does.
    Wait,
    /// `NOWAIT`: refuse instead of waiting.
    NoWait,
    /// `SKIP LOCKED`: leave the held rows out of the result.
    SkipLocked,
}

/// What came of asking for a set of rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockOutcome {
    /// The rows this transaction now holds — every row asked for, or under `SKIP LOCKED`
    /// the ones that were free.
    Granted(Vec<RowKey>),
    /// A row is held by another transaction and the statement asked not to wait.
    Unavailable,
    /// Waiting would close a cycle, so this transaction is the one the engine refuses.
    Deadlocked,
    /// The statement's own deadline passed while it waited.
    TimedOut,
    /// The rows this statement asks to hold would take this endpoint past
    /// [`MAX_HELD_ROWS`].
    TooMany,
}

/// Who holds one row.
#[derive(Debug, Clone)]
enum Holder {
    /// One transaction, for writing.
    Exclusive(u32),
    /// Every transaction reading it under a shared lock.
    Shared(BTreeSet<u32>),
}

impl Holder {
    /// Whether `session` may take this row in `mode` — a row it already holds exclusively
    /// is its own, and a shared row only it holds can be taken exclusively (the upgrade a
    /// transaction that reads then writes a row makes).
    fn admits(&self, session: u32, mode: LockMode) -> bool {
        match (self, mode) {
            (Self::Exclusive(holder), _) => *holder == session,
            // Two shared readers of one row do not conflict, which is what a shared lock is.
            (Self::Shared(_), LockMode::Shared) => true,
            (Self::Shared(holders), LockMode::Exclusive) => {
                holders.iter().all(|holder| *holder == session)
            }
        }
    }

    /// The transaction a waiter would be waiting for.
    fn blocker(&self, session: u32) -> Option<u32> {
        match self {
            Self::Exclusive(holder) if *holder != session => Some(*holder),
            Self::Shared(holders) => holders.iter().find(|holder| **holder != session).copied(),
            Self::Exclusive(_) => None,
        }
    }
}

/// The rows the transactions on one endpoint hold, and the transaction each waiting one
/// waits for.
#[derive(Debug, Default)]
pub struct LockTable {
    state: Mutex<State>,
    /// Signalled whenever a row is released, which is what ends a wait.
    released: Condvar,
}

#[derive(Debug, Default)]
struct State {
    held: BTreeMap<RowKey, Holder>,
    /// waiter -> the transaction it is waiting for. The WAIT-FOR GRAPH: a cycle in it is a
    /// deadlock.
    waiting: BTreeMap<u32, u32>,
}

impl LockTable {
    /// The lock table, recovering a lock a panicking session poisoned: a long-lived
    /// emulator has to keep serving every later session.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take `rows` for `session` in `mode`, answering what the engine answers when they
    /// cannot all be had.
    ///
    /// All or nothing while waiting: the rows are granted only once NONE of them is held by
    /// another transaction, so a statement cannot hold half its rows while it waits for the
    /// rest — which is what would turn two statements over one table into a cycle of their
    /// own. `SKIP LOCKED` is the exception the clause itself defines: it takes the free rows
    /// and reports those.
    pub fn acquire(
        &self,
        session: u32,
        rows: &[RowKey],
        mode: LockMode,
        policy: WaitPolicy,
        deadline: Instant,
    ) -> LockOutcome {
        if rows.is_empty() {
            return LockOutcome::Granted(Vec::new());
        }
        let mut state = self.state();
        loop {
            if would_hold_too_many(&state, rows) {
                return LockOutcome::TooMany;
            }
            match blocked_by(&state, session, rows, mode) {
                None => {
                    grant(&mut state, session, rows, mode);
                    return LockOutcome::Granted(rows.to_vec());
                }
                Some(blocker) => match policy {
                    WaitPolicy::NoWait => return LockOutcome::Unavailable,
                    WaitPolicy::SkipLocked => {
                        let free: Vec<RowKey> = rows
                            .iter()
                            .filter(|row| {
                                state
                                    .held
                                    .get(*row)
                                    .is_none_or(|holder| holder.admits(session, mode))
                            })
                            .cloned()
                            .collect();
                        grant(&mut state, session, &free, mode);
                        return LockOutcome::Granted(free);
                    }
                    WaitPolicy::Wait => {
                        if waits_on(&state, blocker, session) {
                            return LockOutcome::Deadlocked;
                        }
                        let now = Instant::now();
                        if now >= deadline {
                            return LockOutcome::TimedOut;
                        }
                        state.waiting.insert(session, blocker);
                        let slice = (deadline - now).min(WAIT_SLICE);
                        let (waited, _) = self
                            .released
                            .wait_timeout(state, slice)
                            .unwrap_or_else(PoisonError::into_inner);
                        state = waited;
                        state.waiting.remove(&session);
                    }
                },
            }
        }
    }

    /// Let go of every row `session` holds — what COMMIT, ROLLBACK and the end of a session
    /// do — and wake whoever was waiting for one of them.
    pub fn release(&self, session: u32) {
        let mut state = self.state();
        state.held.retain(|_, holder| match holder {
            Holder::Exclusive(holder) => *holder != session,
            Holder::Shared(holders) => {
                holders.remove(&session);
                !holders.is_empty()
            }
        });
        state.waiting.remove(&session);
        drop(state);
        self.released.notify_all();
    }

    /// Let go of `rows` only, which is what a statement OUTSIDE a transaction does: its
    /// locks live for the statement, not for a transaction it is not in.
    pub fn release_rows(&self, session: u32, rows: &[RowKey]) {
        if rows.is_empty() {
            return;
        }
        let mut state = self.state();
        for row in rows {
            let drop_row = match state.held.get_mut(row) {
                Some(Holder::Exclusive(holder)) => *holder == session,
                Some(Holder::Shared(holders)) => {
                    holders.remove(&session);
                    holders.is_empty()
                }
                None => false,
            };
            if drop_row {
                state.held.remove(row);
            }
        }
        drop(state);
        self.released.notify_all();
    }

    /// How many rows are held right now, for the tests that pin that a transaction's locks
    /// go when it does.
    #[must_use]
    pub fn held_rows(&self) -> usize {
        self.state().held.len()
    }
}

/// Whether taking `rows` would take the table past [`MAX_HELD_ROWS`]. Counted over the rows
/// the table does not already hold, so a statement asking again for rows its transaction
/// already holds is not refused for them.
fn would_hold_too_many(state: &State, rows: &[RowKey]) -> bool {
    let fresh = rows
        .iter()
        .filter(|row| !state.held.contains_key(*row))
        .count();
    state.held.len().saturating_add(fresh) > MAX_HELD_ROWS
}

/// The transaction holding the first of `rows` that `session` cannot have in `mode`.
fn blocked_by(state: &State, session: u32, rows: &[RowKey], mode: LockMode) -> Option<u32> {
    rows.iter().find_map(|row| {
        state
            .held
            .get(row)
            .filter(|holder| !holder.admits(session, mode))
            .and_then(|holder| holder.blocker(session))
    })
}

/// Record `session` as the holder of `rows` in `mode`.
fn grant(state: &mut State, session: u32, rows: &[RowKey], mode: LockMode) {
    for row in rows {
        match mode {
            LockMode::Exclusive => {
                state.held.insert(row.clone(), Holder::Exclusive(session));
            }
            LockMode::Shared => match state.held.get_mut(row) {
                Some(Holder::Shared(holders)) => {
                    holders.insert(session);
                }
                Some(Holder::Exclusive(_)) => {}
                None => {
                    state
                        .held
                        .insert(row.clone(), Holder::Shared([session].into()));
                }
            },
        }
    }
}

/// Whether `from` is already waiting, directly or through other waiters, for `target`:
/// that is the cycle a new wait by `target` on `from` would close.
fn waits_on(state: &State, from: u32, target: u32) -> bool {
    let mut at = from;
    for _ in 0..MAX_WAIT_CHAIN {
        if at == target {
            return true;
        }
        match state.waiting.get(&at) {
            Some(next) => at = *next,
            None => return false,
        }
    }
    false
}
