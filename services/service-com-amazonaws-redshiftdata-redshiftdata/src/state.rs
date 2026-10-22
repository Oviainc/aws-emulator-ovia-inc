// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-redshiftdata-redshiftdata service.
//!
//! The Redshift Data API is an HTTP control plane in front of a real SQL engine:
//! `ExecuteStatement` submits one SQL statement and answers immediately with an
//! `Id`, and the caller then polls `DescribeStatement` and fetches rows with
//! `GetStatementResult`. Everything this module holds exists to make that
//! lifecycle real:
//!
//! * [`Statement`] — one submitted statement and everything the API reports
//!   about it. `ExecuteStatement` writes it; `DescribeStatement`,
//!   `GetStatementResult`, `ListStatements` and `CancelStatement` all read it.
//!   A `BatchExecuteStatement` submission is one of these too, holding one per
//!   statement it submitted in [`Statement::sub_statements`] — a batch's
//!   statement has its own text, status, outcome and `Id` (`<batch id>:<n>`),
//!   so it is the same thing as any other statement rather than a second kind.
//! * [`Database`] — the SQL engine itself. One in-process SQLite database
//!   (`rusqlite`, a real engine, not a hand-written SQL subset) per
//!   connection target and database name, so a `CREATE TABLE` submitted by one
//!   statement is visible to the `SELECT` submitted by the next, exactly as a
//!   Redshift database is. A batch's statements run serially on that one
//!   connection, which is what lets [`ExecutionMode::Transaction`] commit or
//!   roll back all of them together.
//!
//! The asynchronous status timeline is modelled with a stored due instant and
//! never with a sleep: a statement is `SUBMITTED` when `ExecuteStatement`
//! returns and the clock owes it its terminal status [`COMPLETION_SPAN`] later,
//! so a caller that cancels straight away still finds a cancellable statement.
//! Every operation materialises the transitions the clock has already made due
//! before it reads.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{ColumnMetadata, Field, SqlParameter};

/// How long after `ExecuteStatement` the statement's move out of `SUBMITTED`
/// falls due.
///
/// A real cluster takes anywhere from milliseconds to hours; what matters for an
/// emulator is that the span is never zero, so a caller acting immediately after
/// `ExecuteStatement` still observes the transient `SUBMITTED` state and the
/// state-dependent behaviour that goes with it (`CancelStatement` succeeds,
/// `GetStatementResult` refuses). This is an emulator-chosen span, not a
/// duration Redshift documents.
///
/// The SQL itself runs during `ExecuteStatement`, and only the STATUS is owed
/// later: the execution is not deferred to a background task because a result
/// nothing has computed cannot be reported honestly, and because a task
/// outliving its request would run on an engine no request is bounding. A caller
/// that sends no `WaitTimeSeconds` therefore still pays the statement's
/// execution latency, which the engine's own deadline bounds.
pub const COMPLETION_SPAN: Duration = Duration::from_millis(300);

/// Page size `ListStatements` uses when the request leaves `MaxResults` at its
/// modelled default of 0 — the modelled maximum, so an unparameterised call
/// returns as much as one page can hold.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// Rows `GetStatementResult` returns in one page. Redshift documents no page
/// size for this operation, so this is an emulator-chosen bound; it only
/// decides where `NextToken` appears, never which rows exist.
pub const RESULT_PAGE_ROWS: usize = 1_000;

/// Wall-clock bound on one statement's execution. The SQL engine runs
/// in-process with no isolation boundary, so a statement that never terminates
/// would take the whole emulator down rather than just its own request.
const SQL_TIMEOUT: Duration = Duration::from_secs(2);

/// Step budget for one statement's execution, counted in SQLite virtual-machine
/// instructions. A wall-clock timeout cannot preempt a CPU-bound query running
/// on the request thread; this can.
const SQL_STEP_BUDGET: u64 = 20_000_000;

/// How much longer than [`SQL_TIMEOUT`] a request waits when it named no
/// `WaitTimeSeconds`: enough for a statement queued behind one other statement
/// on the same database, and no more.
const SQL_QUEUE_GRACE: Duration = Duration::from_secs(3);

/// How many virtual-machine instructions pass between two consecutive checks of
/// the timeout and the step budget.
const PROGRESS_HANDLER_OPS: i32 = 1_000;

/// Cap on the number of rows one submission's results retain.
pub const MAX_RESULT_ROWS: usize = 100_000;

/// Cap on the number of VALUES (rows times columns) one submission's results
/// retain. A cell costs memory whatever it holds, so a wide projection of
/// constants is bounded by this even though it charges almost nothing in bytes.
const MAX_RESULT_CELLS: usize = 2_000_000;

/// Cap on the MATERIALISED size of one SUBMISSION's results, in bytes. Held
/// low enough that [`MAX_RETAINED_STATEMENTS`] of them still fit in the
/// emulator's own memory. A batch's statements share this one budget, so a
/// request cannot retain more by submitting more statements.
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// What one retained cell costs beyond its payload: the `Field` union itself.
/// Charged against [`MAX_RESULT_BYTES`] so a NULL or integer cell is never free.
const RETAINED_CELL_OVERHEAD: usize = std::mem::size_of::<Field>();

/// What one retained row costs beyond its cells: the row's own vector.
const RETAINED_ROW_OVERHEAD: usize = std::mem::size_of::<Vec<Field>>();

/// Worst-case expansion of a text payload: `String::from_utf8_lossy` replaces
/// each invalid byte with a three-byte replacement character, so a text value is
/// charged at three times its engine size.
const LOSSY_TEXT_EXPANSION: usize = 3;

/// `SQLITE_LIMIT_LENGTH`: the largest single string or blob the engine will
/// produce, so one value cannot be larger than the whole result budget.
const MAX_VALUE_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_SQL_LENGTH`: the largest statement text the engine will accept.
const MAX_SQL_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_COLUMN`: the widest row the engine will produce.
const MAX_COLUMNS: i32 = 1_000;

/// `SQLITE_LIMIT_EXPR_DEPTH`: the depth bound on a parsed expression tree, so a
/// deeply nested expression cannot recurse the parser off its stack.
const MAX_EXPR_DEPTH: i32 = 100;

/// `SQLITE_LIMIT_COMPOUND_SELECT`: how many `UNION`/`EXCEPT`/`INTERSECT` terms
/// one statement may chain.
const MAX_COMPOUND_SELECT: i32 = 50;

/// `SQLITE_LIMIT_VARIABLE_NUMBER`: how many parameters one statement may bind.
const MAX_BOUND_PARAMETERS: i32 = 999;

/// `PRAGMA max_page_count`: the engine's own storage bound, in pages, applied to
/// BOTH schemas a statement can write — `main` and `temp`. At SQLite's default
/// 4 KiB page this is 16 MiB each, so a statement that builds a huge table, or a
/// huge TEMP table, runs out of database rather than out of the host's memory.
const MAX_DATABASE_PAGES: i64 = 4_096;

/// `PRAGMA hard_heap_limit`: the ceiling on what the SQL engine may allocate.
/// The page counts above bound each schema; this bounds the allocator itself, so
/// aggregate engine memory is capped however many databases are open and
/// whatever a statement does inside one.
const MAX_ENGINE_HEAP_BYTES: i64 = 256 * 1024 * 1024;

/// How many distinct connection targets and database names may be open at once.
/// The key is caller-chosen, so without this bound a caller could make the
/// emulator open connections and handles without limit.
pub const MAX_OPEN_DATABASES: usize = 16;

/// How long an unused database is kept open. A database no statement has touched
/// for this long is closed, which releases the connection and everything a
/// statement committed to it — otherwise memory a single statement allocated
/// inside the engine would be held for the life of the process. The span is the
/// same as [`STATEMENT_RETENTION`], so a database is closed only once nothing
/// that ran on it is readable any more.
const DATABASE_IDLE_TIMEOUT: Duration = STATEMENT_RETENTION;

/// How many statements are retained. An emulator-chosen memory bound on top of
/// [`STATEMENT_RETENTION`]: each retained statement holds its result set, so the
/// per-statement caps above only bind if the number retained is bounded too.
///
/// It counts STATEMENTS, not submissions: a batch counts as itself plus each of
/// its statements, because each of those holds a result set of its own. Since
/// one submission's results share a single [`MAX_RESULT_BYTES`] budget (see
/// `Retention`), this caps the retained result data at
/// `MAX_RETAINED_STATEMENTS` times [`MAX_RESULT_BYTES`] whatever mix of single
/// statements and batches a caller sends. The oldest submissions are evicted
/// first.
pub const MAX_RETAINED_STATEMENTS: usize = 100;

/// How long a statement stays readable. The Amazon Redshift Data API
/// documentation states that "the Data API retains the result set for 24 hours"
/// after a statement runs, so a statement older than that is no longer readable
/// on the real service either.
const STATEMENT_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// SQL functions this emulator refuses even though the engine's own build may
/// carry them: each one exists to reach something outside the database.
const DENIED_SQL_FUNCTIONS: &[&str] = &[
    "load_extension",
    "readfile",
    "writefile",
    "fsdir",
    "edit",
    "fts3_tokenizer",
];

/// The first process identifier handed to a database connection. Redshift
/// reports a backend process id (`RedshiftPid`); the value only has to be a
/// stable, unique long per connection.
const FIRST_PID: i64 = 1_000;

// ---------------------------------------------------------------------------
// Statement lifecycle
// ---------------------------------------------------------------------------

/// The statement lifecycle states this emulator produces.
///
/// The wire spellings are the ones `StatementStatusString` declares
/// (`shared_types::STATEMENT_STATUS_STRING_VALUES`): ABORTED, FAILED, FINISHED,
/// PICKED, STARTED, SUBMITTED. A statement here is SUBMITTED until the clock
/// owes its outcome and then FINISHED, FAILED or ABORTED; PICKED and STARTED
/// describe a real cluster's scheduling queue, which this emulator does not
/// model, so no statement ever reports them (a `ListStatements` filter naming
/// one therefore matches nothing, which is correct rather than an error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementStatus {
    /// The query was submitted, but not yet processed.
    Submitted,
    /// The query has finished running.
    Finished,
    /// The query run failed.
    Failed,
    /// The query run was stopped by the user.
    Aborted,
}

/// How a submission's statements are committed.
///
/// The wire spellings are the ones `ExecutionMode` declares
/// (`shared_types::EXECUTION_MODE_VALUES`): TRANSACTION and AUTO_COMMIT. The
/// model documents what each one does: "If set to TRANSACTION (the default),
/// all SQL statements are run as a single transaction and they are committed or
/// rolled back together. If set to AUTO_COMMIT, each SQL statement is committed
/// individually, and a failure of one statement does not affect the others."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// The statements are committed or rolled back together.
    Transaction,
    /// Each statement is committed on its own.
    AutoCommit,
}

impl ExecutionMode {
    /// The wire value, as `ExecutionMode` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transaction => "TRANSACTION",
            Self::AutoCommit => "AUTO_COMMIT",
        }
    }

    /// The mode a request's `ExecutionMode` names, or `None` for a value the
    /// model does not declare.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "TRANSACTION" => Some(Self::Transaction),
            "AUTO_COMMIT" => Some(Self::AutoCommit),
            _ => None,
        }
    }
}

impl StatementStatus {
    /// The wire value, as `StatementStatusString` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::Finished => "FINISHED",
            Self::Failed => "FAILED",
            Self::Aborted => "ABORTED",
        }
    }

    /// Whether the statement has stopped moving on its own.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Submitted)
    }
}

/// What a `CancelStatement` request found when it asked for a statement to be
/// stopped. Carries the status the store OBSERVED, so the caller never has to
/// look the statement up a second time to describe the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cancellation {
    /// This call stopped a statement that was still running.
    Cancelled,
    /// The statement had already stopped, in this status, so this call
    /// cancelled nothing.
    AlreadyStopped(StatementStatus),
    /// The `Id` names ONE statement of a batch, which is not a query that runs
    /// on its own: its batch is what a caller cancels. Carries the batch's
    /// `Id`.
    BatchMember(String),
    /// No retained statement carries that `Id`.
    Unknown,
}

/// The rows and column metadata one statement produced.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    /// One entry per column, in result order.
    pub column_metadata: Vec<ColumnMetadata>,
    /// One entry per row; each row has one [`Field`] per column.
    pub rows: Vec<Vec<Field>>,
}

/// Which data warehouse a statement was submitted against. Redshift accepts a
/// provisioned cluster or a serverless workgroup, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// A provisioned cluster, by `ClusterIdentifier`.
    Cluster(String),
    /// A serverless workgroup, by `WorkgroupName`.
    Workgroup(String),
}

impl ConnectionTarget {
    /// The `ClusterIdentifier` to report, present only for a provisioned cluster.
    pub fn cluster_identifier(&self) -> Option<String> {
        match self {
            Self::Cluster(id) => Some(id.clone()),
            Self::Workgroup(_) => None,
        }
    }

    /// The `WorkgroupName` to report, present only for a serverless workgroup.
    pub fn workgroup_name(&self) -> Option<String> {
        match self {
            Self::Cluster(_) => None,
            Self::Workgroup(name) => Some(name.clone()),
        }
    }

    /// The key identifying this target's `database` in [`Database`] storage.
    ///
    /// Every caller-chosen part is LENGTH-PREFIXED rather than joined by a
    /// separator, so no two distinct (target, database) pairs can produce one
    /// key whatever characters the names carry — two different warehouses must
    /// never share one engine.
    fn database_key(&self, database: &str) -> String {
        let (kind, name) = match self {
            Self::Cluster(id) => ("cluster", id.as_str()),
            Self::Workgroup(name) => ("workgroup", name.as_str()),
        };
        format!(
            "{kind}:{}:{name}:{}:{database}",
            name.len(),
            database.len()
        )
    }
}

/// One submitted SQL statement, and everything the Data API reports about it.
///
/// A batch submission is held as one statement too, with one of these per
/// statement it submitted in [`Statement::sub_statements`]: a batch's statement
/// is a statement in its own right — it has its own text, status, outcome and
/// `Id` (`<batch id>:<n>`) and is read back through the same operations — so it
/// is the same type rather than a parallel one.
#[derive(Debug)]
pub struct Statement {
    /// The `Id` minted at submit, a UUID as the `UUID` shape's pattern requires;
    /// for one statement of a batch, that batch's `Id` with the `:<n>` suffix
    /// the same pattern allows.
    pub id: String,
    /// `RedshiftQueryUuid`: the query the cluster ran. A batch is not itself a
    /// query — its statements are, and each one carries its own — so a batch has
    /// none.
    pub query_uuid: Option<String>,
    /// Monotonic submission sequence number. `ListStatements` orders by it and
    /// its pagination cursor names it, so a page boundary survives later
    /// submissions.
    pub seq: u64,
    /// The SQL text as submitted, reported as `QueryString`. Empty for a batch,
    /// whose statements each carry their own text.
    sql: String,
    /// The caller's `StatementName`, if any.
    pub statement_name: Option<String>,
    /// The database the statement ran against.
    pub database: Option<String>,
    /// The `DbUser` the caller authenticated as, when using temporary credentials.
    pub db_user: Option<String>,
    /// The `SecretArn` the caller authenticated with, when using Secrets Manager.
    pub secret_arn: Option<String>,
    /// The cluster or workgroup the statement ran against.
    pub target: ConnectionTarget,
    /// Whether this is a batch submission (`IsBatchStatement`), whose statements
    /// are [`Statement::sub_statements`].
    pub is_batch: bool,
    /// How this submission's statements were committed (`ExecutionMode`).
    execution_mode: ExecutionMode,
    /// The statements of a batch, in submission order. Empty for a single
    /// statement.
    pub sub_statements: Vec<Statement>,
    /// The status as of the last materialised transition.
    pub status: StatementStatus,
    /// The status the clock owes this statement at [`Statement::due_at`].
    terminal_status: StatementStatus,
    /// When that owed transition falls due, on the monotonic clock.
    due_at: Instant,
    /// The wall-clock instant of that owed transition, reported as `UpdatedAt`.
    completed_at: SystemTime,
    /// The engine's own error text, once the statement has FAILED.
    error: Option<String>,
    /// `CreatedAt`.
    pub created_at: SystemTime,
    /// `UpdatedAt`.
    pub updated_at: SystemTime,
    /// Measured execution time in nanoseconds, reported as `Duration`.
    duration_nanos: i64,
    /// `RedshiftPid`: the connection's backend process id.
    pub redshift_pid: i64,
    /// `RedshiftQueryId`.
    pub redshift_query_id: i64,
    /// Whether the statement produced a result set.
    has_result_set: bool,
    /// `ResultRows`: rows returned, or rows affected for a DML statement.
    result_rows: i64,
    /// `ResultSize`: the result set's size in bytes, or -1 when there is none.
    result_size: i64,
    /// Whether the caller asked for an EventBridge event.
    pub with_event: bool,
    /// `ResultFormat`, defaulted to JSON as the model documents.
    pub result_format: String,
    /// The `Parameters` the statement was submitted with.
    pub parameters: Vec<SqlParameter>,
    /// The session the statement ran in, when the caller used one.
    pub session_id: Option<String>,
    /// The `ClientToken` this statement was submitted with, so the idempotency
    /// map can be pruned together with the statement it points at.
    client_token: Option<String>,
    /// The rows the statement produced, if any.
    result: Option<ResultSet>,
}

impl Statement {
    /// Materialise the transition the clock owes this statement, if `now` has
    /// reached its due instant. Idempotent, and a no-op for a statement that is
    /// already terminal: a read calls this so that a status the clock made due
    /// is reported even when nothing else has touched the statement since.
    fn advance(&mut self, now: Instant) {
        // A batch's statements share its due instant, so one call materialises
        // the whole batch's timeline.
        for sub_statement in &mut self.sub_statements {
            sub_statement.advance(now);
        }
        if self.status.is_terminal() || now < self.due_at {
            return;
        }
        self.status = self.terminal_status;
        self.updated_at = self.completed_at;
    }

    /// The `Error` to report: the engine's own message, and only once the
    /// statement has actually failed.
    pub fn error(&self) -> Option<String> {
        match self.status {
            StatementStatus::Failed => self.error.clone(),
            _ => None,
        }
    }

    /// `Duration` — nanoseconds the statement ran. Zero until it stops running.
    pub fn duration_nanos(&self) -> i64 {
        if self.status.is_terminal() {
            self.duration_nanos
        } else {
            0
        }
    }

    /// `HasResultSet`. Not known while the statement is still running, so it is
    /// reported only once the statement is terminal.
    pub fn has_result_set(&self) -> Option<bool> {
        self.status
            .is_terminal()
            .then_some(self.has_result_set && self.status == StatementStatus::Finished)
    }

    /// `ResultRows`. Zero until the statement stops running; -1 (the modelled
    /// "null" marker) when it stopped without producing a count.
    pub fn result_rows(&self) -> i64 {
        match self.status {
            StatementStatus::Submitted => 0,
            StatementStatus::Finished => self.result_rows,
            StatementStatus::Failed | StatementStatus::Aborted => -1,
        }
    }

    /// `ResultSize`. Same shape as [`Statement::result_rows`].
    pub fn result_size(&self) -> i64 {
        match self.status {
            StatementStatus::Submitted => 0,
            StatementStatus::Finished => self.result_size,
            StatementStatus::Failed | StatementStatus::Aborted => -1,
        }
    }

    /// The result set, readable only once the statement FINISHED.
    pub fn result(&self) -> Option<&ResultSet> {
        match self.status {
            StatementStatus::Finished => self.result.as_ref(),
            _ => None,
        }
    }

    /// `ExecutionMode`. A single `ExecuteStatement` is committed on its own,
    /// which is what the modelled `AUTO_COMMIT` means; a batch reports the mode
    /// it was submitted with.
    pub fn execution_mode(&self) -> &'static str {
        self.execution_mode.as_str()
    }

    /// `QueryString`: the text of one statement. A batch has no single query
    /// string — its statements are reported as `QueryStrings` and as
    /// `SubStatements` — so it reports none.
    pub fn query_string(&self) -> Option<String> {
        (!self.is_batch).then(|| self.sql.clone())
    }

    /// `QueryStrings`: the texts of a batch's statements, in submission order.
    /// A single statement reports its text as `QueryString` instead.
    pub fn query_strings(&self) -> Option<Vec<String>> {
        self.is_batch.then(|| {
            self.sub_statements
                .iter()
                .map(|sub_statement| sub_statement.sql.clone())
                .collect()
        })
    }

    /// Stop a still-running statement, as `CancelStatement` does. Returns false
    /// when the statement had already stopped, which is not a cancellation.
    ///
    /// The statement's result set is discarded, because an aborted query has no
    /// rows to fetch. Its WRITES, however, are not undone: the SQL ran during
    /// `ExecuteStatement` and the engine committed it there, so a cancelled
    /// `CREATE TABLE` or `INSERT` reports ABORTED while its effect stays visible
    /// to the next statement on that database. That is the cost of running the
    /// statement during the call that submits it, which is in turn what keeps
    /// the reported result something the engine actually produced; a real
    /// cluster, which cancels work still in flight, does not have it.
    fn cancel(&mut self, now_wall: SystemTime) -> bool {
        if self.status.is_terminal() {
            return false;
        }
        // Cancelling a batch stops every statement of it: they are one
        // submission, and none of them is still running after this call.
        for sub_statement in &mut self.sub_statements {
            sub_statement.cancel(now_wall);
        }
        self.status = StatementStatus::Aborted;
        self.terminal_status = StatementStatus::Aborted;
        self.updated_at = now_wall;
        self.duration_nanos = now_wall
            .duration_since(self.created_at)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_nanos()).ok())
            .unwrap_or(0);
        // The rows the engine produced are discarded: a cancelled statement has
        // no result set to fetch.
        self.result = None;
        self.has_result_set = false;
        true
    }
}

// ---------------------------------------------------------------------------
// The SQL engine
// ---------------------------------------------------------------------------

/// The bound ONE execution runs under: a deadline, a remaining step budget and
/// an abandon flag, read by the connection's progress handler.
#[derive(Debug, Default)]
struct ExecutionGuard {
    deadline: Mutex<Option<Instant>>,
    steps_left: AtomicU64,
    abandoned: std::sync::atomic::AtomicBool,
}

impl ExecutionGuard {
    /// Arm the guard for one execution.
    fn arm(&self, timeout: Duration, steps: u64) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now() + timeout);
        self.steps_left.store(steps, Ordering::SeqCst);
    }

    /// Disarm the guard once the execution has returned, so an idle connection
    /// is never interrupted.
    fn disarm(&self) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Abandon this execution: the engine stops it at its next progress
    /// callback, and it stops before it starts if it has not started yet — the
    /// flag outlives [`ExecutionGuard::arm`] rather than being overwritten by it.
    fn abandon(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
    }

    /// Whether SQLite should abort the running statement. Called from the
    /// progress handler every [`PROGRESS_HANDLER_OPS`] instructions.
    fn should_interrupt(&self) -> bool {
        if self.abandoned.load(Ordering::SeqCst) {
            return true;
        }
        let deadline = *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(deadline) = deadline else {
            return false;
        };
        if Instant::now() >= deadline {
            return true;
        }
        let spent = u64::try_from(PROGRESS_HANDLER_OPS).unwrap_or(1);
        // The budget is compared AFTER this callback's own spend, so the
        // statement is interrupted on the callback that exhausts it rather than
        // on the one after — and a `saturating_sub` that lands past zero is
        // caught rather than missed.
        let previous = self
            .steps_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                Some(left.saturating_sub(spent))
            })
            .unwrap_or(0);
        previous.saturating_sub(spent) == 0
    }
}

/// The bound for ONE execution, created before that execution starts.
///
/// Per execution and never per database: the handler makes one, keeps an
/// [`Interrupt`] for it, and hands the bound to the blocking task, so
/// abandoning one request's statement can never abandon another request's.
#[derive(Debug)]
pub struct ExecutionBound(std::sync::Arc<ExecutionGuard>);

impl ExecutionBound {
    /// A fresh bound for one execution.
    pub fn new() -> Self {
        Self(std::sync::Arc::new(ExecutionGuard::default()))
    }

    /// A handle that abandons THIS execution and no other.
    pub fn interrupt(&self) -> Interrupt {
        Interrupt(std::sync::Arc::clone(&self.0))
    }
}

impl Default for ExecutionBound {
    fn default() -> Self {
        Self::new()
    }
}

/// A handle that asks one execution to be abandoned at the engine's next
/// progress callback — or before it starts, if it has not started yet.
#[derive(Clone, Debug)]
pub struct Interrupt(std::sync::Arc<ExecutionGuard>);

impl Interrupt {
    /// Abandon the execution now.
    pub fn now(&self) {
        self.0.abandon();
    }
}

/// One database of one data warehouse: a real SQL engine running in-process,
/// confined before it ever sees caller SQL (see [`Database::open`]).
///
/// Neither the connection's `RedshiftPid` nor any execution bound is held here:
/// the pid is immutable and lives on the state's own entry (`OpenDatabase`), so
/// a handler can resolve a database without taking this engine's lock — which is
/// held for a whole execution — and each execution brings its OWN
/// [`ExecutionBound`], so no bound outlives the statement it belongs to.
#[derive(Debug)]
pub struct Database {
    connection: rusqlite::Connection,
}

impl Database {
    /// Open a fresh in-memory database, confine it, and install its execution
    /// bound.
    ///
    /// The engine is a real SQL engine, not a purpose-built tree walker, so the
    /// statements it would otherwise accept include ones that act on the HOST
    /// rather than on this database: `ATTACH DATABASE '<path>'` creates, opens or
    /// reads an arbitrary host file, `VACUUM INTO '<path>'` writes one, and a
    /// `PRAGMA` can turn the engine's own switches back on. Every one of those is
    /// closed here, before any request reaches `prepare`, and the order matters:
    /// the emulator's own `PRAGMA`s run first, because the authorizer installed
    /// at the end refuses `PRAGMA` from then on.
    fn open() -> rusqlite::Result<Self> {
        // Opened WITHOUT `SQLITE_OPEN_URI`: a URI filename is another way a
        // caller-chosen string could name a file, and nothing here needs it.
        // `SQLITE_OPEN_MEMORY` keeps the main database off the filesystem.
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_MEMORY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = rusqlite::Connection::open_in_memory_with_flags(flags)?;

        // `temp_store = MEMORY`: a large sort or hash must not spill a file into
        // the host's temp directory. `max_page_count` on BOTH `main` and `temp`:
        // the pragma applies to the schema it is issued against, and
        // `temp_store = MEMORY` puts every temporary object on the process heap,
        // so bounding `main` alone would leave a `CREATE TEMP TABLE ... AS`
        // unbounded. `hard_heap_limit` bounds the engine's allocator itself, so
        // aggregate engine memory is capped whatever a statement does inside one
        // database and however many databases are open.
        connection.execute_batch(&format!(
            "PRAGMA temp_store = MEMORY;
             PRAGMA hard_heap_limit = {MAX_ENGINE_HEAP_BYTES};
             PRAGMA main.max_page_count = {MAX_DATABASE_PAGES};
             PRAGMA temp.max_page_count = {MAX_DATABASE_PAGES};"
        ))?;

        // Extension loading stays off DELIBERATELY, not incidentally: rusqlite's
        // `load_extension` feature is not enabled, so neither
        // `sqlite3_enable_load_extension` nor `Connection::load_extension` is
        // reachable from this crate at all, and the authorizer below denies the
        // `load_extension()` SQL function by name as well.
        // Defensive mode: no direct writes to the schema or to shadow tables.
        connection.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        // The schema is not trusted: a view or trigger stored in the database
        // cannot reach a privileged SQL function.
        connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
            false,
        )?;

        // Engine limits, so a single statement cannot outgrow the bounds this
        // module enforces around it.
        use rusqlite::limits::Limit;
        // No database may be attached at all, which closes `ATTACH` even if the
        // authorizer below were ever bypassed.
        connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COLUMN, MAX_COLUMNS)?;
        connection.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPR_DEPTH)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT)?;
        connection.set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_BOUND_PARAMETERS)?;

        // Default-deny: every action a Data API statement legitimately needs is
        // named, and anything else — including an action a future engine version
        // introduces — is refused.
        connection.authorizer(Some(authorize));

        Ok(Self { connection })
    }
}

/// The engine's default-deny authorizer: what a submitted statement may do.
fn authorize(context: rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        // The host-filesystem escapes, denied by name. `ATTACH`/`DETACH` open an
        // arbitrary path; `PRAGMA` is also how `VACUUM INTO '<path>'` is
        // authorized, and how the switches set in `Database::open` would be
        // turned back on.
        AuthAction::Attach { .. } | AuthAction::Detach { .. } | AuthAction::Pragma { .. } => {
            Authorization::Deny
        }
        // A virtual table module is an arbitrary extension point, some of whose
        // implementations read the filesystem.
        AuthAction::CreateVtable { .. } | AuthAction::DropVtable { .. } => Authorization::Deny,
        // A function whose whole purpose is to reach outside the database.
        AuthAction::Function { function_name, .. }
            if DENIED_SQL_FUNCTIONS
                .iter()
                .any(|denied| function_name.eq_ignore_ascii_case(denied)) =>
        {
            Authorization::Deny
        }
        // The SQL a Data API statement legitimately runs: query, modify and
        // define objects inside this one database.
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Reindex { .. }
        | AuthAction::Analyze { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Recursive
        | AuthAction::Function { .. } => Authorization::Allow,
        // Anything else, including an action this engine version does not yet
        // name: refused rather than allowed by omission.
        _ => Authorization::Deny,
    }
}

/// What one execution produced. Opaque to a handler, which only carries it from
/// [`run_on`] or [`run_batch_on`] to
/// [`ComAmazonawsRedshiftdataRedshiftdataState::record`].
#[derive(Debug)]
pub struct Execution {
    status: StatementStatus,
    error: Option<String>,
    duration_nanos: i64,
    has_result_set: bool,
    result_rows: i64,
    result_size: i64,
    result: Option<ResultSet>,
}

impl Execution {
    /// A statement that produced nothing because the engine refused it, with
    /// the engine's own message for why.
    fn failed(message: String) -> Self {
        Self {
            status: StatementStatus::Failed,
            error: Some(message),
            duration_nanos: 0,
            has_result_set: false,
            result_rows: -1,
            result_size: -1,
            result: None,
        }
    }

    /// A statement of a batch the run never reached: an earlier statement of
    /// the same transaction failed, so the batch stopped there and everything
    /// it had done was rolled back.
    ///
    /// It reports ABORTED — the only modelled status for a run that stopped
    /// without finishing or failing — and no `Error`, because this statement
    /// never reached the engine and so has no message from it.
    fn never_ran() -> Self {
        Self {
            status: StatementStatus::Aborted,
            error: None,
            duration_nanos: 0,
            has_result_set: false,
            result_rows: -1,
            result_size: -1,
            result: None,
        }
    }

    /// Report a statement whose work did not land after all, because the
    /// transaction holding it could not be closed. Its rows are dropped with
    /// it: a result computed inside a transaction that was never committed is
    /// not a result a caller may fetch.
    fn mark_uncommitted(&mut self, message: &str) {
        if self.status != StatementStatus::Finished {
            return;
        }
        self.status = StatementStatus::Failed;
        self.error = Some(message.to_string());
        self.has_result_set = false;
        self.result_rows = -1;
        self.result_size = -1;
        self.result = None;
    }
}

/// What ONE submission may still retain of its results: the rows, the values
/// and the bytes left of its budget.
///
/// The budget belongs to the SUBMISSION, not to one execution: a batch's
/// statements all draw on one of these, so a batch cannot retain more result
/// data than a single `ExecuteStatement` can, whatever it submits and however
/// many statements it submits. [`MAX_RESULT_BYTES`] therefore bounds what one
/// REQUEST can add to the retained history, which is what makes
/// [`MAX_RETAINED_STATEMENTS`] a bound on the whole of it.
///
/// Only a result that is retained is charged: a statement that fails retains
/// nothing, so it spends nothing, and a statement with no result set spends
/// nothing either.
#[derive(Debug)]
struct Retention {
    rows_left: usize,
    cells_left: usize,
    bytes_left: usize,
}

impl Retention {
    /// A full budget, for one submission.
    fn new() -> Self {
        Self {
            rows_left: MAX_RESULT_ROWS,
            cells_left: MAX_RESULT_CELLS,
            bytes_left: MAX_RESULT_BYTES,
        }
    }

    /// Charge a result set this submission is keeping against the budget.
    fn spend(&mut self, rows: usize, cells: usize, bytes: usize) {
        self.rows_left = self.rows_left.saturating_sub(rows);
        self.cells_left = self.cells_left.saturating_sub(cells);
        self.bytes_left = self.bytes_left.saturating_sub(bytes);
    }
}

/// Which of a submission's named `Parameters` are bound to one statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// Every parameter is bound, so one the statement does not reference is the
    /// engine's own error: `ExecuteStatement`'s `Parameters` are "the parameters
    /// for the SQL statement", that one statement's.
    Every,
    /// Only the parameters the statement references are bound: a batch's
    /// `Parameters` "are available to all SQL statements in the batch. Each
    /// statement can reference any subset of the provided parameters."
    Referenced,
}

/// Either the rows a query returned, or the row count a DML/DDL statement
/// affected. SQLite reports a statement that returns no columns as having none,
/// which is exactly the Data API's `HasResultSet=false` case.
enum Produced {
    Rows(ResultSet, i64),
    Affected(i64),
}

/// Why one execution did not produce a complete result.
enum ExecutionFailure {
    /// The engine refused, interrupted or could not run the statement.
    Engine(rusqlite::Error),
    /// The result set reached one of this emulator's retention bounds. The
    /// statement FAILS rather than reporting a truncated result as a complete
    /// one: a caller must never be handed a short answer that looks whole.
    Bound(String),
}

impl From<rusqlite::Error> for ExecutionFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Engine(error)
    }
}

impl ExecutionFailure {
    /// The message to report as the statement's `Error`.
    fn message(&self) -> String {
        match self {
            Self::Engine(error) => scrub_paths(&error.to_string()),
            Self::Bound(message) => message.clone(),
        }
    }
}

/// Run `sql` on the engine `handle` guards.
///
/// BLOCKING, deliberately: the engine is synchronous, so a caller must put this
/// on a blocking pool (`tokio::task::spawn_blocking`) rather than run it on an
/// async worker. It takes only this one database's lock, never the service's
/// state lock, so one caller's slow query serialises nothing but its own
/// database.
pub fn run_on(
    handle: &DatabaseHandle,
    sql: &str,
    parameters: &[SqlParameter],
    timeout: Duration,
    bound: &ExecutionBound,
) -> Execution {
    let database = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut budget = Retention::new();
    run_sql(
        &database,
        sql,
        parameters,
        Binding::Every,
        &mut budget,
        timeout,
        bound,
    )
}

/// Run `sqls` serially, in submission order, on the engine `handle` guards, and
/// report one [`Execution`] per submitted statement in that same order.
///
/// BLOCKING for the same reason [`run_on`] is, and it takes the same single
/// database lock — held for the whole batch, which is what lets
/// [`ExecutionMode::Transaction`] run the statements as ONE transaction: they
/// are committed together, or, as soon as one of them fails, the batch stops
/// there and everything it had done is rolled back ("If any SQL statement
/// fails, all work is rolled back"). Under [`ExecutionMode::AutoCommit`] each
/// statement is committed on its own and a failure stops nothing.
///
/// `timeout` bounds the WHOLE batch rather than each statement: each statement
/// is given what is left of it, so a batch of forty statements cannot outlast
/// the request that is waiting for it.
pub fn run_batch_on(
    handle: &DatabaseHandle,
    sqls: &[String],
    parameters: &[SqlParameter],
    mode: ExecutionMode,
    timeout: Duration,
    bound: &ExecutionBound,
) -> Vec<Execution> {
    let database = handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let deadline = Instant::now() + timeout;
    let transactional = mode == ExecutionMode::Transaction;

    // The transaction is opened through the engine itself, under the same
    // authorizer every submitted statement runs under. If it cannot be opened,
    // no statement of the batch ran, and each one reports the engine's own
    // message for why.
    if transactional {
        if let Err(error) = database.connection.execute_batch("BEGIN") {
            let message = scrub_paths(&error.to_string());
            return sqls
                .iter()
                .map(|_| Execution::failed(message.clone()))
                .collect();
        }
    }

    let mut executions: Vec<Execution> = Vec::with_capacity(sqls.len());
    // ONE retained-result budget for the whole batch, so a batch of forty
    // statements cannot keep forty result sets where a single statement may
    // keep one.
    let mut budget = Retention::new();
    for sql in sqls {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let execution = run_sql(
            &database,
            sql,
            parameters,
            Binding::Referenced,
            &mut budget,
            remaining,
            bound,
        );
        let failed = execution.status == StatementStatus::Failed;
        executions.push(execution);
        // Everything this transaction did is about to be rolled back, so there
        // is nothing left for its remaining statements to do.
        if failed && transactional {
            break;
        }
    }

    if transactional {
        let rolled_back = executions
            .iter()
            .any(|execution| execution.status == StatementStatus::Failed);
        let close = if rolled_back { "ROLLBACK" } else { "COMMIT" };
        if let Err(error) = database.connection.execute_batch(close) {
            // The transaction could not be closed, so nothing it held landed:
            // every statement that had succeeded is reported as failed with the
            // engine's message rather than as work that was committed.
            let message = scrub_paths(&error.to_string());
            for execution in &mut executions {
                execution.mark_uncommitted(&message);
            }
        }
        // A transaction left open would leak into the next statement submitted
        // against this database, so it is closed even when the close above
        // reported an error.
        if !database.connection.is_autocommit() {
            let _ = database.connection.execute_batch("ROLLBACK");
        }
    }

    // The statements the batch never reached, because it stopped at the failure
    // above and rolled everything back.
    while executions.len() < sqls.len() {
        executions.push(Execution::never_ran());
    }
    executions
}

/// Run `sql` on `database` under the wall-clock and step bounds, and report what
/// the engine did. A statement the engine rejects or interrupts fails with the
/// engine's own message — the service never guesses at a result it did not get.
fn run_sql(
    database: &Database,
    sql: &str,
    parameters: &[SqlParameter],
    binding: Binding,
    budget: &mut Retention,
    timeout: Duration,
    bound: &ExecutionBound,
) -> Execution {
    // This execution's own bound is installed on the connection for the duration
    // of this execution, replacing the previous one: a bound is never shared
    // between statements, so abandoning one cannot abandon another.
    let guard = std::sync::Arc::clone(&bound.0);
    let handler_guard = std::sync::Arc::clone(&guard);
    database.connection.progress_handler(
        PROGRESS_HANDLER_OPS,
        Some(move || handler_guard.should_interrupt()),
    );

    let started = Instant::now();
    guard.arm(timeout, SQL_STEP_BUDGET);
    let produced = execute_once(&database.connection, sql, parameters, binding, budget);
    guard.disarm();
    let duration_nanos = i64::try_from(started.elapsed().as_nanos()).unwrap_or(i64::MAX);

    match produced {
        Ok(Produced::Rows(result, bytes)) => Execution {
            status: StatementStatus::Finished,
            error: None,
            duration_nanos,
            has_result_set: true,
            result_rows: i64::try_from(result.rows.len()).unwrap_or(i64::MAX),
            result_size: bytes,
            result: Some(result),
        },
        Ok(Produced::Affected(rows)) => Execution {
            status: StatementStatus::Finished,
            error: None,
            duration_nanos,
            has_result_set: false,
            result_rows: rows,
            // No result set, so the size is the modelled null marker.
            result_size: -1,
            result: None,
        },
        Err(failure) => Execution {
            status: StatementStatus::Failed,
            error: Some(failure.message()),
            duration_nanos,
            has_result_set: false,
            result_rows: -1,
            result_size: -1,
            result: None,
        },
    }
}

/// Redact anything path-shaped from an engine message before it goes on the
/// wire. The message is the engine's own and a caller needs it to see why a
/// statement failed, but a host path in it would tell the caller about the
/// machine the emulator runs on.
///
/// Only a token that STARTS a path is redacted — one beginning at the start of
/// the message or after whitespace or a quote, with `/`, `\\` or a `C:\` drive
/// prefix. A slash inside a token, as in the identifier of a syntax error
/// (`near "a/b"`), is left alone, so the diagnostic keeps its detail.
fn scrub_paths(message: &str) -> String {
    let mut scrubbed = String::with_capacity(message.len());
    let mut characters = message.char_indices().peekable();
    let mut at_token_start = true;
    while let Some((index, character)) = characters.next() {
        let rest = &message[index..];
        let path_start = at_token_start
            && (character == '/'
                || rest.starts_with("\\\\")
                || (character.is_ascii_alphabetic() && rest.get(1..3) == Some(":\\")));
        if !path_start {
            scrubbed.push(character);
            at_token_start = character.is_whitespace()
                || character == '\''
                || character == '"'
                || character == '(';
            continue;
        }
        scrubbed.push_str("<path>");
        while characters
            .peek()
            .is_some_and(|(_, next)| !next.is_whitespace() && *next != '\'' && *next != '"')
        {
            characters.next();
        }
        at_token_start = false;
    }
    scrubbed
}

/// Prepare, bind and run one statement on the engine, keeping no more of its
/// result than `budget` — its SUBMISSION's remaining retained-result budget —
/// still allows.
fn execute_once(
    connection: &rusqlite::Connection,
    sql: &str,
    parameters: &[SqlParameter],
    binding: Binding,
    budget: &mut Retention,
) -> Result<Produced, ExecutionFailure> {
    let mut statement = connection.prepare(sql)?;

    // Redshift's `Parameters` are named (`:name`), and so are SQLite's, so the
    // names bind straight through. Which of them this statement gets is
    // [`Binding`]'s decision, and whether a name is in the statement at all is
    // the ENGINE's answer (`parameter_index`), never a scan of the SQL text.
    let names: Vec<String> = parameters
        .iter()
        .map(|parameter| format!(":{}", parameter.name))
        .collect();
    let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::with_capacity(names.len());
    for (name, parameter) in names.iter().zip(parameters.iter()) {
        if binding == Binding::Referenced && statement.parameter_index(name)?.is_none() {
            continue;
        }
        bound.push((name.as_str(), &parameter.value as &dyn rusqlite::ToSql));
    }

    if statement.column_count() == 0 {
        let affected = statement.execute(bound.as_slice())?;
        return Ok(Produced::Affected(i64::try_from(affected).unwrap_or(i64::MAX)));
    }

    // Column names and declared types are read before the query starts, so the
    // immutable borrow of the prepared statement ends before `query` takes it
    // mutably.
    let columns: Vec<(String, Option<String>)> = statement
        .columns()
        .iter()
        .map(|column| {
            (
                column.name().to_string(),
                column.decl_type().map(str::to_ascii_lowercase),
            )
        })
        .collect();
    let declared: Vec<Option<ColumnType>> = columns
        .iter()
        .map(|(_, decl)| decl.as_deref().map(ColumnType::from_declared))
        .collect();

    let mut rows = statement.query(bound.as_slice())?;
    let mut records: Vec<Vec<Field>> = Vec::new();
    // `reported` is the engine's own value size, which is what `ResultSize`
    // means; `retained` is what holding the result actually costs this process,
    // which is what the bound has to measure.
    let mut reported: usize = 0;
    let mut retained: usize = 0;
    let mut cells: usize = 0;
    let mut observed: Vec<Option<ColumnType>> = declared.clone();
    while let Some(row) = rows.next()? {
        if records.len() >= budget.rows_left {
            return Err(bound_reached(format!(
                "the result set exceeded the {} rows this submission may still retain, of this \
                 emulator's bound of {MAX_RESULT_ROWS}",
                budget.rows_left
            )));
        }
        if cells.saturating_add(columns.len()) > budget.cells_left {
            return Err(bound_reached(format!(
                "the result set exceeded the {} values this submission may still retain, of this \
                 emulator's bound of {MAX_RESULT_CELLS}",
                budget.cells_left
            )));
        }

        // Size the row BEFORE materialising any of it. `get_ref` borrows the
        // engine's own buffer and copies nothing, while `field_from` allocates an
        // owned copy of every value, so a row that would take the result set past
        // its budget is measured and then refused rather than allocated — one
        // very large value cannot be materialised by checking after the fact.
        let mut row_reported: usize = 0;
        let mut row_retained: usize = RETAINED_ROW_OVERHEAD;
        for index in 0..columns.len() {
            let value = row.get_ref(index)?;
            row_reported = row_reported.saturating_add(value_bytes(value));
            row_retained = row_retained.saturating_add(retained_bytes(value));
        }
        if retained.saturating_add(row_retained) > budget.bytes_left {
            return Err(bound_reached(format!(
                "the result set exceeded the {} bytes this submission may still retain, of this \
                 emulator's bound of {MAX_RESULT_BYTES}",
                budget.bytes_left
            )));
        }
        reported = reported.saturating_add(row_reported);
        retained += row_retained;
        cells += columns.len();

        let mut record = Vec::with_capacity(columns.len());
        for (index, declared_type) in declared.iter().enumerate() {
            let value = row.get_ref(index)?;
            if observed[index].is_none() {
                observed[index] = ColumnType::from_value(value);
            }
            record.push(field_from(value, declared_type.as_ref()));
        }
        records.push(record);
    }

    let column_metadata = columns
        .iter()
        .zip(observed.iter())
        .map(|((name, _), column_type)| column_metadata(name, column_type.as_ref()))
        .collect();

    // The result is kept, so its cost is charged to the submission — only here,
    // on the path that returns it: a statement that failed above dropped every
    // row it had materialised and so spends nothing.
    budget.spend(records.len(), cells, retained);

    Ok(Produced::Rows(
        ResultSet {
            column_metadata,
            rows: records,
        },
        i64::try_from(reported).unwrap_or(i64::MAX),
    ))
}

/// Report a retention bound as a statement failure.
fn bound_reached(message: String) -> ExecutionFailure {
    ExecutionFailure::Bound(message)
}

/// The byte size one value contributes to `ResultSize` — the engine's own size
/// for that value, which is what the member means.
fn value_bytes(value: rusqlite::types::ValueRef<'_>) -> usize {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => 0,
        ValueRef::Integer(_) => std::mem::size_of::<i64>(),
        ValueRef::Real(_) => std::mem::size_of::<f64>(),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
    }
}

/// What retaining one value actually costs this process: the `Field` union it is
/// stored in plus its owned payload, with text charged at its lossy worst case.
/// A NULL or integer cell is never free, so a wide projection of constants is
/// charged for the memory it really takes.
fn retained_bytes(value: rusqlite::types::ValueRef<'_>) -> usize {
    use rusqlite::types::ValueRef;
    let payload = match value {
        ValueRef::Null | ValueRef::Integer(_) | ValueRef::Real(_) => 0,
        ValueRef::Text(bytes) => bytes.len().saturating_mul(LOSSY_TEXT_EXPANSION),
        ValueRef::Blob(bytes) => bytes.len(),
    };
    RETAINED_CELL_OVERHEAD.saturating_add(payload)
}

/// Map one engine value onto the modelled `Field` union. The column's type
/// decides between `longValue` and `booleanValue`, which SQLite stores
/// identically.
fn field_from(value: rusqlite::types::ValueRef<'_>, column_type: Option<&ColumnType>) -> Field {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Field::isNull(true),
        ValueRef::Integer(integer) => {
            if column_type.is_some_and(|column_type| column_type.name == "bool") {
                Field::booleanValue(integer != 0)
            } else {
                Field::longValue(integer)
            }
        }
        ValueRef::Real(double) => Field::doubleValue(double),
        ValueRef::Text(bytes) => Field::stringValue(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Field::blobValue(bytes.to_vec()),
    }
}

// ---------------------------------------------------------------------------
// Column metadata
// ---------------------------------------------------------------------------

/// A resolved column type: the name `ColumnMetadata.typeName` reports plus the
/// size facts derived from the declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnType {
    /// The value of `ColumnMetadata.typeName`.
    pub name: String,
    /// `ColumnMetadata.length` / `precision`, from a declared `(n)` or `(p,s)`.
    pub length: i32,
    /// `ColumnMetadata.scale`, from a declared `(p,s)`.
    pub scale: i32,
}

/// `ColumnMetadata.typeName` is "The database-specific data type of the column",
/// and Redshift answers with the database's own (PostgreSQL) type name rather
/// than the SQL keyword the column was declared with. This table is that
/// mapping, for the type keywords in the Amazon Redshift *Data types*
/// documentation
/// (<https://docs.aws.amazon.com/redshift/latest/dg/c_Supported_data_types.html>),
/// whose "Data type" / "Aliases" columns give the left-hand side and whose
/// internal names give the right.
///
/// Longest keyword first is not required: lookup is an exact match on the
/// declared keyword with any `(..)` size suffix already stripped.
const DECLARED_TYPE_NAMES: &[(&str, &str)] = &[
    ("smallint", "int2"),
    ("int2", "int2"),
    ("integer", "int4"),
    ("int", "int4"),
    ("int4", "int4"),
    ("bigint", "int8"),
    ("int8", "int8"),
    ("decimal", "numeric"),
    ("numeric", "numeric"),
    ("real", "float4"),
    ("float4", "float4"),
    ("double precision", "float8"),
    ("float8", "float8"),
    ("float", "float8"),
    ("boolean", "bool"),
    ("bool", "bool"),
    ("char", "bpchar"),
    ("character", "bpchar"),
    ("nchar", "bpchar"),
    ("bpchar", "bpchar"),
    ("varchar", "varchar"),
    ("character varying", "varchar"),
    ("nvarchar", "varchar"),
    ("text", "varchar"),
    ("date", "date"),
    ("time", "time"),
    ("time without time zone", "time"),
    ("timetz", "timetz"),
    ("time with time zone", "timetz"),
    ("timestamp", "timestamp"),
    ("timestamp without time zone", "timestamp"),
    ("timestamptz", "timestamptz"),
    ("timestamp with time zone", "timestamptz"),
    ("varbyte", "varbyte"),
    ("varbinary", "varbyte"),
    ("binary varying", "varbyte"),
    ("super", "super"),
];

/// The type names [`ColumnType`] reports for a numeric column, used for
/// `ColumnMetadata.isSigned`. Every Redshift numeric type is signed.
const SIGNED_TYPE_NAMES: &[&str] = &["int2", "int4", "int8", "numeric", "float4", "float8"];

/// The type names [`ColumnType`] reports for a character column. Redshift
/// compares `CHAR` and `VARCHAR` values case-sensitively, which is what
/// `ColumnMetadata.isCaseSensitive` reports.
const CASE_SENSITIVE_TYPE_NAMES: &[&str] = &["bpchar", "varchar", "super"];

impl ColumnType {
    /// Resolve a column's declared type keyword — `VARCHAR(16)`,
    /// `NUMERIC(10,2)`, `BIGINT` — through [`DECLARED_TYPE_NAMES`]. A keyword
    /// the table does not carry keeps its own spelling: it is still the
    /// database-specific name of that column's type.
    fn from_declared(declared: &str) -> Self {
        let declared = declared.trim().to_ascii_lowercase();
        let (keyword, args) = match declared.split_once('(') {
            Some((keyword, rest)) => (keyword.trim(), rest.trim_end_matches(')')),
            None => (declared.as_str(), ""),
        };
        let mut sizes = args
            .split(',')
            .map(|size| size.trim().parse::<i32>().unwrap_or(0));
        let length = sizes.next().unwrap_or(0);
        let scale = sizes.next().unwrap_or(0);
        // A keyword outside the table is reported verbatim: it is still the
        // database-specific name of that column's type, and inventing a
        // different one would misreport it.
        let name = DECLARED_TYPE_NAMES
            .iter()
            .find(|(candidate, _)| *candidate == keyword)
            .map_or_else(|| keyword.to_string(), |(_, name)| (*name).to_string());
        Self {
            name,
            length,
            scale,
        }
    }

    /// Resolve a column with no declared type — an expression such as
    /// `SELECT 1` — from the value the engine produced for it.
    fn from_value(value: rusqlite::types::ValueRef<'_>) -> Option<Self> {
        use rusqlite::types::ValueRef;
        let name = match value {
            // NULL says nothing about the column's type.
            ValueRef::Null => return None,
            ValueRef::Integer(_) => "int8",
            ValueRef::Real(_) => "float8",
            ValueRef::Text(_) => "varchar",
            ValueRef::Blob(_) => "varbyte",
        };
        Some(Self {
            name: name.to_string(),
            length: 0,
            scale: 0,
        })
    }
}

/// Build the modelled `ColumnMetadata` for one result column.
fn column_metadata(name: &str, column_type: Option<&ColumnType>) -> ColumnMetadata {
    let type_name = column_type.map(|column_type| column_type.name.clone());
    let signed = column_type
        .is_some_and(|column_type| SIGNED_TYPE_NAMES.contains(&column_type.name.as_str()));
    let case_sensitive = column_type
        .is_some_and(|column_type| CASE_SENSITIVE_TYPE_NAMES.contains(&column_type.name.as_str()));
    let length = column_type.map_or(0, |column_type| column_type.length);
    let scale = column_type.map_or(0, |column_type| column_type.scale);
    ColumnMetadata {
        // SQLite's prepared-statement metadata does not expose a result
        // column's NOT NULL constraint, so every column is reported with the
        // weakest claim: nullable.
        nullable: 1,
        is_case_sensitive: case_sensitive,
        // Redshift's supported data types carry no money type, so no column is
        // a currency column.
        is_currency: false,
        is_signed: signed,
        // Redshift reports the column's output name as both name and label; a
        // statement that aliases a column changes both together.
        label: Some(name.to_string()),
        name: Some(name.to_string()),
        // "The precision value of a decimal number column, or the column length
        // for a non-numeric column" — both are the declared size.
        precision: length,
        scale,
        length,
        type_name,
        // A result column's origin schema and table, and its default, are not
        // derivable from a prepared statement here, so they are reported absent
        // rather than guessed.
        schema_name: None,
        table_name: None,
        column_default: None,
    }
}

// ---------------------------------------------------------------------------
// Pagination cursors
// ---------------------------------------------------------------------------

/// Cursor kind naming a position in `ListStatements`' ordering.
pub const CURSOR_STATEMENTS: &str = "statements";
/// Cursor kind naming a row offset in a statement's result set.
pub const CURSOR_ROWS: &str = "rows";

/// Encode an opaque continuation token for `kind` at `position`.
///
/// Hex-encoded so a client cannot read a meaning into it and depend on the
/// encoding; the kind is inside the token so a token minted for one operation
/// is rejected by the other.
pub fn encode_cursor(kind: &str, position: u64) -> String {
    hex::encode(format!("{kind}:{position}"))
}

/// Decode a continuation token minted by [`encode_cursor`] for `kind`.
/// `None` for anything else, which a handler refuses as a `ValidationException`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<u64> {
    let decoded = hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(kind)?.strip_prefix(':')?.parse().ok()
}

// ---------------------------------------------------------------------------
// Timestamps
// ---------------------------------------------------------------------------

/// Render a wall-clock instant as the modelled `Timestamp`.
pub fn to_timestamp(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis())
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

// ---------------------------------------------------------------------------
// The service's state
// ---------------------------------------------------------------------------

/// What was submitted: one statement, or a batch of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionKind {
    /// One statement, submitted by `ExecuteStatement`. It is committed on its
    /// own, which is what the modelled `AUTO_COMMIT` means.
    Single,
    /// A batch, submitted by `BatchExecuteStatement` in the `ExecutionMode` it
    /// asked for.
    Batch(ExecutionMode),
}

impl SubmissionKind {
    /// Whether this submission is a batch, which is reported as one statement
    /// with one `SubStatements` entry per statement it submitted.
    pub fn is_batch(self) -> bool {
        matches!(self, Self::Batch(_))
    }

    /// The `ExecutionMode` this submission's statements were committed under.
    pub fn execution_mode(self) -> ExecutionMode {
        match self {
            Self::Single => ExecutionMode::AutoCommit,
            Self::Batch(mode) => mode,
        }
    }
}

/// Everything a submit operation needs to record what it ran.
#[derive(Debug)]
pub struct Submission {
    /// The statements that were run, in submission order: one for
    /// `ExecuteStatement`, a batch's `Sqls` for `BatchExecuteStatement`.
    pub statements: Vec<String>,
    /// Whether this was one statement or a batch, and how a batch committed.
    pub kind: SubmissionKind,
    /// The cluster or workgroup to run it against.
    pub target: ConnectionTarget,
    /// The database to run it in.
    pub database: String,
    /// The `DbUser`, when authenticating with temporary credentials.
    pub db_user: Option<String>,
    /// The `SecretArn`, when authenticating with Secrets Manager.
    pub secret_arn: Option<String>,
    /// The caller's `StatementName`.
    pub statement_name: Option<String>,
    /// The caller's `Parameters`.
    pub parameters: Vec<SqlParameter>,
    /// The caller's `WithEvent`.
    pub with_event: bool,
    /// The resolved `ResultFormat`.
    pub result_format: String,
    /// The session the statement runs in, when the caller uses one.
    pub session_id: Option<String>,
    /// The caller's `ClientToken`, for an idempotent replay.
    pub client_token: Option<String>,
    /// Whether the caller asked to wait for completion (`WaitTimeSeconds`).
    pub wait_for_completion: bool,
}

/// The connection members of a submit request, as the wire carried them.
///
/// `ExecuteStatement` and `BatchExecuteStatement` document the SAME
/// authorization combinations, so both resolve them here rather than each
/// deciding for itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConnectionRequest<'a> {
    /// `ClusterIdentifier`: a provisioned cluster.
    pub cluster_identifier: Option<&'a String>,
    /// `WorkgroupName`: a serverless workgroup.
    pub workgroup_name: Option<&'a String>,
    /// `SessionId`: a session a previous statement opened.
    pub session_id: Option<&'a String>,
    /// `Database`.
    pub database: Option<&'a String>,
    /// `DbUser`: temporary-credentials authorization.
    pub db_user: Option<&'a String>,
    /// `SecretArn`: Secrets Manager authorization.
    pub secret_arn: Option<&'a String>,
}

/// The data warehouse, database and authorization method a submission runs
/// under.
#[derive(Debug)]
pub struct Connection {
    /// The cluster or workgroup.
    pub target: ConnectionTarget,
    /// The database name.
    pub database: String,
    /// `DbUser`, when authenticating with temporary credentials.
    pub db_user: Option<String>,
    /// `SecretArn`, when authenticating with Secrets Manager.
    pub secret_arn: Option<String>,
}

/// Why a request's connection members do not name exactly one data warehouse,
/// one database and one authorization method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadConnection {
    /// A provisioned cluster AND a serverless workgroup.
    BothWarehouses,
    /// Neither of them, and no session either.
    NoWarehouse,
    /// Neither of them, but a `SessionId`: a valid request whose warehouse and
    /// database belong to a session this emulator does not model yet.
    SessionOnly,
    /// No database.
    NoDatabase,
    /// Two authorization methods at once.
    BothAuthMethods,
}

impl BadConnection {
    /// What is wrong with the request, as a `ValidationException` message.
    /// [`BadConnection::SessionOnly`] is NOT a malformed request, so its text
    /// says what is missing from the emulator instead.
    pub fn message(self) -> &'static str {
        match self {
            Self::BothWarehouses => {
                "ClusterIdentifier and WorkgroupName cannot both be supplied: a statement runs \
                 against either a provisioned cluster or a serverless workgroup"
            }
            Self::NoWarehouse => {
                "one of ClusterIdentifier (a provisioned cluster) or WorkgroupName (a serverless \
                 workgroup) is required"
            }
            Self::SessionOnly => "only a SessionId as its connection target",
            Self::NoDatabase => "Database is required",
            Self::BothAuthMethods => {
                "DbUser and SecretArn cannot both be supplied: DbUser authenticates with \
                 temporary credentials and SecretArn with Secrets Manager"
            }
        }
    }
}

impl ConnectionRequest<'_> {
    /// Resolve the one data warehouse, database and authorization method the
    /// documented combinations name.
    ///
    /// A statement runs against exactly one data warehouse: every documented
    /// authorization combination names either a provisioned cluster
    /// (`ClusterIdentifier`) or a serverless workgroup (`WorkgroupName`), never
    /// both and never neither, and supplies the database. `SecretArn` selects
    /// Secrets Manager authorization and `DbUser` temporary credentials; they
    /// are alternatives, so supplying both names two methods for one
    /// submission.
    ///
    /// The session-reuse form, where a caller that already holds a `SessionId`
    /// names neither warehouse, is a VALID request this slice cannot serve
    /// faithfully: the warehouse and database such a call runs against are the
    /// ones its session was opened with, and no session store exists yet
    /// (`ListSessions` is still scaffolded), so answering it would mean
    /// inventing a connection target. It is reported as
    /// [`BadConnection::SessionOnly`], which a handler answers with the
    /// scaffold's `NotImplemented` — the gap is the emulator's, not the
    /// caller's.
    pub fn resolve(self) -> Result<Connection, BadConnection> {
        let target = match (
            non_empty(self.cluster_identifier),
            non_empty(self.workgroup_name),
        ) {
            (Some(cluster), None) => ConnectionTarget::Cluster(cluster),
            (None, Some(workgroup)) => ConnectionTarget::Workgroup(workgroup),
            (Some(_), Some(_)) => return Err(BadConnection::BothWarehouses),
            (None, None) if non_empty(self.session_id).is_some() => {
                return Err(BadConnection::SessionOnly)
            }
            (None, None) => return Err(BadConnection::NoWarehouse),
        };
        let Some(database) = non_empty(self.database) else {
            return Err(BadConnection::NoDatabase);
        };
        let db_user = non_empty(self.db_user);
        let secret_arn = non_empty(self.secret_arn);
        if db_user.is_some() && secret_arn.is_some() {
            return Err(BadConnection::BothAuthMethods);
        }
        Ok(Connection {
            target,
            database,
            db_user,
            secret_arn,
        })
    }
}

/// A lockable handle to one database's engine.
///
/// The engine lives behind its OWN lock rather than inside the service's state
/// lock, so a handler takes the state lock only to resolve the handle and again
/// to record the result — never for the duration of an execution.
pub type DatabaseHandle = std::sync::Arc<Mutex<Database>>;

/// Why a submission could not be given a database to run on.
#[derive(Debug)]
pub enum NoDatabase {
    /// The SQL engine itself could not open or confine a database.
    EngineUnavailable(String),
    /// This emulator already holds [`MAX_OPEN_DATABASES`] databases.
    TooManyOpen(usize),
}

/// One open database, its connection's immutable `RedshiftPid`, and when a
/// statement last ran on it so an idle one can be closed instead of being held
/// for the life of the process.
///
/// The pid is cached HERE rather than read back off the connection: reading it
/// would mean taking the engine's mutex while the service's state lock is held,
/// and that mutex is held for a whole execution — so one slow statement would
/// stall every other operation of this service behind the state lock.
#[derive(Debug)]
struct OpenDatabase {
    handle: DatabaseHandle,
    pid: i64,
    last_used: SystemTime,
}

/// Per-service state for com-amazonaws-redshiftdata-redshiftdata. A handler reaches it through [`service_state`].
///
/// Every map here grows on request, so every map here is bounded:
/// [`MAX_RETAINED_STATEMENTS`] and [`STATEMENT_RETENTION`] bound the statement
/// history (and the result sets it holds, and the client tokens that point into
/// it), and [`MAX_OPEN_DATABASES`] bounds the engines and handles a caller can
/// make the emulator open.
#[derive(Debug, Default)]
pub struct ComAmazonawsRedshiftdataRedshiftdataState {
    /// The retained statement history, by `Id`.
    statements: HashMap<String, Statement>,
    /// `ClientToken` -> `Id`, so an idempotent replay answers with the first
    /// statement rather than submitting a second one. An entry is dropped with
    /// the statement it points at, so this cannot outgrow the history.
    client_tokens: HashMap<String, String>,
    /// The SQL engine, one database per connection target and database name.
    databases: HashMap<String, OpenDatabase>,
    /// The next submission sequence number.
    next_seq: u64,
    /// The next `RedshiftQueryId`.
    next_query_id: i64,
    /// The next `RedshiftPid`.
    next_pid: i64,
}

impl ComAmazonawsRedshiftdataRedshiftdataState {
    /// Materialise every status transition the clock has already made due.
    ///
    /// Every operation calls this before it reads, so a status change that was
    /// owed at a given instant is reported from that instant on, whether or not
    /// anyone called in between.
    pub fn materialise_due_transitions(&mut self) {
        let now = Instant::now();
        for statement in self.statements.values_mut() {
            statement.advance(now);
        }
    }

    /// The statement with this `Id`.
    ///
    /// The `Id` may name ONE statement of a batch: `SubStatementData.Id`
    /// documents that "a suffix indicates the number of the SQL statement" —
    /// `<batch id>:2` "indicates the second SQL statement of a batch query" —
    /// and the `UUID` pattern allows exactly that suffix. So the suffix is
    /// resolved against the batch's statements, in submission order, and a
    /// batch's statement is readable through every operation that takes an
    /// `Id`.
    pub fn statement(&self, id: &str) -> Option<&Statement> {
        if let Some(statement) = self.statements.get(id) {
            return Some(statement);
        }
        let (batch_id, position) = split_batch_member(id)?;
        self.statements
            .get(batch_id)
            .filter(|batch| batch.is_batch)
            .and_then(|batch| batch.sub_statements.get(position.checked_sub(1)?))
    }

    /// The `Id` a previous `ExecuteStatement` with this `ClientToken` minted.
    pub fn statement_id_for_client_token(&self, token: &str) -> Option<&str> {
        self.client_tokens.get(token).map(String::as_str)
    }

    /// Every retained statement, newest submission first — the order
    /// `ListStatements` enumerates in.
    pub fn statements_newest_first(&self) -> Vec<&Statement> {
        let mut statements: Vec<&Statement> = self.statements.values().collect();
        statements.sort_unstable_by_key(|statement| std::cmp::Reverse(statement.seq));
        statements
    }

    /// Stop a still-running statement, reporting what this call found: one
    /// lookup decides the outcome AND carries the status that was observed, so
    /// no caller re-reads the statement to describe what happened.
    pub fn cancel(&mut self, id: &str) -> Cancellation {
        if let Some(statement) = self.statements.get_mut(id) {
            let observed = statement.status;
            return if statement.cancel(SystemTime::now()) {
                Cancellation::Cancelled
            } else {
                Cancellation::AlreadyStopped(observed)
            };
        }
        // One statement of a batch does not run on its own: the batch was
        // submitted as one request and its statements run serially inside it,
        // so what a caller stops is the batch.
        if let Some((batch_id, position)) = split_batch_member(id) {
            let names_a_member = self.statements.get(batch_id).is_some_and(|batch| {
                batch.is_batch && position >= 1 && position <= batch.sub_statements.len()
            });
            if names_a_member {
                return Cancellation::BatchMember(batch_id.to_string());
            }
        }
        Cancellation::Unknown
    }

    /// Resolve the database a submission will run on, opening and confining it on
    /// first use, and report the `RedshiftPid` of its connection.
    ///
    /// Returns a handle, not a borrow, so the caller can release the state lock
    /// before running anything on it.
    pub fn reserve_database(
        &mut self,
        target: &ConnectionTarget,
        database: &str,
    ) -> Result<(DatabaseHandle, i64), NoDatabase> {
        let now = SystemTime::now();
        // Close what nothing has used for a full retention span first, so memory
        // a statement committed inside the engine is reclaimed rather than held
        // for the life of the process.
        self.close_idle_databases(now);

        let key = target.database_key(database);
        if let Some(open) = self.databases.get_mut(&key) {
            open.last_used = now;
            // No engine lock is taken here: the pid is cached on the entry, so
            // resolving a database never waits on an execution.
            return Ok((std::sync::Arc::clone(&open.handle), open.pid));
        }
        // The key is built from caller-chosen names, so a new name must not be
        // able to open another engine without limit.
        if self.databases.len() >= MAX_OPEN_DATABASES {
            return Err(NoDatabase::TooManyOpen(MAX_OPEN_DATABASES));
        }
        if self.next_pid == 0 {
            self.next_pid = FIRST_PID;
        }
        let pid = self.next_pid;
        let database = Database::open()
            .map_err(|error| NoDatabase::EngineUnavailable(scrub_paths(&error.to_string())))?;
        self.next_pid += 1;
        let handle: DatabaseHandle = std::sync::Arc::new(Mutex::new(database));
        self.databases.insert(
            key,
            OpenDatabase {
                handle: std::sync::Arc::clone(&handle),
                pid,
                last_used: now,
            },
        );
        Ok((handle, pid))
    }

    /// Close every database no statement has run on for
    /// [`DATABASE_IDLE_TIMEOUT`]. Dropping the entry drops the connection, which
    /// releases both of its page-bounded schemas back to the allocator.
    fn close_idle_databases(&mut self, now: SystemTime) {
        self.databases.retain(|_, open| {
            !now.duration_since(open.last_used)
                .is_ok_and(|idle| idle > DATABASE_IDLE_TIMEOUT)
        });
    }

    /// Record a finished submission as a new statement, as SUBMITTED with the
    /// clock owing its outcome, and return its `Id`.
    ///
    /// `executions` carries one entry per statement of the submission, in
    /// submission order, exactly as [`run_on`] and [`run_batch_on`] report
    /// them. A batch is recorded as ONE statement with one substatement per
    /// statement it submitted, and the batch's own status, error, duration and
    /// `HasResultSet` are its statements' — never values of its own.
    pub fn record(
        &mut self,
        submission: Submission,
        pid: i64,
        executions: Vec<Execution>,
    ) -> String {
        let id = new_uuid();
        let created_at = SystemTime::now();
        self.next_seq += 1;

        // The statement answers SUBMITTED now and owes its terminal status
        // later; a caller that asked to wait for completion has already waited
        // longer than the span, so for that caller it is due immediately.
        let (due_at, completed_at) = if submission.wait_for_completion {
            (Instant::now(), created_at)
        } else {
            (
                Instant::now() + COMPLETION_SPAN,
                created_at + COMPLETION_SPAN,
            )
        };

        // One entry per submitted statement, whatever the engine reported.
        let mut executions = executions;
        executions.truncate(submission.statements.len());
        while executions.len() < submission.statements.len() {
            executions.push(Execution::never_ran());
        }

        let shared = Shared {
            submission: &submission,
            created_at,
            due_at,
            completed_at,
            seq: self.next_seq,
            pid,
        };

        let mut statement = if submission.kind.is_batch() {
            let mut sub_statements = Vec::with_capacity(executions.len());
            for (index, (sql, execution)) in submission
                .statements
                .iter()
                .zip(executions)
                .enumerate()
            {
                self.next_query_id += 1;
                sub_statements.push(build_statement(
                    &shared,
                    Recorded {
                        // "A suffix indicates the number of the SQL statement",
                        // counted from one in submission order.
                        id: format!("{id}:{}", index + 1),
                        sql: sql.clone(),
                        query_uuid: Some(new_uuid()),
                        redshift_query_id: self.next_query_id,
                        execution,
                        client_token: None,
                        is_batch: false,
                        sub_statements: Vec::new(),
                    },
                ));
            }
            let outcome = batch_outcome(&sub_statements);
            build_statement(
                &shared,
                Recorded {
                    id: id.clone(),
                    // A batch has no single query string; each of its
                    // statements carries its own.
                    sql: String::new(),
                    // Nor is a batch itself a query the cluster ran, so it
                    // carries none of the identifiers a query gets.
                    query_uuid: None,
                    redshift_query_id: 0,
                    execution: outcome,
                    client_token: submission.client_token.clone(),
                    is_batch: true,
                    sub_statements,
                },
            )
        } else {
            self.next_query_id += 1;
            build_statement(
                &shared,
                Recorded {
                    id: id.clone(),
                    sql: submission.statements.first().cloned().unwrap_or_default(),
                    query_uuid: Some(new_uuid()),
                    redshift_query_id: self.next_query_id,
                    execution: executions.into_iter().next().unwrap_or_else(Execution::never_ran),
                    client_token: submission.client_token.clone(),
                    is_batch: false,
                    sub_statements: Vec::new(),
                },
            )
        };
        statement.advance(Instant::now());

        if let Some(token) = submission.client_token {
            self.client_tokens.insert(token, id.clone());
        }
        self.statements.insert(id.clone(), statement);
        self.evict_expired_statements(created_at);
        id
    }

    /// Drop the statements this service no longer retains: those past
    /// [`STATEMENT_RETENTION`], then the oldest submissions while more than
    /// [`MAX_RETAINED_STATEMENTS`] statements remain — a batch counting as
    /// itself plus each of its statements, since each of those holds a result
    /// set of its own. Each eviction drops that submission's result sets and
    /// its client token with it, so neither map outgrows the history.
    fn evict_expired_statements(&mut self, now: SystemTime) {
        let expired: Vec<String> = self
            .statements
            .values()
            .filter(|statement| {
                now.duration_since(statement.created_at)
                    .is_ok_and(|age| age > STATEMENT_RETENTION)
            })
            .map(|statement| statement.id.clone())
            .collect();
        for id in expired {
            self.forget(&id);
        }

        while self.retained_statement_count() > MAX_RETAINED_STATEMENTS {
            let Some(oldest) = self
                .statements
                .values()
                .min_by_key(|statement| statement.seq)
                .map(|statement| statement.id.clone())
            else {
                break;
            };
            self.forget(&oldest);
        }
    }

    /// How many statements the history holds: every retained submission, plus
    /// each statement of a batch, because each of those retains a result set of
    /// its own.
    fn retained_statement_count(&self) -> usize {
        self.statements
            .values()
            .map(|statement| 1 + statement.sub_statements.len())
            .sum()
    }

    /// Remove one statement and everything that points at it. A batch's
    /// statements go with the batch they belong to, since they are held on it.
    fn forget(&mut self, id: &str) {
        if let Some(statement) = self.statements.remove(id) {
            if let Some(token) = statement.client_token {
                self.client_tokens.remove(&token);
            }
        }
    }
}

/// What every statement of ONE submission shares: its connection, its database,
/// its parameters and its clock.
struct Shared<'a> {
    submission: &'a Submission,
    created_at: SystemTime,
    due_at: Instant,
    completed_at: SystemTime,
    seq: u64,
    pid: i64,
}

/// What is a statement's OWN: its `Id`, its text, the identifiers Redshift
/// generated for it, what its execution produced, and — for a batch — the
/// statements it submitted.
struct Recorded {
    id: String,
    sql: String,
    query_uuid: Option<String>,
    redshift_query_id: i64,
    execution: Execution,
    client_token: Option<String>,
    is_batch: bool,
    sub_statements: Vec<Statement>,
}

/// Assemble one stored statement from what its submission shares and what is
/// its own. A batch and each statement of it are built through here, so neither
/// can drift from the other on the connection they ran under.
fn build_statement(shared: &Shared<'_>, recorded: Recorded) -> Statement {
    let submission = shared.submission;
    Statement {
        id: recorded.id,
        query_uuid: recorded.query_uuid,
        seq: shared.seq,
        sql: recorded.sql,
        statement_name: submission.statement_name.clone(),
        database: Some(submission.database.clone()),
        db_user: submission.db_user.clone(),
        secret_arn: submission.secret_arn.clone(),
        target: submission.target.clone(),
        status: StatementStatus::Submitted,
        terminal_status: recorded.execution.status,
        due_at: shared.due_at,
        completed_at: shared.completed_at,
        error: recorded.execution.error,
        created_at: shared.created_at,
        updated_at: shared.created_at,
        duration_nanos: recorded.execution.duration_nanos,
        redshift_pid: shared.pid,
        redshift_query_id: recorded.redshift_query_id,
        has_result_set: recorded.execution.has_result_set,
        result_rows: recorded.execution.result_rows,
        result_size: recorded.execution.result_size,
        with_event: submission.with_event,
        result_format: submission.result_format.clone(),
        parameters: submission.parameters.clone(),
        session_id: submission.session_id.clone(),
        client_token: recorded.client_token,
        result: recorded.execution.result,
        is_batch: recorded.is_batch,
        execution_mode: submission.kind.execution_mode(),
        sub_statements: recorded.sub_statements,
    }
}

/// What a batch as a whole did, from the statements it ran.
///
/// A batch computes nothing of its own: it FAILED if any of its statements
/// failed, it stopped short if any of them never ran, and "the value [of
/// `HasResultSet`] is true if any substatement returns a result set". Its
/// `ResultRows` and `ResultSize` belong to its statements, so the batch reports
/// the modelled `-1` null marker for both.
fn batch_outcome(sub_statements: &[Statement]) -> Execution {
    let failed = sub_statements
        .iter()
        .any(|sub| sub.terminal_status == StatementStatus::Failed);
    let stopped = sub_statements
        .iter()
        .any(|sub| sub.terminal_status == StatementStatus::Aborted);
    let status = match (failed, stopped) {
        (true, _) => StatementStatus::Failed,
        (false, true) => StatementStatus::Aborted,
        (false, false) => StatementStatus::Finished,
    };
    Execution {
        status,
        // The engine's own message from the statement that failed — never a
        // message this service wrote about the batch.
        error: sub_statements.iter().find_map(|sub| sub.error.clone()),
        duration_nanos: sub_statements
            .iter()
            .fold(0_i64, |total, sub| total.saturating_add(sub.duration_nanos)),
        has_result_set: sub_statements.iter().any(|sub| sub.has_result_set),
        result_rows: -1,
        result_size: -1,
        result: None,
    }
}

/// The batch `Id` and the 1-based statement number an `Id` of the form
/// `<batch id>:<n>` names, or `None` for an `Id` that carries no such suffix.
fn split_batch_member(id: &str) -> Option<(&str, usize)> {
    let (batch_id, suffix) = id.rsplit_once(':')?;
    Some((batch_id, suffix.parse().ok()?))
}

/// How long a request waits for the engine before giving up.
///
/// `WaitTimeSeconds` is modelled with the range 1..30, and its documentation
/// ("the number of seconds to wait for the SQL statement to complete execution
/// before returning the response. ... The maximum value is 30 seconds") makes it
/// the caller's own ceiling, so a caller that named one is held to it. A request
/// that named none still waits no longer than the engine's own deadline plus a
/// grace period for a statement queued behind another on the same database.
pub fn wait_budget(wait_time_seconds: Option<i32>) -> Duration {
    match wait_time_seconds {
        Some(seconds) => Duration::from_secs(u64::from(seconds.clamp(1, 30).unsigned_abs())),
        None => SQL_TIMEOUT + SQL_QUEUE_GRACE,
    }
}

/// The engine's own wall-clock deadline for a request that waits `budget`.
///
/// Kept to half the request's budget, and never above [`SQL_TIMEOUT`], so the
/// engine always interrupts a runaway statement — turning it into a FAILED
/// statement the caller can read — BEFORE the request gives up waiting for it.
pub fn engine_timeout(budget: Duration) -> Duration {
    SQL_TIMEOUT.min(budget / 2)
}

/// An optional string member's value, treating an all-whitespace value as
/// absent. The Data API's connection and filter members are names, so a member
/// carrying only whitespace names nothing and is the same as not sending it.
pub fn non_empty(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// A fresh identifier in the form the `UUID` shape's pattern
/// `^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$` accepts.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().as_hyphenated().to_string()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRedshiftdataRedshiftdataState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRedshiftdataRedshiftdataState>)
}
