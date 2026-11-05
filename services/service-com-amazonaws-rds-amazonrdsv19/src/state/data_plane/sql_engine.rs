// SPDX-License-Identifier: CC0-1.0

//! The SQL engine behind a DB instance's engine endpoint: one in-process SQLite
//! database per DB instance.
//!
//! A statement a client sends over the engine's native wire is RUN here and the rows
//! this module returns are the rows the wire reports — nothing is synthesised from the
//! statement's text. The engine is SQLite (`rusqlite`, the `bundled` build already in
//! this workspace), which is not PostgreSQL and not MySQL: what it cannot do it refuses
//! with its own error, which the wire renders as that engine's error response, rather
//! than answering a fabricated success.
//!
//! One connection per DB instance, taken under a mutex for the duration of a single
//! statement:
//!
//! * the database is in memory and private to the instance, so one instance's rows never
//!   reach another's session and nothing is written to the host's disk;
//! * sessions of one instance share the connection, which is why a transaction one
//!   session opens is the instance's transaction — a limit of this engine, documented on
//!   [`InstanceDatabase::rollback_open_transaction`], which the wire calls when a session
//!   ends so uncommitted work never outlives the session that wrote it;
//! * every statement runs with the connection's own bounds in force (below) and under a
//!   DEFAULT-DENY authorizer ([`confine_to_the_instance`]), and the result set a session
//!   can draw out of one statement is capped in rows and in bytes.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::Connection;

/// How many rows one statement may hand back to a session.
pub const MAX_RESULT_ROWS: usize = 10_000;

/// How many bytes of result values one statement may hand back to a session, charged as
/// each value is read out of the engine, so a statement that would build a larger answer
/// is refused instead of allocating it.
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

/// The wall-clock budget one statement gets. Enforced by the engine's own progress
/// handler, which runs on the thread executing the statement — a timeout on the task
/// could not interrupt a CPU-bound statement, and this can.
pub const STATEMENT_DEADLINE: Duration = Duration::from_secs(5);

/// How often the engine asks the progress handler whether to carry on.
const PROGRESS_HANDLER_OPS: std::os::raw::c_int = 1_000;

/// The engine's own limits, which bound what one statement can ask of it. The values
/// mirror the ones this emulator's other SQL data plane uses
/// (`service-com-amazonaws-rdsdata-rdsdataservice`), so the two cannot drift on what a
/// statement may cost.
const MAX_VALUE_BYTES: i32 = 1024 * 1024;
const MAX_SQL_BYTES: i32 = 65_536;
const MAX_EXPRESSION_DEPTH: i32 = 200;
const MAX_COMPOUND_SELECT: i32 = 20;
const MAX_BOUND_PARAMETERS: i32 = 1_000;

/// The page ceiling on each database the connection has, so the rows one instance holds
/// cannot grow the process without limit (16,384 pages of 4 KiB, 64 MiB per schema).
const MAX_DATABASE_PAGES: i64 = 16_384;

/// `PRAGMA temp_store = MEMORY`, so the engine's temporary database is in memory rather
/// than a file in the host's temp directory.
const TEMP_STORE_MEMORY: i32 = 2;

/// `PRAGMA cache_size`, in KiB (the pragma reads a negative value as a byte budget): what
/// ONE instance's connection may hold in its page cache. One emulator serves many DB
/// instances at once, each with its own connection, so the per-connection cache is set
/// well below the engine's default rather than left to multiply.
const MAX_PAGE_CACHE_KIB: i64 = -256;

/// The schemas the page ceiling is applied to: the instance's own tables and the
/// temporary database a sort or a materialised CTE spills into.
const CAPPED_SCHEMAS: [&str; 2] = ["main", "temp"];

/// What went wrong with a statement, in terms both wires can render: each one maps a kind
/// to its own engine's code (a PostgreSQL SQLSTATE, a MySQL error number) and sends the
/// engine's real message with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorKind {
    /// The statement does not parse, or names something the engine does not define.
    Syntax,
    /// A table, column or function the statement names does not exist.
    UndefinedObject,
    /// A database the statement names does not exist.
    UndefinedDatabase,
    /// A unique or primary-key constraint refused the row.
    UniqueViolation,
    /// A NOT NULL constraint refused the row.
    NotNullViolation,
    /// A CHECK constraint refused the row.
    CheckViolation,
    /// A foreign-key constraint refused the row.
    ForeignKeyViolation,
    /// A value is not one its column accepts.
    DataError,
    /// The statement ran past [`STATEMENT_DEADLINE`].
    Cancelled,
    /// The statement asked for more than this emulator's endpoint serves: a result set
    /// past its caps, or more session state than one session may hold.
    LimitExceeded,
    /// The caller is not allowed to do this.
    InsufficientPrivilege,
    /// The engine cannot serve this at all.
    Internal,
}

/// One failed statement: the kind of failure and the ENGINE's own message for it.
#[derive(Debug, Clone)]
pub struct EngineError {
    pub kind: EngineErrorKind,
    pub message: String,
}

impl EngineError {
    /// A failure of this emulator's own engine, with `message` as the engine worded it.
    pub fn new(kind: EngineErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// One column of a result set: the name the engine reports for it and the type its
/// declaration gave, which is what decides the type code the wire reports.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub declared_type: Option<String>,
}

/// One value in a result set, as the engine holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    /// What this value costs against [`MAX_RESULT_BYTES`].
    fn byte_cost(&self) -> usize {
        match self {
            Self::Null => 1,
            Self::Integer(_) | Self::Real(_) => 8,
            Self::Text(text) => text.len(),
            Self::Blob(bytes) => bytes.len(),
        }
    }
}

/// What one statement produced: its columns and rows (empty for a statement that returns
/// none), how many rows it changed, and the key the engine assigned on an insert.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub affected: u64,
    pub last_insert_id: i64,
}

impl ResultSet {
    /// A result set with no columns at all — what a statement that returns no rows
    /// produced, carrying only what it changed.
    pub fn changed(affected: u64, last_insert_id: i64) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            affected,
            last_insert_id,
        }
    }

    /// A result set built here rather than by the engine: the columns and rows a SERVER
    /// command answers with (`SHOW`), which are the server's own state and not a query
    /// over the instance's tables.
    pub fn of_rows(columns: Vec<ColumnInfo>, rows: Vec<Vec<Cell>>) -> Self {
        Self {
            columns,
            rows,
            affected: 0,
            last_insert_id: 0,
        }
    }
}

/// One DB instance's database.
pub struct InstanceDatabase {
    conn: Mutex<Connection>,
}

/// Written by hand rather than derived: a connection is not `Debug`, and what a reader
/// needs here is that the instance has one.
impl std::fmt::Debug for InstanceDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceDatabase").finish()
    }
}

impl InstanceDatabase {
    /// Open the one empty in-memory database a DB instance's engine serves, with every
    /// bound above in force before any client statement can reach it.
    pub fn open() -> Result<Self, EngineError> {
        let conn = Connection::open_in_memory().map_err(unavailable)?;
        // Temp database in memory FIRST, so naming `temp` below opens it in memory and
        // not as a host file; then the page ceiling on both schemas.
        conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)
            .map_err(unavailable)?;
        conn.pragma_update(None, "cache_size", MAX_PAGE_CACHE_KIB)
            .map_err(unavailable)?;
        for schema in CAPPED_SCHEMAS {
            conn.pragma_update(Some(schema), "max_page_count", MAX_DATABASE_PAGES)
                .map_err(unavailable)?;
        }
        for (limit, value) in [
            // No second database, ever: this also takes `VACUUM INTO` away, which the
            // engine implements by attaching its output file.
            (Limit::SQLITE_LIMIT_ATTACHED, 0),
            (Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES),
            (Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES),
            (Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPRESSION_DEPTH),
            (Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT),
            (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_BOUND_PARAMETERS),
        ] {
            conn.set_limit(limit, value).map_err(unavailable)?;
        }
        for (config, enabled) in [
            (rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
            (rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
        ] {
            conn.set_db_config(config, enabled).map_err(unavailable)?;
        }
        // Installed LAST, so the settings above — themselves pragmas — are not refused
        // by it.
        conn.authorizer(Some(confine_to_the_instance));
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// The connection, recovering a lock a panicking session poisoned: a long-lived
    /// emulator has to keep serving every later session.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `sql` with `params` bound to its placeholders in order, and answer with what
    /// the engine produced.
    ///
    /// Blocking, CPU-bound work: a wire handler calls it on a blocking task so a long
    /// statement cannot starve the async runtime, and the statement itself is bounded by
    /// [`STATEMENT_DEADLINE`] and by the result caps.
    pub fn execute(&self, sql: &str, params: &[Option<String>]) -> Result<ResultSet, EngineError> {
        let conn = self.lock();
        let deadline = Instant::now() + STATEMENT_DEADLINE;
        conn.progress_handler(PROGRESS_HANDLER_OPS, Some(move || Instant::now() >= deadline));
        let answer = run_statement(&conn, sql, params);
        conn.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        answer
    }

    /// Whether the engine is inside a transaction a session opened, which is what a
    /// PostgreSQL `ReadyForQuery` and a MySQL status flag report.
    #[must_use]
    pub fn in_transaction(&self) -> bool {
        !self.lock().is_autocommit()
    }

    /// The columns `sql` would produce, without running it: what the extended query
    /// protocol's `Describe` answers.
    pub fn describe(&self, sql: &str) -> Result<Vec<ColumnInfo>, EngineError> {
        let conn = self.lock();
        let statement = conn.prepare(sql).map_err(from_engine)?;
        Ok(statement
            .columns()
            .iter()
            .map(|column| ColumnInfo {
                name: column.name().to_string(),
                declared_type: column.decl_type().map(str::to_string),
            })
            .collect())
    }

    /// Roll back the transaction a session left open, if it left one.
    ///
    /// The sessions of one instance share the connection, so a `BEGIN` that is never
    /// committed would otherwise hold the instance's uncommitted rows for whoever
    /// connects next. A wire handler calls this when a session ends — including when the
    /// session is severed by a reboot or a stop — which is the engine behaviour a client
    /// sees: work it never committed is gone, work it committed stays.
    pub fn rollback_open_transaction(&self) {
        let conn = self.lock();
        if !conn.is_autocommit() {
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
}

/// An engine that could not be brought up or configured at all.
fn unavailable(error: rusqlite::Error) -> EngineError {
    EngineError::new(EngineErrorKind::Internal, error.to_string())
}

/// Prepare, bind and run one statement, reading its rows out under the result caps.
fn run_statement(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
) -> Result<ResultSet, EngineError> {
    let mut statement = conn.prepare(sql).map_err(from_engine)?;
    if statement.parameter_count() != params.len() {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "bind message supplies {} parameters, but the statement requires {}",
                params.len(),
                statement.parameter_count()
            ),
        ));
    }
    for (position, value) in params.iter().enumerate() {
        statement
            .raw_bind_parameter(position + 1, value.as_deref())
            .map_err(from_engine)?;
    }
    let columns: Vec<ColumnInfo> = statement
        .columns()
        .iter()
        .map(|column| ColumnInfo {
            name: column.name().to_string(),
            declared_type: column.decl_type().map(str::to_string),
        })
        .collect();
    if columns.is_empty() {
        let affected = statement.raw_execute().map_err(from_engine)?;
        return Ok(ResultSet::changed(
            u64::try_from(affected).unwrap_or(u64::MAX),
            conn.last_insert_rowid(),
        ));
    }

    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut cursor = statement.raw_query();
    while let Some(row) = cursor.next().map_err(from_engine)? {
        if rows.len() >= MAX_RESULT_ROWS {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!("the result set is larger than this emulator serves ({MAX_RESULT_ROWS} rows)"),
            ));
        }
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let cell = match row.get_ref(index).map_err(from_engine)? {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(value) => Cell::Integer(value),
                ValueRef::Real(value) => Cell::Real(value),
                ValueRef::Text(bytes) => {
                    Cell::Text(String::from_utf8_lossy(bytes).into_owned())
                }
                ValueRef::Blob(bytes) => Cell::Blob(bytes.to_vec()),
            };
            bytes = bytes.saturating_add(cell.byte_cost());
            if bytes > MAX_RESULT_BYTES {
                return Err(EngineError::new(
                    EngineErrorKind::LimitExceeded,
                    format!(
                        "the result set is larger than this emulator serves ({} MiB of values)",
                        MAX_RESULT_BYTES / (1024 * 1024)
                    ),
                ));
            }
            values.push(cell);
        }
        rows.push(values);
    }
    // `changes()` and the assigned key belong to a statement that wrote rows — an
    // `INSERT ... RETURNING` both returns rows and changes them.
    Ok(ResultSet {
        columns,
        rows,
        affected: conn.changes(),
        last_insert_id: conn.last_insert_rowid(),
    })
}

/// Classify what the engine refused, keeping ITS message.
fn from_engine(error: rusqlite::Error) -> EngineError {
    let message = engine_message(&error);
    let kind = match &error {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
            rusqlite::ErrorCode::ConstraintViolation => constraint_kind(failure.extended_code),
            rusqlite::ErrorCode::OperationInterrupted => EngineErrorKind::Cancelled,
            rusqlite::ErrorCode::AuthorizationForStatementDenied => {
                EngineErrorKind::InsufficientPrivilege
            }
            rusqlite::ErrorCode::TypeMismatch => EngineErrorKind::DataError,
            rusqlite::ErrorCode::Unknown => EngineErrorKind::Syntax,
            _ => EngineErrorKind::Internal,
        },
        rusqlite::Error::InvalidParameterCount(..) | rusqlite::Error::InvalidQuery => {
            EngineErrorKind::Syntax
        }
        _ => EngineErrorKind::Internal,
    };
    // SQLite words a missing table or column as "no such <thing>", which is the one
    // failure class every client branches on separately.
    let kind = if message.contains("no such table")
        || message.contains("no such column")
        || message.contains("no such function")
        || message.contains("no such index")
        || message.contains("no such view")
    {
        EngineErrorKind::UndefinedObject
    } else {
        kind
    };
    EngineError::new(kind, message)
}

/// Which constraint the engine refused the row for, from its extended result code.
fn constraint_kind(extended_code: std::os::raw::c_int) -> EngineErrorKind {
    // The extended codes SQLite documents for SQLITE_CONSTRAINT: the primary result code
    // (19) in the low byte and the constraint in the high byte.
    const SQLITE_CONSTRAINT_CHECK: std::os::raw::c_int = 275;
    const SQLITE_CONSTRAINT_FOREIGNKEY: std::os::raw::c_int = 787;
    const SQLITE_CONSTRAINT_NOTNULL: std::os::raw::c_int = 1299;
    const SQLITE_CONSTRAINT_PRIMARYKEY: std::os::raw::c_int = 1555;
    const SQLITE_CONSTRAINT_UNIQUE: std::os::raw::c_int = 2067;
    match extended_code {
        SQLITE_CONSTRAINT_CHECK => EngineErrorKind::CheckViolation,
        SQLITE_CONSTRAINT_FOREIGNKEY => EngineErrorKind::ForeignKeyViolation,
        SQLITE_CONSTRAINT_NOTNULL => EngineErrorKind::NotNullViolation,
        SQLITE_CONSTRAINT_PRIMARYKEY | SQLITE_CONSTRAINT_UNIQUE => {
            EngineErrorKind::UniqueViolation
        }
        _ => EngineErrorKind::CheckViolation,
    }
}

/// The engine's own sentence for a failure, which is what the client is told.
fn engine_message(error: &rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        other => other.to_string(),
    }
}

/// What a client's SQL may do inside one DB instance's database.
///
/// DEFAULT DENY: an action not named here is refused, so an engine capability this
/// emulator has not considered — including one a later SQLite build adds, which arrives as
/// a variant this match has never seen — cannot be reached from a session. What is allowed
/// is what the data plane exists for: reading and writing the instance's own tables, the
/// DDL that defines them, transactions, and pure computation over the values in them.
///
/// What that refuses, and why it matters: `ATTACH`/`DETACH` would make a host path visible
/// to SQL (and `VACUUM INTO` is implemented by attaching its output file), `PRAGMA` reaches
/// engine configuration including temp-file paths, and a virtual table is arbitrary
/// registered code. None of them is something a PostgreSQL or MySQL client can ask for over
/// its own wire anyway, so refusing them takes nothing away from the engine this plane
/// emulates. The functions are an allow-list of their own
/// ([`::service_common::sqlite_confinement`]), which is where the host-reaching ones are
/// named absent.
fn confine_to_the_instance(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Recursive
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Reindex { .. }
        | AuthAction::Analyze { .. } => Authorization::Allow,
        // Pure computation over the values in those tables, as an ALLOW-LIST: a function
        // this emulator has not reviewed — including one a future engine build defines —
        // falls through to the deny arm below.
        AuthAction::Function { function_name } => {
            if ::service_common::sqlite_confinement::sql_function_is_permitted(function_name) {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        // Everything that could reach outside this instance's own database, and everything
        // this emulator has not considered.
        _ => Authorization::Deny,
    }
}
