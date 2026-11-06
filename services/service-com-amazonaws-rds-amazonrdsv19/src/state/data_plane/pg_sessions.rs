// SPDX-License-Identifier: CC0-1.0

//! The sessions one DB instance's engine endpoint holds OPEN, and what each of them is
//! doing.
//!
//! The engines report their own live connections — PostgreSQL through `pg_stat_activity`,
//! MySQL through its connection count — and they BOUND them: `max_connections` is the
//! parameter that decides how many a server admits at once. Both need the same fact, the
//! set of sessions this endpoint currently serves, so it is kept once, by the endpoint
//! itself, rather than inferred from anything.
//!
//! A session registers when it is admitted and is removed when it ends, on every path
//! including a severed connection, because the entry is dropped by the session's own
//! teardown guard. Nothing here is reported for a session that is not open.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

/// One live session on an engine endpoint, as the engine reports it.
#[derive(Debug, Clone)]
pub struct LiveSession {
    /// The PostgreSQL backend pid / MySQL connection id of this session.
    pub pid: u32,
    /// The role or account the session authenticated as.
    pub user: String,
    /// The database the session is connected to.
    pub database: String,
    /// The `application_name` the client's startup carried, empty where it sent none.
    pub application_name: String,
    /// The address the client connected from, as the session's own socket gives it.
    pub client_address: String,
    /// When the session was admitted, in the engine's timestamp text form.
    pub backend_start: String,
    /// What the session is doing: `active` while a statement of it runs, `idle` between
    /// statements — PostgreSQL's own `pg_stat_activity.state` values.
    pub state: String,
    /// The statement the session is running, or the last one it ran.
    pub query: String,
    /// When that statement started.
    pub query_start: Option<String>,
}

/// The sessions open on one endpoint, by pid, shared by the endpoint and every session on
/// it.
#[derive(Debug, Clone, Default)]
pub struct SessionRegistry {
    sessions: Arc<Mutex<BTreeMap<u32, LiveSession>>>,
}

/// `pg_stat_activity.state` for a session that is running a statement.
pub const STATE_ACTIVE: &str = "active";
/// `pg_stat_activity.state` for a session between statements.
pub const STATE_IDLE: &str = "idle";

impl SessionRegistry {
    /// Take the entries under the lock, recovering one a panicking session poisoned: a
    /// long-lived emulator has to keep serving every later session.
    fn entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, LiveSession>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many sessions are open right now, which is what `max_connections` bounds.
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.entries().len()
    }

    /// Record a session that has just been admitted.
    pub fn admitted(&self, session: LiveSession) {
        self.entries().insert(session.pid, session);
    }

    /// Record what the session `pid` is running now.
    pub fn running(&self, pid: u32, query: &str, started: String) {
        if let Some(session) = self.entries().get_mut(&pid) {
            session.state = STATE_ACTIVE.to_string();
            session.query = query.to_string();
            session.query_start = Some(started);
        }
    }

    /// Record that the session `pid` has finished its statement.
    pub fn idle(&self, pid: u32) {
        if let Some(session) = self.entries().get_mut(&pid) {
            session.state = STATE_IDLE.to_string();
        }
    }

    /// Forget the session `pid`, which has ended.
    pub fn ended(&self, pid: u32) {
        self.entries().remove(&pid);
    }

    /// Every session open right now, ordered by pid — what `pg_stat_activity` reports.
    #[must_use]
    pub fn open_sessions(&self) -> Vec<LiveSession> {
        self.entries().values().cloned().collect()
    }
}
