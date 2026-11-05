// SPDX-License-Identifier: CC0-1.0

//! What the two engine wires share: the facts a login is admitted against, the server
//! commands a client sends that are not queries over its own tables (`SHOW`, `SET`, `USE`),
//! and the session values a statement can read (`current_user`, `version()`, `@@version`).
//!
//! Everything here is decided from this service's OWN STATE — the stored DB instance, its
//! master user, its parameter group, the session's own socket — and every statement that
//! is a query over the instance's tables is handed to the SQL engine unchanged except for
//! the session values substituted into it. Nothing is answered from the shape of the
//! statement's text.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use sha1::Sha1;
use sha2::{Digest, Sha256};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect};
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};
use tokio::sync::watch;

use super::sql_engine::{Cell, ColumnInfo, EngineError, EngineErrorKind, InstanceDatabase, ResultSet};
use super::EngineWire;
use crate::state::ComAmazonawsRdsAmazonrdsv19State;

/// How many bytes of SQL one statement may carry, matching the engine's own statement
/// limit: a longer one is refused before it is tokenized.
pub const MAX_STATEMENT_BYTES: usize = 65_536;

/// How many statements one simple-query message may carry.
pub const MAX_STATEMENTS_PER_QUERY: usize = 32;

/// The database a PostgreSQL DB instance always has, whatever `DBName` named: RDS creates
/// it with every PostgreSQL instance and it is the one a client connects to when it names
/// no other.
pub const POSTGRES_DEFAULT_DATABASE: &str = "postgres";

/// The databases a MySQL server always has: its own system schemas, which a client may
/// connect to and read.
pub const MYSQL_SYSTEM_DATABASES: [&str; 4] =
    ["information_schema", "mysql", "performance_schema", "sys"];

/// The master user's password, kept only as the DIGESTS a login can be verified against.
///
/// The password itself is `@sensitive` in the model, is never reported by any operation
/// and is not stored: what is stored is what a server needs to recognise it again. The
/// MySQL digest is `SHA1(SHA1(password))`, which is exactly the value a MySQL server keeps
/// for `mysql_native_password` and the only form the challenge-response handshake can be
/// checked against.
#[derive(Clone, PartialEq, Eq)]
pub struct MasterPasswordHash {
    /// `SHA256(username \0 password)`, which a password presented in the clear over the
    /// PostgreSQL wire is checked against. Salted with the user name so the same password
    /// under two names does not store the same digest.
    login_digest: [u8; 32],
    /// `SHA1(SHA1(password))` — see the type's own note.
    mysql_native: [u8; 20],
}

/// Written by hand rather than derived, so no formatter — a log line, a test failure — can
/// print anything derived from the master password.
impl std::fmt::Debug for MasterPasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterPasswordHash(<redacted>)")
    }
}

impl MasterPasswordHash {
    /// The digests of `password` as the master user `username`.
    #[must_use]
    pub fn new(username: &str, password: &str) -> Self {
        Self {
            login_digest: login_digest(username, password),
            mysql_native: mysql_native_hash(password),
        }
    }

    /// Whether `password`, presented in the clear by `username`, is this password.
    #[must_use]
    pub fn accepts_cleartext(&self, username: &str, password: &str) -> bool {
        login_digest(username, password) == self.login_digest
    }

    /// Whether `response` is the `mysql_native_password` proof of this password for the
    /// server's `nonce`.
    ///
    /// The client sends `SHA1(password) XOR SHA1(nonce || SHA1(SHA1(password)))`, so the
    /// server recovers `SHA1(password)` from the response and the hash it stores, and the
    /// proof holds exactly when hashing that recovered value again gives the stored hash.
    #[must_use]
    pub fn accepts_mysql_native(&self, nonce: &[u8], response: &[u8]) -> bool {
        if response.len() != self.mysql_native.len() {
            return false;
        }
        let mut hasher = Sha1::new();
        hasher.update(nonce);
        hasher.update(self.mysql_native);
        let mask = hasher.finalize();
        let mut recovered = [0u8; 20];
        for (index, byte) in recovered.iter_mut().enumerate() {
            let (Some(sent), Some(mask)) = (response.get(index), mask.get(index)) else {
                return false;
            };
            *byte = sent ^ mask;
        }
        Sha1::digest(recovered).as_slice() == self.mysql_native
    }
}

/// `SHA256(username \0 password)`.
fn login_digest(username: &str, password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update([0u8]);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

/// `SHA1(SHA1(password))`, the hash a MySQL server stores for a native-password account.
fn mysql_native_hash(password: &str) -> [u8; 20] {
    Sha1::digest(Sha1::digest(password.as_bytes())).into()
}

/// What a wire handler needs to serve every session on one DB instance's listener.
#[derive(Clone)]
pub struct SessionContext {
    /// This service's store, read fresh at each login so a session sees the instance as
    /// it is NOW — its status, its master user, its parameter group.
    pub state: ::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    /// The region the instance was created in; half of its key.
    pub region: String,
    /// The instance's `DbiResourceId`, which never changes — so a rename under a live
    /// connection cannot make the listener point at nothing.
    pub resource_id: String,
    /// The instance's database.
    pub database: Arc<InstanceDatabase>,
    /// The core's TLS server, for the in-band upgrade each wire negotiates.
    pub upgrader: ::emulator_server::TlsUpgrader,
    /// The engine's run: a session ends when this changes (a reboot, a stop).
    pub generation: watch::Receiver<u64>,
    /// The connection ids this endpoint has handed out.
    pub next_connection_id: Arc<AtomicU32>,
}

impl SessionContext {
    /// The next connection id for a session on this endpoint — the PostgreSQL backend pid
    /// and the MySQL connection id, which both identify one live session.
    pub fn take_connection_id(&self) -> u32 {
        self.next_connection_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The facts this session is admitted against, read once under the state lock.
    ///
    /// `None` when the store holds no such instance any more (it was deleted while the
    /// listener was still accepting), which the wire answers as an engine that is not
    /// there.
    pub fn instance_facts(&self) -> Option<InstanceFacts> {
        let mut store = self.state.lock();
        // The transitions the clock has already made due — owed at this instant whether
        // or not anybody called, exactly as every control-plane handler materialises them
        // — so a session is admitted against the status the instance has NOW. A client
        // that connects without calling the control plane first still finds the instance
        // it would have been told about.
        store.settle_due_transitions(&self.region);
        let record = store.instance_by_resource_id(&self.region, &self.resource_id)?;
        let wire = super::engine_wire(&record.engine)?;
        let endpoint_host = store
            .engine_endpoint(&(record.region.clone(), record.identifier.clone()))
            .map(|endpoint| endpoint.endpoint.host.clone())
            .unwrap_or_else(|| "localhost".to_string());
        let server_version = record
            .engine_version
            .clone()
            .or_else(|| {
                crate::ops::describe_db_engine_versions::resolve_version(&record.engine, None)
                    .ok()
                    .flatten()
                    .map(|entry| entry.engine_version.to_string())
            })
            .unwrap_or_default();
        let mut databases = Vec::new();
        if let Some(name) = &record.db_name {
            databases.push(name.clone());
        }
        match wire {
            EngineWire::Postgres => {
                if !databases.iter().any(|name| name == POSTGRES_DEFAULT_DATABASE) {
                    databases.push(POSTGRES_DEFAULT_DATABASE.to_string());
                }
            }
            EngineWire::MySql => {
                databases.extend(MYSQL_SYSTEM_DATABASES.iter().map(|name| (*name).to_string()));
            }
        }
        let default_database = match wire {
            EngineWire::Postgres => record
                .db_name
                .clone()
                .unwrap_or_else(|| POSTGRES_DEFAULT_DATABASE.to_string()),
            EngineWire::MySql => record.db_name.clone().unwrap_or_default(),
        };
        Some(InstanceFacts {
            identifier: record.identifier.clone(),
            engine: record.engine.clone(),
            status: record.status.clone(),
            wire,
            server_version,
            master_username: record.master_username.clone(),
            password: record.master_user_password.clone(),
            databases,
            default_database,
            requires_secure_transport: store.requires_secure_transport(record),
            endpoint_host,
        })
    }
}

/// The instance a session is being opened on, as the store holds it right now.
#[derive(Debug, Clone)]
pub struct InstanceFacts {
    pub identifier: String,
    pub engine: String,
    pub status: String,
    pub wire: EngineWire,
    /// The version the engine reports for itself: the instance's `EngineVersion`.
    pub server_version: String,
    pub master_username: Option<String>,
    pub password: Option<MasterPasswordHash>,
    /// The databases a login may name.
    pub databases: Vec<String>,
    /// The database a login that names none gets.
    pub default_database: String,
    /// Whether the instance's parameter group requires an encrypted session.
    pub requires_secure_transport: bool,
    /// The host the instance's listener is bound to, which is the host a refusal names.
    pub endpoint_host: String,
}

/// Why a login was refused. Each wire words it in its own protocol's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginRefusal {
    /// The instance is not running: it is still coming up, stopped, rebooting or on its
    /// way out, so there is no engine to connect to.
    NotRunning { status: String },
    /// The session is in the clear and the instance's parameter group requires TLS.
    PlaintextRefused,
    /// The login named a database this instance does not have.
    UnknownDatabase { database: String },
    /// The login did not prove the master user's password — a wrong password, a password
    /// for another user, or no password at all.
    AuthenticationFailed { user: String, password_given: bool },
}

impl LoginRefusal {
    /// What the refusal is, in one word, for the audit line
    /// [`record_refused_login`] writes.
    fn outcome(&self) -> &'static str {
        match self {
            Self::NotRunning { .. } => "engine-not-running",
            Self::PlaintextRefused => "encryption-required",
            Self::UnknownDatabase { .. } => "unknown-database",
            Self::AuthenticationFailed { .. } => "authentication-failed",
        }
    }
}

/// Record a login an engine endpoint refused.
///
/// Every attempt on a published endpoint is attributable: the DB instance it was made on
/// and why it was refused. The credential the client presented is NEVER logged, and
/// neither is the user or the database it named — those are the client's own values, and
/// the instance plus the reason is what a reader needs.
pub fn record_refused_login(facts: &InstanceFacts, refusal: &LoginRefusal) {
    ::tracing::info!(
        audit = true,
        operation = "DbInstanceEngineSession",
        outcome = refusal.outcome(),
        db_instance_identifier = %facts.identifier,
        "an engine session on a DB instance endpoint was refused"
    );
}

/// How a client proved its password on its own wire.
pub enum PasswordProof<'a> {
    /// The password as the client sent it (the PostgreSQL cleartext password message).
    Cleartext(&'a str),
    /// The MySQL native-password challenge response, against the nonce the greeting
    /// carried.
    MysqlNative { nonce: &'a [u8], response: &'a [u8] },
    /// The client sent no password at all.
    None,
}

impl PasswordProof<'_> {
    /// Whether the client sent anything to be checked, which is what a MySQL refusal
    /// reports as `using password: YES/NO`.
    fn password_given(&self) -> bool {
        match self {
            Self::Cleartext(password) => !password.is_empty(),
            Self::MysqlNative { response, .. } => !response.is_empty(),
            Self::None => false,
        }
    }
}

/// Decide the refusals that come BEFORE any credential is read: an engine that is not
/// running, and a session that may not be in the clear.
///
/// Separate from [`admit`] because that is the order the engines work in — a server tells
/// a client its connection is not admitted at all before it asks for a password — and
/// [`admit`] checks these again, so a wire that only calls `admit` is still correct.
pub fn precheck(facts: &InstanceFacts, encrypted: bool) -> Result<(), LoginRefusal> {
    if facts.status != crate::state::DB_INSTANCE_STATUS_AVAILABLE {
        return Err(LoginRefusal::NotRunning {
            status: facts.status.clone(),
        });
    }
    if facts.requires_secure_transport && !encrypted {
        return Err(LoginRefusal::PlaintextRefused);
    }
    Ok(())
}

/// Decide whether a login is admitted, in the order the engines decide it: an engine that
/// is not running answers nothing else, a session that may not be in the clear is refused
/// before any credential is read, and the password is checked before the database is
/// resolved (so a wrong password cannot be used to discover which databases exist).
pub fn admit(
    facts: &InstanceFacts,
    user: &str,
    database: Option<&str>,
    encrypted: bool,
    proof: &PasswordProof<'_>,
) -> Result<String, LoginRefusal> {
    if facts.status != crate::state::DB_INSTANCE_STATUS_AVAILABLE {
        return Err(LoginRefusal::NotRunning {
            status: facts.status.clone(),
        });
    }
    if facts.requires_secure_transport && !encrypted {
        return Err(LoginRefusal::PlaintextRefused);
    }
    let authenticated = facts
        .master_username
        .as_ref()
        .is_some_and(|master| master == user)
        && match (&facts.password, proof) {
            (Some(stored), PasswordProof::Cleartext(password)) => {
                stored.accepts_cleartext(user, password)
            }
            (Some(stored), PasswordProof::MysqlNative { nonce, response }) => {
                stored.accepts_mysql_native(nonce, response)
            }
            // An instance created with no master password has no credential to check a
            // login against, so nothing can authenticate as its master user.
            (_, PasswordProof::None) | (None, _) => false,
        };
    if !authenticated {
        return Err(LoginRefusal::AuthenticationFailed {
            user: user.to_string(),
            password_given: proof.password_given(),
        });
    }
    match database {
        None | Some("") => Ok(facts.default_database.clone()),
        Some(named) => {
            if facts.databases.iter().any(|known| known == named) {
                Ok(named.to_string())
            } else {
                Err(LoginRefusal::UnknownDatabase {
                    database: named.to_string(),
                })
            }
        }
    }
}

/// What the TLS handshake of a session negotiated, as far as it can be observed here.
#[derive(Debug, Clone, Default)]
pub struct TlsFacts {
    /// The protocol version, e.g. `TLSv1.3`.
    pub protocol: Option<String>,
    /// The cipher suite, under its IANA name.
    pub cipher: Option<String>,
}

/// One live session: who is connected, to which database, over what, and the settings it
/// has changed for itself.
#[derive(Debug, Clone)]
pub struct SessionFacts {
    pub wire: EngineWire,
    pub user: String,
    pub database: String,
    /// The PostgreSQL backend pid / MySQL connection id of this session.
    pub connection_id: u32,
    /// `None` for a session in the clear.
    pub tls: Option<TlsFacts>,
    pub server_version: String,
    pub engine: String,
    /// Whether the instance's parameter group requires an encrypted session, which is
    /// what `SHOW rds.force_ssl` / `SHOW VARIABLES LIKE 'require_secure_transport'`
    /// report.
    pub requires_secure_transport: bool,
    /// The settings this session has changed with `SET`.
    pub settings: BTreeMap<String, String>,
    /// The databases this instance has, which is what a `USE` is resolved against.
    pub databases: Vec<String>,
}

impl SessionFacts {
    /// Whether this session's socket is encrypted.
    #[must_use]
    pub fn encrypted(&self) -> bool {
        self.tls.is_some()
    }

    /// The value of the setting `name` for this session: what it `SET` for itself, else
    /// what the server reports.
    #[must_use]
    pub fn setting(&self, name: &str) -> Option<String> {
        let lowered = name.to_ascii_lowercase();
        if let Some(value) = self.settings.get(&lowered) {
            return Some(value.clone());
        }
        server_setting(self, &lowered)
    }
}

/// The settings the server itself answers for, by lowercased name.
///
/// Every value is this emulator's own state: the instance's engine version, the session's
/// own socket, the parameter group's requirement. A name that is not here is not a
/// setting this server has.
fn server_setting(facts: &SessionFacts, lowered: &str) -> Option<String> {
    let on_off = |value: bool| Some(if value { "on" } else { "off" }.to_string());
    match lowered {
        // The server accepts TLS: the core gives every engine endpoint its TLS material,
        // so the in-band upgrade is always available.
        "ssl" => on_off(true),
        "server_version" | "version" => Some(facts.server_version.clone()),
        "server_encoding" | "client_encoding" | "character_set_server"
        | "character_set_client" | "character_set_results" | "character_set_connection" => {
            Some(match facts.wire {
                // The character set an RDS MySQL instance runs under, and the one its
                // `utf8mb4` tables are stored in.
                EngineWire::MySql => "utf8mb4".to_string(),
                EngineWire::Postgres => "UTF8".to_string(),
            })
        }
        "collation_server" | "collation_connection" => match facts.wire {
            EngineWire::MySql => Some("utf8mb4_0900_ai_ci".to_string()),
            EngineWire::Postgres => None,
        },
        "datestyle" => Some("ISO, MDY".to_string()),
        "timezone" | "time_zone" => Some("UTC".to_string()),
        "integer_datetimes" | "standard_conforming_strings" => on_off(true),
        // The master user is not a superuser on either engine (`rds_superuser` /
        // the RDS master account), which is what the privilege refusals rest on.
        "is_superuser" => on_off(false),
        "session_authorization" | "current_user" | "user" => Some(facts.user.clone()),
        "transaction_isolation" | "tx_isolation" => Some(match facts.wire {
            EngineWire::MySql => "REPEATABLE-READ".to_string(),
            EngineWire::Postgres => "read committed".to_string(),
        }),
        "autocommit" => match facts.wire {
            EngineWire::MySql => Some("ON".to_string()),
            EngineWire::Postgres => None,
        },
        "rds.force_ssl" => match facts.wire {
            EngineWire::Postgres => Some(if facts.requires_secure_transport { "1" } else { "0" }.to_string()),
            EngineWire::MySql => None,
        },
        "require_secure_transport" => match facts.wire {
            EngineWire::MySql => Some(if facts.requires_secure_transport { "ON" } else { "OFF" }.to_string()),
            EngineWire::Postgres => None,
        },
        "ssl_cipher" => facts
            .tls
            .as_ref()
            .map(|tls| tls.cipher.clone().unwrap_or_default()),
        "ssl_version" => facts
            .tls
            .as_ref()
            .map(|tls| tls.protocol.clone().unwrap_or_default()),
        _ => None,
    }
}

/// What a statement did, as the wire has to report it.
#[derive(Debug, Clone)]
pub struct StatementAnswer {
    /// The leading keyword(s) of the statement, uppercased — `SELECT`, `CREATE TABLE` —
    /// which is what a PostgreSQL `CommandComplete` tag names.
    pub verb: String,
    /// The rows and the counts the statement produced.
    pub result: ResultSet,
    /// A setting this statement changed for the session, which the caller writes back
    /// onto its [`SessionFacts`].
    pub setting_changed: Option<(String, String)>,
    /// The database this statement moved the session to (`USE`), which the caller writes
    /// back the same way.
    pub database_changed: Option<String>,
}

impl StatementAnswer {
    fn new(verb: impl Into<String>, result: ResultSet) -> Self {
        Self {
            verb: verb.into(),
            result,
            setting_changed: None,
            database_changed: None,
        }
    }
}

/// Run one statement for a session: a server command answered from this emulator's own
/// state, or a statement handed to the SQL engine with the session's values substituted
/// into it.
///
/// Blocking: the engine runs the statement on the calling thread, so a wire handler calls
/// this on a blocking task.
pub fn run_statement(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    sql: &str,
    params: &[Option<String>],
) -> Result<StatementAnswer, EngineError> {
    if sql.len() > MAX_STATEMENT_BYTES {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!("the statement is longer than this emulator serves ({MAX_STATEMENT_BYTES} bytes)"),
        ));
    }
    // Two readings of the same tokens: the significant ones decide what the statement
    // IS, and the whole stream — whitespace included — is what the engine is handed, so a
    // statement the engine runs is the client's own text with only the session's values
    // substituted into it.
    let stream = statement_tokens(facts.wire, sql)?;
    let tokens: Vec<Token> = stream
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .cloned()
        .collect();
    if tokens.is_empty() {
        // An empty query: the wires report it as such and nothing runs.
        return Ok(StatementAnswer::new(String::new(), ResultSet::default()));
    }
    let words = leading_words(&tokens, 3);
    if let Some(answer) = server_command(facts, &words, &tokens)? {
        return Ok(answer);
    }
    let verb = statement_verb(&words);
    let prepared = render_for_engine(facts, &stream);
    let result = database.execute(&prepared, params)?;
    Ok(StatementAnswer::new(verb, result))
}

/// The columns a statement would produce, as the engine reports them for it.
///
/// A server command is not the engine's to plan, and neither is a statement it cannot
/// prepare, so both describe as producing no columns — what they do produce is described
/// when they run.
#[must_use]
pub fn describe_statement(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    sql: &str,
) -> Vec<ColumnInfo> {
    let Ok(stream) = statement_tokens(facts.wire, sql) else {
        return Vec::new();
    };
    let tokens: Vec<Token> = stream
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .cloned()
        .collect();
    let words = leading_words(&tokens, 3);
    if matches!(server_command(facts, &words, &tokens), Ok(Some(_)) | Err(_)) {
        return Vec::new();
    }
    database
        .describe(&render_for_engine(facts, &stream))
        .unwrap_or_default()
}

/// The server commands a client sends that are not queries over its own tables.
///
/// `None` means the statement is the engine's to run.
fn server_command(
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<Option<StatementAnswer>, EngineError> {
    let first = words.first().map(String::as_str).unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    match (facts.wire, first, second) {
        // "ALTER SYSTEM is used for changing server configuration parameters across the
        // entire database cluster ... only superusers can change configuration
        // parameters" — and the RDS master user is not one.
        (EngineWire::Postgres, "ALTER", "SYSTEM") => Err(EngineError::new(
            EngineErrorKind::InsufficientPrivilege,
            "must be superuser to execute ALTER SYSTEM command".to_string(),
        )),
        (EngineWire::Postgres, "SHOW", _) => Ok(Some(postgres_show(facts, tokens)?)),
        (EngineWire::Postgres, "SET", _) => Ok(Some(postgres_set(facts, tokens)?)),
        (EngineWire::MySql, "SHOW", _) => mysql_show(facts, words, tokens).map(Some),
        // Setting a GLOBAL system variable is a server-wide change, which the RDS master
        // user does not hold the privilege for; a parameter group is how it is changed.
        (EngineWire::MySql, "SET", "GLOBAL") | (EngineWire::MySql, "SET", "PERSIST") => {
            Err(EngineError::new(
                EngineErrorKind::InsufficientPrivilege,
                "Access denied; you need (at least one of) the SUPER or SYSTEM_VARIABLES_ADMIN \
                 privilege(s) for this operation"
                    .to_string(),
            ))
        }
        (EngineWire::MySql, "SET", _) => Ok(Some(mysql_set(facts, tokens)?)),
        (EngineWire::MySql, "USE", _) => {
            let named: String = tokens
                .iter()
                .skip(1)
                .map(|token| token.to_string().replace(['`', '"'], ""))
                .collect();
            if facts
                .databases
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&named))
            {
                let mut answer = StatementAnswer::new("USE", ResultSet::default());
                answer.database_changed = Some(named);
                Ok(Some(answer))
            } else {
                Err(EngineError::new(
                    EngineErrorKind::UndefinedDatabase,
                    format!("Unknown database '{named}'"),
                ))
            }
        }
        _ => Ok(None),
    }
}

/// `SHOW <name>` on the PostgreSQL wire: one row, one column named after the setting.
fn postgres_show(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    // A namespaced setting (`rds.force_ssl`) is several tokens; the name is everything
    // after SHOW, rejoined as the client wrote it.
    let name: String = tokens
        .iter()
        .skip(1)
        .map(|token| token.to_string().replace('"', ""))
        .collect();
    let lowered = name.to_ascii_lowercase();
    match facts.setting(&lowered) {
        Some(value) => Ok(StatementAnswer::new(
            "SHOW",
            ResultSet::of_rows(
                vec![ColumnInfo {
                    name: lowered,
                    declared_type: Some("text".to_string()),
                }],
                vec![vec![Cell::Text(value)]],
            ),
        )),
        None => Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("unrecognized configuration parameter \"{name}\""),
        )),
    }
}

/// How many settings one session may hold at once, and how long a name and a value it
/// keeps may be.
///
/// A `SET` records a name and a value a CLIENT chose, so the map they go in is bounded the
/// same way the prepared statements and portals of a session are: past the cap a further
/// `SET` is refused rather than growing what the session holds, and each entry is clamped
/// so one of them cannot cost a whole statement's worth of bytes. The cap is well above
/// what a driver sets on a connection (a handful of `SET`s at startup) and far below where
/// the map matters.
pub const MAX_SESSION_SETTINGS: usize = 64;
const MAX_SETTING_NAME_BYTES: usize = 128;
const MAX_SETTING_VALUE_BYTES: usize = 1024;

/// The setting a `SET` records, refused where it would grow the session past
/// [`MAX_SESSION_SETTINGS`] or carry more than one entry's worth of bytes.
///
/// A name the session ALREADY holds is a replacement rather than a new entry, so a client
/// may keep setting the same parameter for as long as it likes.
fn recorded_setting(
    facts: &SessionFacts,
    name: String,
    value: String,
) -> Result<(String, String), EngineError> {
    if name.len() > MAX_SETTING_NAME_BYTES || value.len() > MAX_SETTING_VALUE_BYTES {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!(
                "a session setting of this endpoint carries at most \
                 {MAX_SETTING_NAME_BYTES} bytes of name and {MAX_SETTING_VALUE_BYTES} \
                 bytes of value"
            ),
        ));
    }
    if !facts.settings.contains_key(&name) && facts.settings.len() >= MAX_SESSION_SETTINGS {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!("this endpoint holds at most {MAX_SESSION_SETTINGS} settings per session"),
        ));
    }
    Ok((name, value))
}

/// `SET <name> { TO | = } <value>` on the PostgreSQL wire: the session's own setting.
fn postgres_set(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    let Some((name, value)) = assignment(tokens) else {
        // `SET TRANSACTION ...`, `SET CONSTRAINTS ...` and the other forms that set no
        // named parameter: accepted, and nothing of this session changes.
        return Ok(StatementAnswer::new("SET", ResultSet::default()));
    };
    let lowered = name.to_ascii_lowercase();
    // A name with a namespace (`my.setting`) is one PostgreSQL lets a session define for
    // itself; a bare name has to be a setting this server has.
    if !lowered.contains('.') && facts.setting(&lowered).is_none() {
        return Err(EngineError::new(
            EngineErrorKind::UndefinedObject,
            format!("unrecognized configuration parameter \"{name}\""),
        ));
    }
    let mut answer = StatementAnswer::new("SET", ResultSet::default());
    answer.setting_changed = Some(recorded_setting(facts, lowered, value)?);
    Ok(answer)
}

/// `SET <name> = <value>` on the MySQL wire, for the session.
///
/// MySQL takes any session variable a client names, including the user variables
/// (`@name`) a client sets for itself, so this records the assignment rather than
/// resolving the name against a list.
fn mysql_set(facts: &SessionFacts, tokens: &[Token]) -> Result<StatementAnswer, EngineError> {
    let mut answer = StatementAnswer::new("SET", ResultSet::default());
    if let Some((name, value)) = assignment(tokens) {
        answer.setting_changed = Some(recorded_setting(
            facts,
            name.trim_start_matches('@').to_ascii_lowercase(),
            value,
        )?);
    }
    Ok(answer)
}

/// The `SHOW` forms a MySQL client sends.
fn mysql_show(
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<StatementAnswer, EngineError> {
    let subject = words.get(1).map(String::as_str).unwrap_or_default();
    let qualified = words.get(2).map(String::as_str).unwrap_or_default();
    let subject = match (subject, qualified) {
        ("GLOBAL" | "SESSION", next) => next,
        (subject, _) => subject,
    };
    let pattern = string_literal(tokens);
    match subject {
        "VARIABLES" => Ok(StatementAnswer::new(
            "SHOW",
            name_value_rows("Variable_name", session_variables(facts), pattern.as_deref()),
        )),
        "STATUS" => Ok(StatementAnswer::new(
            "SHOW",
            name_value_rows("Variable_name", session_status(facts), pattern.as_deref()),
        )),
        "DATABASES" | "SCHEMAS" => Ok(StatementAnswer::new(
            "SHOW",
            ResultSet::of_rows(
                vec![ColumnInfo {
                    name: "Database".to_string(),
                    declared_type: Some("text".to_string()),
                }],
                facts
                    .databases
                    .iter()
                    .map(|name| vec![Cell::Text(name.clone())])
                    .collect(),
            ),
        )),
        // `SHOW TABLES`, `SHOW COLUMNS`, `SHOW CREATE TABLE` and the rest read the
        // schema, which is the ENGINE's to report; the engine has no `SHOW`, so the
        // honest answer is that this server does not serve the command rather than a
        // schema listing assembled here.
        _ => Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "this emulator's MySQL endpoint does not serve SHOW {}",
                words[1..].join(" ")
            ),
        )),
    }
}

/// The session variables `SHOW VARIABLES` reports: the settings this server answers for.
fn session_variables(facts: &SessionFacts) -> Vec<(String, String)> {
    [
        "version",
        "character_set_server",
        "character_set_client",
        "collation_server",
        "autocommit",
        "require_secure_transport",
        "transaction_isolation",
        "time_zone",
    ]
    .iter()
    .filter_map(|name| {
        facts
            .setting(name)
            .map(|value| ((*name).to_string(), value))
    })
    .collect()
}

/// The status variables `SHOW STATUS` reports for THIS session: what its own socket is.
fn session_status(facts: &SessionFacts) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Ssl_cipher".to_string(),
        facts
            .tls
            .as_ref()
            .and_then(|tls| tls.cipher.clone())
            .unwrap_or_default(),
    )];
    rows.push((
        "Ssl_version".to_string(),
        facts
            .tls
            .as_ref()
            .and_then(|tls| tls.protocol.clone())
            .unwrap_or_default(),
    ));
    rows
}

/// A two-column `Variable_name`/`Value` result, filtered by a `LIKE` pattern when the
/// statement carried one.
fn name_value_rows(
    name_column: &str,
    rows: Vec<(String, String)>,
    pattern: Option<&str>,
) -> ResultSet {
    let rows = rows
        .into_iter()
        .filter(|(name, _)| pattern.is_none_or(|pattern| like_matches(pattern, name)))
        .map(|(name, value)| vec![Cell::Text(name), Cell::Text(value)])
        .collect();
    ResultSet::of_rows(
        vec![
            ColumnInfo {
                name: name_column.to_string(),
                declared_type: Some("text".to_string()),
            },
            ColumnInfo {
                name: "Value".to_string(),
                declared_type: Some("text".to_string()),
            },
        ],
        rows,
    )
}

/// Whether `value` matches the SQL `LIKE` `pattern`, case-INSENSITIVELY, which is how a
/// `SHOW ... LIKE` pattern matches a variable or status name: `%` stands for any run of
/// characters (including none) and `_` for exactly one.
///
/// Matched over CHARACTERS rather than bytes, so a multi-byte character is one `_`, with
/// the classic two-cursor walk and ONE backtrack point: linear in the value per `%`, and
/// it allocates only the two character vectors. This is the only `LIKE` reading this
/// crate needs; the Config service carries its own
/// (`service-com-amazonaws-configservice-starlingdoveservice/src/state/select_query.rs`)
/// because a Config query's `LIKE` is case-SENSITIVE, which is the opposite fold, and
/// neither service reads the other's dialect.
fn like_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let value: Vec<char> = value.to_ascii_lowercase().chars().collect();
    // The classic two-index walk with one backtrack point, so the match is linear in the
    // value's length rather than exponential in the pattern's.
    let (mut p, mut v) = (0usize, 0usize);
    let (mut star, mut star_v) = (None, 0usize);
    while v < value.len() {
        match pattern.get(p) {
            Some('%') => {
                star = Some(p);
                star_v = v;
                p += 1;
            }
            Some('_') => {
                p += 1;
                v += 1;
            }
            Some(expected) if *expected == value[v] => {
                p += 1;
                v += 1;
            }
            _ => match star {
                Some(position) => {
                    p = position + 1;
                    star_v += 1;
                    v = star_v;
                }
                None => return false,
            },
        }
    }
    pattern.get(p..).is_none_or(|rest| rest.iter().all(|c| *c == '%'))
}

/// The first string literal in a statement, which is what a `LIKE` pattern is.
fn string_literal(tokens: &[Token]) -> Option<String> {
    tokens.iter().find_map(|token| match token {
        Token::SingleQuotedString(value) | Token::DoubleQuotedString(value) => {
            Some(value.clone())
        }
        _ => None,
    })
}

/// The `name = value` (or `name TO value`) an assignment statement carries.
fn assignment(tokens: &[Token]) -> Option<(String, String)> {
    let mut name = String::new();
    let mut rest = Vec::new();
    let mut seen_separator = false;
    // Skip the leading keyword (SET) and any scope word (SESSION, LOCAL).
    for token in tokens.iter().skip(1) {
        if seen_separator {
            rest.push(token.to_string());
            continue;
        }
        match token {
            Token::Eq => seen_separator = true,
            Token::Word(word) if word.value.eq_ignore_ascii_case("TO") => seen_separator = true,
            Token::Word(word)
                if name.is_empty()
                    && matches!(
                        word.value.to_ascii_uppercase().as_str(),
                        "SESSION" | "LOCAL"
                    ) => {}
            other => name.push_str(&other.to_string()),
        }
    }
    if name.is_empty() || !seen_separator {
        return None;
    }
    let value = rest.join("");
    Some((
        name.trim().trim_matches('"').to_string(),
        value.trim().trim_matches('\'').to_string(),
    ))
}

/// The statement's leading keywords, uppercased: what decides whether it is a server
/// command and what a `CommandComplete` tag names.
fn leading_words(tokens: &[Token], count: usize) -> Vec<String> {
    tokens
        .iter()
        .take(count)
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect()
}

/// The tag a PostgreSQL `CommandComplete` carries for this statement: the one or two
/// leading keywords, as PostgreSQL words them.
fn statement_verb(words: &[String]) -> String {
    let first = words.first().cloned().unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    match (first.as_str(), second) {
        ("CREATE" | "DROP" | "ALTER", object) if !object.is_empty() => {
            format!("{first} {object}")
        }
        _ => first,
    }
}

/// The statement as the SQL ENGINE is asked to run it: the same tokens, with the values
/// this SESSION carries substituted for the session functions and system variables that
/// name them.
///
/// Substituting a session value is what a server does with `current_user`: the value comes
/// from the connection, not from the statement. Only WORD tokens are replaced, so a
/// string literal that happens to spell one is left exactly as the client wrote it.
fn render_for_engine(facts: &SessionFacts, tokens: &[Token]) -> String {
    let mut rendered = String::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];
        let text = token.to_string();
        // A MySQL system variable: `@@name`, or `@@` followed by the name.
        if let Some(name) = text.strip_prefix("@@") {
            let (name, consumed) = if name.is_empty() {
                (
                    tokens
                        .get(index + 1)
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    2,
                )
            } else {
                (name.to_string(), 1)
            };
            let name = name
                .trim_start_matches("global.")
                .trim_start_matches("session.")
                .to_string();
            rendered.push_str(&quote_value(facts.setting(&name)));
            index += consumed;
            continue;
        }
        match token {
            Token::Word(word) if word.quote_style.is_none() => {
                let lowered = word.value.to_ascii_lowercase();
                let next = next_significant(tokens, index + 1);
                let called = matches!(next.and_then(|at| tokens.get(at)), Some(Token::LParen));
                match session_function(facts, &lowered, called) {
                    // A function written with an empty argument list is substituted
                    // together with it.
                    Some(value) => {
                        rendered.push_str(&value);
                        index += 1;
                        if called {
                            let close = next.and_then(|at| next_significant(tokens, at + 1));
                            if matches!(close.and_then(|at| tokens.get(at)), Some(Token::RParen)) {
                                index = close.map_or(index, |at| at + 1);
                            }
                        }
                        continue;
                    }
                    None => rendered.push_str(&text),
                }
            }
            _ => rendered.push_str(&text),
        }
        index += 1;
    }
    rendered
}

/// The index of the next token that is not whitespace, at or after `from`.
fn next_significant(tokens: &[Token], from: usize) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, token)| !matches!(token, Token::Whitespace(_)))
        .map(|(index, _)| index)
}

/// The value a session function returns, as a SQL literal, or `None` for a word that is
/// not one.
///
/// `called` says whether the word is followed by an argument list, which is what separates
/// the two kinds: the forms the SQL standard spells as KEYWORDS (`current_user`,
/// `session_user`) carry no parentheses, while the rest are functions and are substituted
/// only where they are CALLED — so a column or alias that happens to be named `version`
/// stays the column it is.
fn session_function(facts: &SessionFacts, lowered: &str, called: bool) -> Option<String> {
    let text = |value: &str| Some(format!("'{}'", value.replace('\'', "''")));
    match lowered {
        "current_user" | "session_user" | "user" | "current_role" => text(&facts.user),
        "current_catalog" => text(&facts.database),
        "current_schema" if !called => text(match facts.wire {
            EngineWire::Postgres => "public",
            EngineWire::MySql => &facts.database,
        }),
        _ if !called => None,
        "current_database" | "database" | "schema" => text(&facts.database),
        "current_schema" => text(match facts.wire {
            EngineWire::Postgres => "public",
            EngineWire::MySql => &facts.database,
        }),
        "pg_backend_pid" | "connection_id" => Some(facts.connection_id.to_string()),
        "version" => text(&server_version_banner(facts)),
        _ => None,
    }
}

/// The version string the engine reports for itself, as `version()` returns it.
///
/// PostgreSQL's `version()` is a banner naming the server and its version; MySQL's is the
/// bare version string, which is also what the client sees in the handshake greeting.
#[must_use]
pub fn server_version_banner(facts: &SessionFacts) -> String {
    match facts.wire {
        EngineWire::Postgres => format!("PostgreSQL {}", facts.server_version),
        EngineWire::MySql => facts.server_version.clone(),
    }
}

/// `value` as a SQL literal, or `NULL` where the session has no such value.
fn quote_value(value: Option<String>) -> String {
    match value {
        Some(value) => format!("'{}'", value.replace('\'', "''")),
        None => "NULL".to_string(),
    }
}

/// The statements one simple-query message carries, split on the statement separator.
///
/// Split on the TOKENS, so a semicolon inside a string literal or a quoted identifier
/// does not end a statement.
pub fn split_statements(wire: EngineWire, sql: &str) -> Result<Vec<String>, EngineError> {
    let tokens = statement_tokens(wire, sql)?;
    let mut statements = Vec::new();
    let mut current = String::new();
    for token in tokens {
        if matches!(token, Token::SemiColon) {
            if !current.trim().is_empty() {
                statements.push(current.trim().to_string());
            }
            current = String::new();
            continue;
        }
        current.push_str(&token.to_string());
    }
    if !current.trim().is_empty() {
        statements.push(current.trim().to_string());
    }
    if statements.len() > MAX_STATEMENTS_PER_QUERY {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "one query may carry at most {MAX_STATEMENTS_PER_QUERY} statements in this emulator"
            ),
        ));
    }
    Ok(statements)
}

/// The statement's tokens, keeping the whitespace that separates them (so a re-rendered
/// statement still parses) but dropping comments.
fn statement_tokens(wire: EngineWire, sql: &str) -> Result<Vec<Token>, EngineError> {
    let dialect: Box<dyn Dialect> = match wire {
        EngineWire::Postgres => Box::new(PostgreSqlDialect {}),
        EngineWire::MySql => Box::new(MySqlDialect {}),
    };
    let tokens = Tokenizer::new(dialect.as_ref(), sql)
        .tokenize()
        .map_err(|error| EngineError::new(EngineErrorKind::Syntax, error.to_string()))?;
    Ok(tokens
        .into_iter()
        .filter(|token| {
            !matches!(
                token,
                Token::Whitespace(Whitespace::SingleLineComment { .. })
                    | Token::Whitespace(Whitespace::MultiLineComment(_))
            )
        })
        .collect())
}
