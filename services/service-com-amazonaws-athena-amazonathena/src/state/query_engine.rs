// SPDX-License-Identifier: CC0-1.0

//! The SQL engine an Athena query execution RUNS on, and the result set it produces.
//!
//! Athena is an HTTP control plane in front of a query engine: `StartQueryExecution`
//! takes SQL and `GetQueryResults` hands back the rows that SQL produced. So the SQL is
//! really run, by a real engine in this process ([`Database`], `rusqlite`/SQLite), and
//! the rows a read reports are the rows that run computed — nothing here matches on the
//! query text to decide an answer, and nothing synthesises a result set.
//!
//! The engine is CONFINED before any request reaches it (see [`Database::open`]): it
//! holds an in-memory database that names no path, every host-reaching SQL action is
//! refused by a default-deny authorizer, and one run is bounded by a wall-clock deadline,
//! a step budget and a cap on the rows, cells and bytes its result set may retain.
//!
//! One [`Database`] per catalog and database name a query addresses, so two queries in
//! the same database see each other's writes and a query in another database does not.
//! Registering what a `CREATE TABLE` defines in the Glue Data Catalog is NOT part of this
//! slice: Athena's metadata reads project Glue (see [`super`]), and wiring DDL through to
//! it belongs with the DDL operations' own issues.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::ops::types::{ColumnInfo, Datum, Row};

/// The `StatementType` of a statement that queries or changes DATA: Athena classifies
/// `SELECT`, `INSERT`, `UPDATE`, `DELETE` and `CREATE TABLE AS SELECT` as DML. One of
/// `STATEMENT_TYPE_VALUES` in [`crate::ops::types`] (the model's `StatementType` enum).
pub const STATEMENT_TYPE_DML: &str = "DML";
/// The `StatementType` of a statement that DEFINES an object — `CREATE`, `ALTER`,
/// `DROP` — see [`STATEMENT_TYPE_DML`].
pub const STATEMENT_TYPE_DDL: &str = "DDL";
/// The `StatementType` of everything else Athena accepts, such as `SHOW`, `DESCRIBE`,
/// `EXPLAIN` and `USE` — see [`STATEMENT_TYPE_DML`].
pub const STATEMENT_TYPE_UTILITY: &str = "UTILITY";

/// Athena's type name for a 32-bit integer column, as `ColumnInfo$Type` reports it.
///
/// Every name below is one of Athena's own SQL type names, and the precision beside it
/// is that type's own width: a 32-bit integer spans 10 decimal digits, a 64-bit one 19,
/// a double carries 17 significant decimal digits, and the unbounded character and
/// binary types report their maximum length.
const TYPE_INTEGER: (&str, i32) = ("integer", 10);
/// Athena's type name for a 64-bit integer column; see [`TYPE_INTEGER`].
const TYPE_BIGINT: (&str, i32) = ("bigint", 19);
/// Athena's type name for a double-precision column; see [`TYPE_INTEGER`].
const TYPE_DOUBLE: (&str, i32) = ("double", 17);
/// Athena's type name for a boolean column; see [`TYPE_INTEGER`].
const TYPE_BOOLEAN: (&str, i32) = ("boolean", 0);
/// Athena's type name for a character column; see [`TYPE_INTEGER`].
const TYPE_VARCHAR: (&str, i32) = ("varchar", 2_147_483_647);
/// Athena's type name for a binary column; see [`TYPE_INTEGER`].
const TYPE_VARBINARY: (&str, i32) = ("varbinary", 1_073_741_824);
/// The type name reported for a column the engine produced no typed value for — a
/// projection of `NULL`, or a column of a result set with no rows at all, neither of
/// which carries a type to report; see [`TYPE_INTEGER`].
const TYPE_UNKNOWN: (&str, i32) = ("unknown", 0);

/// The `ColumnNullable` every column reports.
///
/// The engine answers nothing about whether one column of a projection admits NULL, so
/// `UNKNOWN` is what can honestly be said; it is one of `COLUMN_NULLABLE_VALUES` in
/// [`crate::ops::types`].
const COLUMN_NULLABLE_UNKNOWN: &str = "UNKNOWN";

/// How long ONE query's engine run may take, wall clock.
///
/// The EMULATOR's own bound, with no AWS counterpart (Athena's own query timeout is
/// measured in minutes): a query that is still running past this is stopped and the
/// execution reports FAILED, so no request can keep an engine thread for longer than
/// this however expensive the SQL it submitted is.
pub const MAX_QUERY_DEADLINE: Duration = Duration::from_secs(5);

/// Step budget per second of [`MAX_QUERY_DEADLINE`], counted in SQLite virtual-machine
/// instructions. A wall-clock deadline cannot stop a CPU-bound query if the host clock
/// stalls or the process is suspended; this can. Set well above the instruction rate the
/// engine sustains, so it is the backstop and not a second, tighter limit.
const SQL_STEPS_PER_SECOND: u64 = 150_000_000;

/// How many virtual-machine instructions pass between two consecutive checks of the
/// deadline and the step budget.
const PROGRESS_HANDLER_OPS: i32 = 1_000;

/// How many rows one query's result set may retain. A result set is held for as long as
/// the execution is readable, so this bounds what one query can make the emulator keep.
pub const MAX_RESULT_ROWS: usize = 10_000;

/// How many VALUES (rows times columns) one query's result set may retain. A cell costs
/// memory whatever it holds, so a wide projection of constants is bounded by this even
/// though it charges almost nothing in bytes.
const MAX_RESULT_CELLS: usize = 200_000;

/// How many bytes of materialised result one query's result set may retain.
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// What one retained cell costs beyond its payload, charged against
/// [`MAX_RESULT_BYTES`] so a NULL cell is never free.
const RETAINED_CELL_OVERHEAD: usize = std::mem::size_of::<Datum>();

/// What one retained row costs beyond its cells: the row's own vector.
const RETAINED_ROW_OVERHEAD: usize = std::mem::size_of::<Row>();

/// Worst-case expansion of a text value rendered through `String::from_utf8_lossy`,
/// which replaces each invalid byte with a three-byte replacement character.
const LOSSY_TEXT_EXPANSION: usize = 3;

/// Worst-case expansion of a binary value, which is rendered as two hex digits a byte.
const HEX_EXPANSION: usize = 2;

/// `SQLITE_LIMIT_LENGTH`: the largest single string or blob the engine will produce, so
/// one value cannot be larger than the whole result budget.
const MAX_VALUE_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_SQL_LENGTH`: the largest statement text the engine will accept. The
/// model already bounds `QueryString` at 262144 bytes; this is the engine's own guard.
const MAX_SQL_BYTES: i32 = 1024 * 1024;

/// `SQLITE_LIMIT_COLUMN`: the widest row the engine will produce.
const MAX_COLUMNS: i32 = 1_000;

/// `SQLITE_LIMIT_EXPR_DEPTH`: the depth bound on a parsed expression tree, so a deeply
/// nested expression cannot recurse the parser off its stack.
const MAX_EXPR_DEPTH: i32 = 100;

/// `SQLITE_LIMIT_COMPOUND_SELECT`: how many `UNION`/`EXCEPT`/`INTERSECT` terms one
/// statement may chain.
const MAX_COMPOUND_SELECT: i32 = 50;

/// `PRAGMA max_page_count`, applied to BOTH schemas a statement can write (`main` and
/// `temp`). At SQLite's default 4 KiB page this is 16 MiB each, so a query that builds a
/// huge table runs out of database rather than out of the host's memory.
const MAX_DATABASE_PAGES: i64 = 4_096;

/// `PRAGMA hard_heap_limit`: the ceiling on what the SQL engine may allocate, so engine
/// memory is capped however many databases are open and whatever one query does.
const MAX_ENGINE_HEAP_BYTES: i64 = 256 * 1024 * 1024;

/// SQL functions this emulator refuses even though the engine's own build may carry
/// them: each one exists to reach something outside the database.
const DENIED_SQL_FUNCTIONS: &[&str] = &[
    "load_extension",
    "readfile",
    "writefile",
    "fsdir",
    "edit",
    "fts3_tokenizer",
];

/// One database a query runs in: a real SQL engine in this process, confined before it
/// ever sees caller SQL (see [`Database::open`]).
#[derive(Debug)]
pub struct Database {
    /// The engine connection. Private: every statement reaches it through [`run`], which
    /// is what installs the bounds one run is held to.
    connection: rusqlite::Connection,
}

/// A database as the state holds it, so two executions in the same database run one at a
/// time on one engine and an execution never holds the service's own state lock while
/// the engine works.
pub type DatabaseHandle = Arc<Mutex<Database>>;

impl Database {
    /// Open a fresh in-memory database and confine it.
    ///
    /// The engine is a real SQL engine, not a purpose-built tree walker, so the
    /// statements it would otherwise accept include ones that act on the HOST rather than
    /// on this database: `ATTACH DATABASE '<path>'` opens an arbitrary host file,
    /// `VACUUM INTO '<path>'` writes one, and a `PRAGMA` can turn the engine's own
    /// switches back on. Every one of those is closed here, before any request reaches
    /// `prepare`, and the order matters: the emulator's own `PRAGMA`s run first, because
    /// the authorizer installed at the end refuses `PRAGMA` from then on.
    fn open() -> rusqlite::Result<Self> {
        // Opened WITHOUT `SQLITE_OPEN_URI`: a URI filename is another way a
        // caller-chosen string could name a file, and nothing here needs it.
        // `SQLITE_OPEN_MEMORY` keeps the database off the filesystem entirely.
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_MEMORY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = rusqlite::Connection::open_in_memory_with_flags(flags)?;

        // `temp_store = MEMORY`: a large sort must not spill a file into the host's temp
        // directory. `max_page_count` on BOTH `main` and `temp`, because the pragma
        // applies to the schema it is issued against and every temporary object is on the
        // process heap. `hard_heap_limit` bounds the engine's allocator itself.
        connection.execute_batch(&format!(
            "PRAGMA temp_store = MEMORY;
             PRAGMA hard_heap_limit = {MAX_ENGINE_HEAP_BYTES};
             PRAGMA main.max_page_count = {MAX_DATABASE_PAGES};
             PRAGMA temp.max_page_count = {MAX_DATABASE_PAGES};"
        ))?;

        // Extension loading stays off DELIBERATELY: the dependency's `load_extension`
        // feature is not enabled, so neither the C entry point nor its Rust wrapper is
        // reachable from this crate at all, and the authorizer below denies the
        // `load_extension()` SQL function by name as well.
        // Defensive mode: no direct writes to the schema or to shadow tables.
        connection.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        // The schema is not trusted: a view or trigger stored in the database cannot
        // reach a privileged SQL function.
        connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
            false,
        )?;

        use rusqlite::limits::Limit;
        // No database may be attached at all, which closes `ATTACH` even if the
        // authorizer below were ever bypassed.
        connection.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0)?;
        connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COLUMN, MAX_COLUMNS)?;
        connection.set_limit(Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPR_DEPTH)?;
        connection.set_limit(Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT)?;

        // Default-deny: every action a query legitimately needs is named, and anything
        // else — including an action a future engine version introduces — is refused.
        connection.authorizer(Some(authorize));

        Ok(Self { connection })
    }
}

/// A confined database, ready to run queries, or the engine's own message for why one
/// could not be opened.
pub fn open_database() -> Result<DatabaseHandle, String> {
    Database::open()
        .map(|database| Arc::new(Mutex::new(database)))
        .map_err(|error| scrub_paths(&error.to_string()))
}

/// The engine's default-deny authorizer: what a submitted query may do.
fn authorize(context: rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization {
    use rusqlite::hooks::{AuthAction, Authorization};
    match context.action {
        // The host-filesystem escapes, denied by name. `ATTACH`/`DETACH` open an
        // arbitrary path; `PRAGMA` is also how `VACUUM INTO '<path>'` is authorized, and
        // how the switches set in `Database::open` would be turned back on.
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
        // The SQL a query legitimately runs: query, modify and define objects inside this
        // one database.
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
        // Anything else, including an action this engine version does not yet name:
        // refused rather than allowed by omission.
        _ => Authorization::Deny,
    }
}

/// The bound ONE run is held to: a deadline, a remaining step budget and an abandon
/// flag, read by the connection's progress handler.
#[derive(Debug, Default)]
struct RunGuard {
    /// When the run must stop, or `None` while no run is in flight.
    deadline: Mutex<Option<Instant>>,
    /// Engine instructions this run may still spend.
    steps_left: AtomicU64,
    /// Set once the run has been abandoned, whatever the clock says.
    abandoned: AtomicBool,
}

impl RunGuard {
    /// Arm the guard for one run.
    fn arm(&self, timeout: Duration, steps: u64) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now() + timeout);
        self.steps_left.store(steps, Ordering::SeqCst);
    }

    /// Disarm the guard once the run has returned, so an idle connection is never
    /// interrupted.
    fn disarm(&self) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Whether the engine should abandon the running statement. Called from the progress
    /// handler every [`PROGRESS_HANDLER_OPS`] instructions.
    fn should_interrupt(&self) -> bool {
        if self.abandoned.load(Ordering::SeqCst) {
            return true;
        }
        let deadline = *self
            .deadline
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(deadline) = deadline else {
            return false;
        };
        if Instant::now() >= deadline {
            return true;
        }
        let spent = u64::try_from(PROGRESS_HANDLER_OPS).unwrap_or(1);
        // Compared AFTER this callback's own spend, so the run is interrupted on the
        // callback that exhausts the budget rather than on the one after it.
        //
        // `try_update` is the current name of the atomic read-modify-write this budget has
        // always used: the standard library renamed `fetch_update` to it and deprecates the
        // old name, which this workspace's deny-on-warnings build turns into an error. Same
        // signature and orderings, and it answers the PREVIOUS value — which is what
        // `previous` is read as here, so the budget still trips on exactly the callback that
        // spends the last of it.
        let previous = self
            .steps_left
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                Some(left.saturating_sub(spent))
            })
            .unwrap_or(0);
        previous.saturating_sub(spent) == 0
    }
}

/// The step budget one run gets, derived from [`MAX_QUERY_DEADLINE`] at
/// [`SQL_STEPS_PER_SECOND`] so the two bounds scale together.
fn step_budget() -> u64 {
    let millis = u64::try_from(MAX_QUERY_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    millis
        .saturating_mul(SQL_STEPS_PER_SECOND / 1_000)
        .max(u64::try_from(PROGRESS_HANDLER_OPS).unwrap_or(1))
}

/// One query's result set, as the execution that produced it stores it.
///
/// Every value is held as the text Athena reports it as (`Datum$VarCharValue`), so a
/// repeat `GetQueryResults` reports byte-for-byte what the first one did.
#[derive(Debug, Clone, Default)]
pub struct StoredResultSet {
    /// One entry per column the query projected, in projection order.
    pub column_info: Vec<ColumnInfo>,
    /// The DATA rows, without the column-label row `GetQueryResults` puts in front of
    /// them: that row is the reading operation's own rendering of `column_info`.
    pub rows: Vec<Row>,
}

/// What running one query produced.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The engine ran the statement.
    Succeeded {
        /// The rows and columns it produced; empty for a statement that returns none.
        result: StoredResultSet,
        /// How many rows a data-manipulating statement wrote, for `UpdateCount`.
        update_count: Option<i64>,
        /// How many bytes of data the engine read to answer, for `DataScannedInBytes`.
        data_scanned_bytes: i64,
    },
    /// The statement did not run to completion, with the message for why.
    Failed {
        /// The engine's message, with anything path-shaped redacted, or the emulator's
        /// own for a bound it holds a run to.
        message: String,
        /// Whose doing the failure was, which is what the reported error category
        /// follows from.
        cause: FailureCause,
    },
}

/// Whose doing a failed query's failure was.
///
/// Read off the error CODE the engine reports, never off its message: an authorization
/// denial and an interruption are this emulator's doing, and every other engine error is
/// the SQL submitted. The engine reports a DENIED FUNCTION as a plain statement error
/// rather than as an authorization denial, so that one is reported as the SQL — which is
/// what the engine said about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCause {
    /// The engine refused, interrupted or could not run the SQL submitted — the
    /// caller's own input.
    Sql,
    /// This EMULATOR stopped the run: a bound it holds a run to was reached, or its
    /// authorizer refused the statement outright. The SQL is not wrong; what it asked
    /// for is not allowed here, or it outgrew what is allowed.
    Emulator,
}

/// One run: what it produced, and how long the engine took.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The rows, or the failure.
    pub outcome: Outcome,
    /// What the engine spent on it, reported as `EngineExecutionTimeInMillis`.
    pub engine_time: Duration,
}

/// Run `sql` on the engine `handle` guards, with `parameters` bound to its placeholders
/// in order.
///
/// `counts_rows` says whether the engine counts the rows this statement writes, which is
/// what decides whether a row count is reported at all (see [`Submitted`]).
///
/// BLOCKING, deliberately: the engine is synchronous, so a caller must put this on a
/// blocking pool (`tokio::task::spawn_blocking`) rather than run it on an async worker.
/// It takes only this one database's lock and never the service's state lock, so one
/// caller's slow query serialises nothing but its own database.
pub fn run(
    handle: &DatabaseHandle,
    sql: &str,
    counts_rows: bool,
    parameters: &[String],
) -> Completion {
    let database = handle.lock().unwrap_or_else(PoisonError::into_inner);
    let guard = Arc::new(RunGuard::default());
    let handler = Arc::clone(&guard);
    database
        .connection
        .progress_handler(PROGRESS_HANDLER_OPS, Some(move || handler.should_interrupt()));

    let started = Instant::now();
    guard.arm(MAX_QUERY_DEADLINE, step_budget());
    let produced = execute_once(&database.connection, sql, parameters);
    guard.disarm();
    let engine_time = started.elapsed();

    let outcome = match produced {
        Ok(Produced::Rows {
            result,
            data_scanned_bytes,
        }) => Outcome::Succeeded {
            result,
            // Rows were returned rather than written, so there is no row count to
            // report: `UpdateCount` is "the number of rows inserted".
            update_count: None,
            data_scanned_bytes,
        },
        Ok(Produced::Affected(rows)) => Outcome::Succeeded {
            result: StoredResultSet::default(),
            // Reported only for a statement whose rows the engine counts: a definition
            // statement writes none to count, and the rows a `CREATE TABLE AS SELECT`
            // writes are not counted by this engine at all — so no count is reported for
            // it rather than one it did not produce.
            update_count: counts_rows.then_some(rows),
            data_scanned_bytes: 0,
        },
        Err(failure) => Outcome::Failed {
            message: failure.message(),
            cause: failure.cause(),
        },
    };
    Completion {
        outcome,
        engine_time,
    }
}

/// Either the rows a query returned, or the row count a statement that returns no
/// columns affected.
enum Produced {
    /// A projection: its rows and the bytes the engine read for them.
    Rows {
        /// The result set itself.
        result: StoredResultSet,
        /// The engine's own size for the values it read.
        data_scanned_bytes: i64,
    },
    /// A statement with no result set, and the number of rows it changed.
    Affected(i64),
}

/// Why one run did not produce a complete result.
enum RunFailure {
    /// The engine refused, interrupted or could not run the statement.
    Engine(rusqlite::Error),
    /// The result set reached one of this emulator's retention bounds. The query FAILS
    /// rather than reporting a truncated result as a complete one: a caller must never
    /// be handed a short answer that looks whole.
    Bound(String),
}

impl From<rusqlite::Error> for RunFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Engine(error)
    }
}

impl RunFailure {
    /// The message to report as the execution's `StateChangeReason`.
    fn message(&self) -> String {
        match self {
            Self::Engine(error) => scrub_paths(&error.to_string()),
            Self::Bound(message) => message.clone(),
        }
    }

    /// Whose doing this failure was; see [`FailureCause`].
    fn cause(&self) -> FailureCause {
        let emulator_code = |error: &rusqlite::ffi::Error| {
            matches!(
                error.code,
                // The EMULATOR stopped the run at its deadline or step budget.
                rusqlite::ErrorCode::OperationInterrupted
                    // The EMULATOR's own authorizer refused the statement: the SQL is
                    // valid, and what it asked for is what this confinement does not
                    // allow.
                    | rusqlite::ErrorCode::AuthorizationForStatementDenied
            )
        };
        match self {
            Self::Engine(rusqlite::Error::SqliteFailure(error, _)) if emulator_code(error) => {
                FailureCause::Emulator
            }
            Self::Engine(_) => FailureCause::Sql,
            Self::Bound(_) => FailureCause::Emulator,
        }
    }
}

/// Prepare and run one statement, keeping no more of its result than the bounds above
/// allow.
fn execute_once(
    connection: &rusqlite::Connection,
    sql: &str,
    parameters: &[String],
) -> Result<Produced, RunFailure> {
    let mut statement = connection.prepare(sql)?;

    // `ExecutionParameters` are "applied sequentially to the parameters in the query in
    // the order in which the parameters occur", and both Athena and this engine write a
    // query parameter as `?`, so they bind positionally and in order. A count that does
    // not match the placeholders in the query is the ENGINE's error, not a scan of the
    // SQL text here.
    let bound: Vec<rusqlite::types::Value> =
        parameters.iter().map(String::as_str).map(bound_parameter).collect();

    if statement.column_count() == 0 {
        let affected = statement.execute(rusqlite::params_from_iter(bound.iter()))?;
        return Ok(Produced::Affected(i64::try_from(affected).unwrap_or(i64::MAX)));
    }

    // The column names and declared types are read before the query starts, so the
    // immutable borrow of the prepared statement ends before `query` takes it mutably.
    let columns: Vec<(String, Option<ColumnType>)> = statement
        .columns()
        .iter()
        .map(|column| {
            (
                column.name().to_string(),
                column.decl_type().and_then(ColumnType::declared),
            )
        })
        .collect();

    let mut rows = statement.query(rusqlite::params_from_iter(bound.iter()))?;
    let mut records: Vec<Row> = Vec::new();
    // `scanned` is the engine's own size for the values it read, which is what
    // `DataScannedInBytes` means; `retained` is what holding the result actually costs
    // this process, which is what the bound has to measure.
    let mut scanned: usize = 0;
    let mut retained: usize = 0;
    let mut cells: usize = 0;
    let mut observed: Vec<Option<ColumnType>> =
        columns.iter().map(|(_, declared)| *declared).collect();

    while let Some(row) = rows.next()? {
        if records.len() >= MAX_RESULT_ROWS {
            return Err(RunFailure::Bound(format!(
                "the result set exceeded the {MAX_RESULT_ROWS} rows this emulator retains \
                 for one query"
            )));
        }
        if cells.saturating_add(columns.len()) > MAX_RESULT_CELLS {
            return Err(RunFailure::Bound(format!(
                "the result set exceeded the {MAX_RESULT_CELLS} values this emulator \
                 retains for one query"
            )));
        }

        // The row is SIZED before any of it is materialised: `get_ref` borrows the
        // engine's own buffer and copies nothing, so a row that would take the result
        // set past its budget is measured and refused rather than allocated first.
        let mut row_scanned: usize = 0;
        let mut row_retained: usize = RETAINED_ROW_OVERHEAD;
        for index in 0..columns.len() {
            let value = row.get_ref(index)?;
            row_scanned = row_scanned.saturating_add(value_bytes(value));
            row_retained = row_retained.saturating_add(retained_bytes(value));
        }
        if retained.saturating_add(row_retained) > MAX_RESULT_BYTES {
            return Err(RunFailure::Bound(format!(
                "the result set exceeded the {MAX_RESULT_BYTES} bytes this emulator \
                 retains for one query"
            )));
        }
        scanned = scanned.saturating_add(row_scanned);
        retained = retained.saturating_add(row_retained);
        cells = cells.saturating_add(columns.len());

        let mut data = Vec::with_capacity(columns.len());
        for (index, (_, declared)) in columns.iter().enumerate() {
            let value = row.get_ref(index)?;
            // A column the engine declares a type for reports THAT type; one without is
            // inferred from the values it produces, and the inferred type has to hold
            // every one of them, so every row is looked at rather than only the first.
            if declared.is_none() {
                if let Some(slot) = observed.get_mut(index) {
                    *slot = ColumnType::widened(*slot, value);
                }
            }
            data.push(Datum {
                var_char_value: rendered(value, *declared),
            });
        }
        records.push(Row { data: Some(data) });
    }

    let column_info = columns
        .iter()
        .zip(observed.iter())
        .map(|((name, _), column_type)| column_metadata(name, *column_type))
        .collect();

    Ok(Produced::Rows {
        result: StoredResultSet {
            column_info,
            rows: records,
        },
        data_scanned_bytes: i64::try_from(scanned).unwrap_or(i64::MAX),
    })
}

/// One `ExecutionParameter` as the value it binds.
///
/// A parameter arrives as the SQL literal the caller wrote (`1`, `-2.5`, `'a name'`), so
/// it is READ by the SQL parser this crate already depends on — never by a reader of
/// literal syntax written here — and binds as the value that literal denotes. Anything
/// that is not a literal binds as the text the caller sent. Nothing is interpolated into
/// the SQL: the value is BOUND, so a parameter can never add syntax to the statement.
fn bound_parameter(parameter: &str) -> rusqlite::types::Value {
    use ::sqlparser::ast::{Expr, UnaryOperator, Value as SqlValue};
    use rusqlite::types::Value;

    let text = || Value::Text(parameter.to_string());
    let dialect = ::sqlparser::dialect::GenericDialect {};
    let Ok(expression) = ::sqlparser::parser::Parser::new(&dialect)
        .try_with_sql(parameter)
        .and_then(|mut parser| parser.parse_expr())
    else {
        return text();
    };
    // A negative number parses as a negation of a positive one, so the sign is read off
    // the expression rather than off the characters.
    let (negated, value) = match expression {
        Expr::Value(value) => (false, value.value),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match *expr {
            Expr::Value(value) => (true, value.value),
            _ => return text(),
        },
        _ => return text(),
    };
    match value {
        SqlValue::Number(number, _) => {
            let signed = if negated {
                format!("-{number}")
            } else {
                number
            };
            signed.parse::<i64>().map(Value::Integer).unwrap_or_else(|_| {
                signed
                    .parse::<f64>()
                    .map_or_else(|_| text(), Value::Real)
            })
        }
        SqlValue::SingleQuotedString(string)
        | SqlValue::DoubleQuotedString(string)
        | SqlValue::EscapedStringLiteral(string) => Value::Text(string),
        SqlValue::Boolean(flag) => Value::Integer(i64::from(flag)),
        SqlValue::Null => Value::Null,
        _ => text(),
    }
}

/// The byte size one value contributes to `DataScannedInBytes`: the engine's own size
/// for that value.
fn value_bytes(value: rusqlite::types::ValueRef<'_>) -> usize {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => 0,
        ValueRef::Integer(_) => std::mem::size_of::<i64>(),
        ValueRef::Real(_) => std::mem::size_of::<f64>(),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
    }
}

/// What retaining one value actually costs this process: the `Datum` it is stored in
/// plus the text it is rendered as, charged at its worst-case expansion.
fn retained_bytes(value: rusqlite::types::ValueRef<'_>) -> usize {
    use rusqlite::types::ValueRef;
    let payload = match value {
        // A number is rendered as decimal text, which is never longer than the digits of
        // a 64-bit value plus a sign and an exponent.
        ValueRef::Null => 0,
        ValueRef::Integer(_) | ValueRef::Real(_) => 32,
        ValueRef::Text(bytes) => bytes.len().saturating_mul(LOSSY_TEXT_EXPANSION),
        ValueRef::Blob(bytes) => bytes.len().saturating_mul(HEX_EXPANSION),
    };
    RETAINED_CELL_OVERHEAD.saturating_add(payload)
}

/// One value as the text `Datum$VarCharValue` carries, or `None` for SQL NULL — which
/// Athena reports as a datum with no value rather than as the empty string.
fn rendered(value: rusqlite::types::ValueRef<'_>, declared: Option<ColumnType>) -> Option<String> {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(integer) => {
            if declared == Some(ColumnType::Boolean) {
                // A boolean column's values are Athena's own `true`/`false`, not the 1
                // and 0 the engine stores them as.
                Some((integer != 0).to_string())
            } else {
                Some(integer.to_string())
            }
        }
        // `{:?}` keeps the fractional part of a whole double (`1.0`, not `1`), which is
        // how an approximate-numeric value is told from an exact one.
        ValueRef::Real(double) => Some(format!("{double:?}")),
        ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Some(hex::encode(bytes)),
    }
}

/// The Athena type one column of a result set reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    /// A 32-bit integer.
    Integer,
    /// A 64-bit integer.
    BigInt,
    /// A double-precision number.
    Double,
    /// A boolean.
    Boolean,
    /// Character data.
    VarChar,
    /// Binary data.
    VarBinary,
}

impl ColumnType {
    /// The type a column DECLARED as `declared` reports, or `None` when the declaration
    /// names none this engine maps — in which case the values decide (see
    /// [`ColumnType::widened`]).
    fn declared(declared: &str) -> Option<Self> {
        let declared = declared.to_ascii_lowercase();
        // The declaration is matched the way SQLite's own type affinity reads it: by the
        // substring that names the type family.
        if declared.contains("bool") {
            return Some(Self::Boolean);
        }
        if declared.contains("bigint") {
            return Some(Self::BigInt);
        }
        if declared.contains("int") {
            return Some(Self::Integer);
        }
        if declared.contains("real") || declared.contains("floa") || declared.contains("doub") {
            return Some(Self::Double);
        }
        if declared.contains("char") || declared.contains("clob") || declared.contains("text") {
            return Some(Self::VarChar);
        }
        if declared.contains("blob") || declared.contains("binary") {
            return Some(Self::VarBinary);
        }
        None
    }

    /// The type that holds both what `current` holds and `value`.
    fn widened(current: Option<Self>, value: rusqlite::types::ValueRef<'_>) -> Option<Self> {
        use rusqlite::types::ValueRef;
        let observed = match value {
            // A NULL says nothing about the column's type.
            ValueRef::Null => return current,
            ValueRef::Integer(integer) => {
                if i32::try_from(integer).is_ok() {
                    Self::Integer
                } else {
                    Self::BigInt
                }
            }
            ValueRef::Real(_) => Self::Double,
            ValueRef::Text(_) => Self::VarChar,
            ValueRef::Blob(_) => Self::VarBinary,
        };
        Some(match current {
            None => observed,
            Some(current) if current == observed => current,
            // Two different types in one column: the wider of the two holds both, and
            // the widths run narrow to wide in `rank`.
            Some(current) => {
                if current.rank() >= observed.rank() {
                    current
                } else {
                    observed
                }
            }
        })
    }

    /// How wide this type is beside the others: a type of a higher rank can carry every
    /// value one of a lower rank can.
    fn rank(self) -> u8 {
        match self {
            Self::Boolean => 0,
            Self::Integer => 1,
            Self::BigInt => 2,
            Self::Double => 3,
            Self::VarBinary => 4,
            Self::VarChar => 5,
        }
    }

    /// This type's Athena name and its precision.
    fn named(self) -> (&'static str, i32) {
        match self {
            Self::Integer => TYPE_INTEGER,
            Self::BigInt => TYPE_BIGINT,
            Self::Double => TYPE_DOUBLE,
            Self::Boolean => TYPE_BOOLEAN,
            Self::VarChar => TYPE_VARCHAR,
            Self::VarBinary => TYPE_VARBINARY,
        }
    }
}

/// The `ColumnInfo` one column reports.
fn column_metadata(name: &str, column_type: Option<ColumnType>) -> ColumnInfo {
    let (type_name, precision) = column_type.map_or(TYPE_UNKNOWN, ColumnType::named);
    ColumnInfo {
        // A projection's columns belong to no one table, and the engine names neither a
        // catalog nor a schema for them, so none is reported.
        catalog_name: None,
        schema_name: None,
        table_name: None,
        name: name.to_string(),
        // The label is what a client renders as the heading, which is this column's name.
        label: Some(name.to_string()),
        r#type: type_name.to_string(),
        precision,
        // Every type above is a whole-number or character type, none of which has digits
        // after a decimal point.
        scale: 0,
        nullable: Some(COLUMN_NULLABLE_UNKNOWN.to_string()),
        // Only character data compares case-sensitively.
        case_sensitive: column_type == Some(ColumnType::VarChar),
    }
}

/// Redact anything path-shaped from an engine message before it goes on the wire. The
/// message is the engine's own and a caller needs it to see why a query failed, but a
/// host path in it would tell the caller about the machine the emulator runs on.
///
/// Only a token that STARTS a path is redacted — one beginning at the start of the
/// message or after whitespace or a quote, with `/`, `\\` or a `C:\` drive prefix. A
/// slash inside a token, as in the identifier of a syntax error, is left alone, so the
/// diagnostic keeps its detail.
fn scrub_paths(message: &str) -> String {
    let mut scrubbed = String::with_capacity(message.len());
    let mut characters = message.char_indices().peekable();
    let mut at_token_start = true;
    while let Some((index, character)) = characters.next() {
        let rest = message.get(index..).unwrap_or_default();
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

/// What the SQL of one query execution IS: how many statements it carries, the
/// `StatementType` of the one it carries when it carries exactly one, and whether the
/// engine counts the rows that statement writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submitted {
    /// How many statements the text parses as, or `None` when it does not parse at all —
    /// in which case the engine is the one that reports why.
    pub statements: Option<usize>,
    /// The `StatementType` of the single statement submitted, when there is one.
    pub statement_type: Option<&'static str>,
    /// Whether this statement is one whose written rows the engine counts: an `INSERT`,
    /// `UPDATE`, `DELETE` or `MERGE`. A `CREATE TABLE AS SELECT` is a data-manipulating
    /// statement too, but the engine does not count the rows it writes, so it is not one
    /// of these.
    pub counts_rows: bool,
}

/// Read `sql` as the statements it carries.
///
/// The text is PARSED, by the maintained SQL parser this workspace already uses, rather
/// than classified from its leading word: `StatementType` is a fact about the statement's
/// grammar, so it is read off the parse and not off the spelling.
pub fn submitted(sql: &str) -> Submitted {
    let dialect = ::sqlparser::dialect::GenericDialect {};
    let Ok(statements) = ::sqlparser::parser::Parser::parse_sql(&dialect, sql) else {
        return Submitted {
            statements: None,
            statement_type: None,
            counts_rows: false,
        };
    };
    let single = match statements.as_slice() {
        [statement] => Some(statement),
        _ => None,
    };
    Submitted {
        statements: Some(statements.len()),
        statement_type: single.map(statement_type_of),
        counts_rows: single.is_some_and(counts_rows),
    }
}

/// Whether the engine counts the rows this statement writes; see
/// [`Submitted::counts_rows`].
fn counts_rows(statement: &::sqlparser::ast::Statement) -> bool {
    use ::sqlparser::ast::Statement;
    matches!(
        statement,
        Statement::Insert(..)
            | Statement::Update { .. }
            | Statement::Delete(..)
            | Statement::Merge { .. }
    )
}

/// The `StatementType` of one parsed statement.
///
/// Athena documents the three classes `StatementType` names: a statement that queries or
/// changes DATA is DML, one that DEFINES an object is DDL, and everything else — the
/// statements that ask the engine about itself or set up a session — is UTILITY.
fn statement_type_of(statement: &::sqlparser::ast::Statement) -> &'static str {
    use ::sqlparser::ast::Statement;
    match statement {
        // `CREATE TABLE AS SELECT` defines a table AND writes the rows a query
        // produced, and Athena classifies it with the statements that manipulate data.
        Statement::CreateTable(create) if create.query.is_some() => STATEMENT_TYPE_DML,
        Statement::Query(..)
        | Statement::Insert(..)
        | Statement::Update { .. }
        | Statement::Delete(..)
        | Statement::Merge { .. } => STATEMENT_TYPE_DML,
        Statement::CreateTable(..)
        | Statement::CreateView { .. }
        | Statement::CreateIndex(..)
        | Statement::CreateSchema { .. }
        | Statement::CreateDatabase { .. }
        | Statement::AlterTable { .. }
        | Statement::Drop { .. }
        | Statement::Truncate { .. } => STATEMENT_TYPE_DDL,
        _ => STATEMENT_TYPE_UTILITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The STEP budget is the backstop for a run the wall clock cannot stop, so the
    /// progress handler must answer "interrupt" on exactly the callback that spends the
    /// last of it — not before it, and not one callback late.
    ///
    /// Armed with a deadline far in the future, so the only bound under test is the budget.
    #[test]
    fn the_step_budget_interrupts_the_run_on_the_callback_that_spends_it() {
        let spend = u64::try_from(PROGRESS_HANDLER_OPS).expect("the callback's own spend");
        // Each row: the budget one run gets, and which callback must be the interrupting
        // one — a budget of exactly one callback's spend, of several, and one that is not a
        // whole multiple of it.
        for (budget, interrupting_call) in [
            (spend, 1usize),
            (spend * 2, 2),
            (spend * 3, 3),
            (spend * 5 / 2, 3),
        ] {
            let guard = RunGuard::default();
            guard.arm(Duration::from_secs(3_600), budget);
            for call in 1..=interrupting_call {
                let interrupted = guard.should_interrupt();
                assert_eq!(
                    interrupted,
                    call == interrupting_call,
                    "case budget {budget}: callback {call} of {interrupting_call} answered \
                     interrupt={interrupted}"
                );
            }
            // Past the budget the answer stays "interrupt": the counter saturates rather
            // than wrapping back into a fresh budget.
            assert!(
                guard.should_interrupt(),
                "case budget {budget}: a spent budget stays spent"
            );
        }
    }

    /// A disarmed guard interrupts nothing, so an idle connection is never stopped.
    #[test]
    fn a_disarmed_guard_interrupts_nothing() {
        let guard = RunGuard::default();
        guard.arm(Duration::from_secs(3_600), 0);
        guard.disarm();
        assert!(
            !guard.should_interrupt(),
            "case disarmed: a guard with no run in flight must not interrupt"
        );
    }
}
