// SPDX-License-Identifier: CC0-1.0

//! The transaction ONE SESSION has open, and what putting its rows back means.
//!
//! A transaction belongs to the session that opened it: `BEGIN` … `COMMIT` brackets the
//! statements of THAT connection, two sessions hold two independent transactions at once,
//! and one session's `ROLLBACK` undoes its own work and nobody else's. The embedded engine
//! (SQLite) runs one transaction per database at a time, which is why the transaction a
//! session opens cannot be the engine's. It is kept here instead, out of two facts the
//! ENGINE itself reports:
//!
//! * the state the database was in before the transaction's first statement — a
//!   page-for-page copy made by the engine's own online backup, which is what the rows go
//!   back to;
//! * WHICH rows the transaction has changed since, from the engine's update hook, which
//!   reports every row an `INSERT`, `UPDATE` or `DELETE` touched, including the rows a
//!   cascading foreign-key action or a trigger touched with it.
//!
//! A `ROLLBACK` then puts exactly those rows back as that copy holds them, and a `COMMIT`
//! drops the copy. Nothing here reads a statement's text to work out what it did.
//!
//! The limits of keeping a transaction this way, both of them stated rather than hidden:
//!
//! * a row a session has written but not committed is already in the database, so another
//!   session's statement reads it. The rows a transaction writes are locked for it
//!   ([`super::row_locks`]), so no other transaction can write them until it ends, and its
//!   own rollback puts them back — but it is the holder's own uncommitted value that another
//!   session reads in between;
//! * what goes back is ROWS. A table a transaction created or dropped is not put back by its
//!   rollback, because the copy is of the rows and the schema the transaction's own
//!   statements changed is the schema the client is left with.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use rusqlite::types::Value;
use rusqlite::Connection;

use super::sql_engine::{EngineError, EngineErrorKind};

/// How many distinct rows ONE transaction may hold uncommitted, across every database it
/// touched.
///
/// Each one is a row a `ROLLBACK` has to put back, so this is what bounds the bookkeeping
/// one session can make the engine hold by never committing. Past it the recording stops and
/// the transaction reports that it can no longer be rolled back, rather than quietly putting
/// back only some of its rows.
pub const MAX_UNCOMMITTED_ROWS: usize = 200_000;

/// How many uncommitted rows the transactions of ONE ENDPOINT may hold between them.
///
/// The bound above is one transaction's; this one is what keeps the bookkeeping of many
/// sessions from being that bound times the number of sessions. Past it a transaction stops
/// recording and reports that it can no longer be rolled back, the same way it does at its
/// own ceiling.
pub const MAX_UNCOMMITTED_ROWS_PER_ENDPOINT: usize = 500_000;

/// One row the engine changed: the table it is in and the engine's own row id for it.
pub type ChangedRow = (String, i64);

/// The open transactions of one engine endpoint's sessions, shared with the update hook
/// installed on every one of its databases' connections.
#[derive(Debug, Default)]
pub struct OpenTransactions {
    state: Mutex<Transactions>,
}

#[derive(Debug, Default)]
struct Transactions {
    /// The session whose statement is running right now, where it has a transaction open:
    /// the one the engine's row changes are recorded against. Statements run one at a time
    /// under the engine's own lock, so one value is enough.
    recording: Option<u32>,
    open: BTreeMap<u32, Transaction>,
    /// How many rows the open transactions hold between them, which is what
    /// [`MAX_UNCOMMITTED_ROWS_PER_ENDPOINT`] bounds.
    rows: usize,
}

/// One session's transaction.
#[derive(Debug, Default)]
struct Transaction {
    /// The rows it has changed, by database.
    changed: BTreeMap<String, BTreeSet<ChangedRow>>,
    /// Each of those databases as it stood before this transaction's first statement in it.
    before: BTreeMap<String, Connection>,
    /// How many rows `changed` holds in all, which is what [`MAX_UNCOMMITTED_ROWS`] bounds:
    /// the count is the transaction's, not one database's.
    rows: usize,
    /// How many engine PAGES those copies hold, which is what the endpoint's own budget for
    /// uncommitted state is counted in.
    pages: i64,
    /// Whether the recording hit [`MAX_UNCOMMITTED_ROWS`] and stopped.
    overflowed: bool,
}

/// What a transaction that has ended held: the rows to put back, the database each of them
/// goes back to as it was, and whether the recording of them is complete.
#[derive(Debug, Default)]
pub struct ClosedTransaction {
    pub changed: BTreeMap<String, BTreeSet<ChangedRow>>,
    pub before: BTreeMap<String, Connection>,
    pub overflowed: bool,
}

impl OpenTransactions {
    /// The transactions, recovering a lock a panicking session poisoned: a long-lived
    /// emulator has to keep serving every later session.
    fn state(&self) -> MutexGuard<'_, Transactions> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open a transaction for `session`, answering whether it had none open already.
    pub fn open(&self, session: u32) -> bool {
        let mut state = self.state();
        if state.open.contains_key(&session) {
            return false;
        }
        state.open.insert(session, Transaction::default());
        true
    }

    /// Whether `session` has a transaction open.
    #[must_use]
    pub fn is_open(&self, session: u32) -> bool {
        self.state().open.contains_key(&session)
    }

    /// Whether `session`'s transaction already holds the database `database` as it was.
    #[must_use]
    pub fn holds_before(&self, session: u32, database: &str) -> bool {
        self.state()
            .open
            .get(&session)
            .is_some_and(|open| open.before.contains_key(database))
    }

    /// Keep `copy` as the state the database `database` was in before `session`'s
    /// transaction touched it, charging the `pages` it holds to that transaction.
    pub fn keep_before(&self, session: u32, database: &str, copy: Connection, pages: i64) {
        if let Some(open) = self.state().open.get_mut(&session) {
            open.before.insert(database.to_string(), copy);
            open.pages = open.pages.saturating_add(pages);
        }
    }

    /// How many engine pages the states the OPEN transactions hold add up to right now,
    /// which is what the endpoint's budget for uncommitted state is checked against.
    #[must_use]
    pub fn charged_pages(&self) -> i64 {
        self.state()
            .open
            .values()
            .map(|open| open.pages)
            .fold(0i64, i64::saturating_add)
    }

    /// End `session`'s transaction and answer what it held, or `None` where it had none
    /// open.
    pub fn close(&self, session: u32) -> Option<ClosedTransaction> {
        let mut state = self.state();
        if state.recording == Some(session) {
            state.recording = None;
        }
        state.open.remove(&session).map(|open| {
            state.rows = state.rows.saturating_sub(open.rows);
            ClosedTransaction {
                changed: open.changed,
                before: open.before,
                overflowed: open.overflowed,
            }
        })
    }

    /// The sessions holding a transaction open right now.
    #[must_use]
    pub fn open_sessions(&self) -> Vec<u32> {
        self.state().open.keys().copied().collect()
    }

    /// Record the rows the statement about to run changes against `session`'s transaction,
    /// where it has one; `None` for a statement whose changes belong to no transaction — a
    /// statement outside one, the server's own statements, and the rollback itself.
    pub fn recording_for(&self, session: Option<u32>) {
        let mut state = self.state();
        state.recording = session.filter(|session| state.open.contains_key(session));
    }

    /// Record the rows `rows` of `database` as changed by `session`'s transaction, whatever
    /// is recording right now.
    ///
    /// What the engine's own hook does not report: a `DELETE` of every row of a table is
    /// served by the engine's truncate path, which changes the rows without reporting them
    /// one by one, so the statement's OWN rows are recorded here as well — they are the rows
    /// it wrote.
    pub fn note_rows_for(
        &self,
        session: u32,
        database: &str,
        rows: impl IntoIterator<Item = ChangedRow>,
    ) {
        let mut state = self.state();
        let endpoint_rows = state.rows;
        let Some(open) = state.open.get_mut(&session) else {
            return;
        };
        let changed = open.changed.entry(database.to_string()).or_default();
        let mut added = 0usize;
        let mut overflowed = false;
        for row in rows {
            let at_its_own_ceiling = open.rows + added >= MAX_UNCOMMITTED_ROWS;
            let at_the_endpoints = endpoint_rows + added >= MAX_UNCOMMITTED_ROWS_PER_ENDPOINT;
            if (at_its_own_ceiling || at_the_endpoints) && !changed.contains(&row) {
                overflowed = true;
                break;
            }
            if changed.insert(row) {
                added += 1;
            }
        }
        open.rows += added;
        open.overflowed = open.overflowed || overflowed;
        state.rows = endpoint_rows.saturating_add(added);
    }

    /// Record the rows `rows` of `database` as changed by whichever transaction is
    /// recording.
    pub fn note_rows(&self, database: &str, rows: impl IntoIterator<Item = ChangedRow>) {
        let session = self.state().recording;
        if let Some(session) = session {
            self.note_rows_for(session, database, rows);
        }
    }
}

/// Install the recording hook on `conn`, the connection the database `database` runs on.
///
/// Every row the engine changes on this connection is reported to `transactions`, which
/// keeps it only while a transaction is recording ([`OpenTransactions::recording_for`]).
pub fn record_changes_of(conn: &Connection, database: &str, transactions: &Arc<OpenTransactions>) {
    let transactions = Arc::clone(transactions);
    let database = database.to_string();
    conn.update_hook(Some(
        move |_action, _schema: &str, table: &str, rowid: i64| {
            transactions.note_rows(&database, [(table.to_string(), rowid)]);
        },
    ));
}

/// Put the rows `rows` of one database back as `before` holds them, on the live connection
/// `live`.
///
/// Two passes, because a row's old value can be the value another of these rows holds now:
/// every one of them is removed first, then the ones the database held before are put back.
/// It runs as ONE engine transaction with the foreign-key checks deferred to its end, so a
/// row that is the parent of another need not be the first one back.
///
/// Bounded by `deadline`, which the caller also installed on the engine itself: the restore
/// gives up — leaving the engine transaction rolled back — rather than running past the
/// budget a rollback gets, because this runs while the endpoint's engine is held.
///
/// A table that is not in both ends is left alone: one the transaction created is not in
/// `before`, and one it dropped is no longer live — the schema a transaction changed is the
/// schema the client is left with (see this module's own notes).
pub fn put_back(
    live: &Connection,
    before: &Connection,
    rows: &BTreeSet<ChangedRow>,
    deadline: Instant,
) -> Result<(), EngineError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut by_table: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (table, rowid) in rows {
        by_table.entry(table.as_str()).or_default().push(*rowid);
    }
    live.execute_batch("BEGIN; PRAGMA defer_foreign_keys = 1")
        .map_err(internal)?;
    match restore_each(live, before, &by_table, deadline) {
        Ok(()) => live.execute_batch("COMMIT").map_err(internal),
        Err(error) => {
            let _ = live.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// The two passes of [`put_back`], under the transaction it opened.
fn restore_each(
    live: &Connection,
    before: &Connection,
    by_table: &BTreeMap<&str, Vec<i64>>,
    deadline: Instant,
) -> Result<(), EngineError> {
    for (table, rowids) in by_table {
        let columns = column_names(before, table);
        if columns.is_empty() || column_names(live, table).is_empty() {
            continue;
        }
        let quoted_table = quoted(table);
        let images = row_images(before, &quoted_table, &columns, rowids, deadline)?;
        for rowid in rowids {
            out_of_time(deadline)?;
            live.execute(
                &format!("DELETE FROM {quoted_table} WHERE rowid = ?1"),
                [rowid],
            )
            .map_err(internal)?;
        }
        if images.is_empty() {
            continue;
        }
        let mut names: Vec<String> = vec!["rowid".to_string()];
        names.extend(columns.iter().map(|name| quoted(name)));
        let placeholders: Vec<String> = (1..=names.len()).map(|at| format!("?{at}")).collect();
        let statement = format!(
            "INSERT INTO {quoted_table} ({}) VALUES ({})",
            names.join(", "),
            placeholders.join(", ")
        );
        for (rowid, values) in images {
            out_of_time(deadline)?;
            let mut bound: Vec<Value> = vec![Value::Integer(rowid)];
            bound.extend(values);
            live.execute(&statement, rusqlite::params_from_iter(bound))
                .map_err(internal)?;
        }
    }
    Ok(())
}

/// Whether the rollback's budget has run out, as the error that says so.
fn out_of_time(deadline: Instant) -> Result<(), EngineError> {
    if Instant::now() >= deadline {
        return Err(EngineError::new(
            EngineErrorKind::Cancelled,
            "putting a transaction's rows back took longer than this endpoint serves",
        ));
    }
    Ok(())
}

/// The rows `rowids` as `before` holds them: the ones that are there, with their values in
/// the table's own column order.
fn row_images(
    before: &Connection,
    quoted_table: &str,
    columns: &[String],
    rowids: &[i64],
    deadline: Instant,
) -> Result<Vec<(i64, Vec<Value>)>, EngineError> {
    let mut statement = before
        .prepare(&format!("SELECT * FROM {quoted_table} WHERE rowid = ?1"))
        .map_err(internal)?;
    let mut images = Vec::new();
    for rowid in rowids {
        out_of_time(deadline)?;
        let mut rows = statement.query([rowid]).map_err(internal)?;
        let Some(row) = rows.next().map_err(internal)? else {
            continue;
        };
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            values.push(row.get::<usize, Value>(index).map_err(internal)?);
        }
        images.push((*rowid, values));
    }
    Ok(images)
}

/// The columns of `table` on `conn`, in the engine's own order, or none for a table that
/// connection does not hold.
fn column_names(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(statement) = conn.prepare(&format!("SELECT * FROM {} LIMIT 0", quoted(table))) else {
        return Vec::new();
    };
    statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// One identifier, quoted so a name that is also a keyword reaches the engine as a name.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A rollback the engine itself refused, which the caller reports as a failure of this
/// emulator rather than as the client's.
fn internal(error: rusqlite::Error) -> EngineError {
    EngineError::new(EngineErrorKind::Internal, error.to_string())
}
