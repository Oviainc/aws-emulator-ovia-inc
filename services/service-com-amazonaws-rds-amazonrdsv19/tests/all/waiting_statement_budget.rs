// SPDX-License-Identifier: CC0-1.0

//! What bounds a statement that WAITS inside the engine, end to end over a PostgreSQL
//! session on the endpoint a DB instance publishes.
//!
//! RULE: a statement holds the instance's engine only for the budget its session runs
//! under — this endpoint's own budget
//! (`state::data_plane::sql_engine::STATEMENT_DEADLINE`), or the session's
//! `statement_timeout` where that is shorter — and the wait a server function performs is
//! cut off at that instant and reported as the cancellation PostgreSQL reports, with the
//! sentence belonging to whichever bound decided it. The bound holds whatever the statement
//! is: the engine's progress handler cannot interrupt a function that is waiting, so the
//! deadline is the only thing that can, and every path that reaches the engine carries one —
//! a plain query, and a statement whose own predicate is run first to identify the rows it
//! would lock.
//!
//! The rows below differ along what the rule generalises over: which statement form reaches
//! the engine (a query, an update, a delete — the last two being the ones whose predicate is
//! run on their behalf before they are), and which of the two bounds is in force, with the
//! wait asked for far longer than either so what is observed is the bound and never the
//! argument.

use std::time::{Duration, Instant};

use service_com_amazonaws_rds_amazonrdsv19::state::data_plane::sql_engine::{
    ENDPOINT_DEADLINE_CANCELLATION, STATEMENT_DEADLINE, STATEMENT_TIMEOUT_CANCELLATION,
};

use crate::db_instance_engine_endpoint::{
    available_instance, postgres_login, PostgresSession, Transport, DATABASE, MASTER_PASSWORD,
    MASTER_USER,
};
use crate::db_instances::emulator;

/// The SQLSTATE PostgreSQL reports a cancelled statement with (`query_canceled`).
const QUERY_CANCELED: &str = "57014";

/// How long the statements below ask to wait: far longer than either bound, so a wait that
/// reached its own argument is unmistakable.
const ASKED_FOR: Duration = Duration::from_secs(60);

/// The session's own `statement_timeout`, in milliseconds, for the row that puts one in
/// force: shorter than this endpoint's budget, so it is the bound that decides.
const SESSION_TIMEOUT_MS: u64 = 300;

/// What a row expects to have decided the cancellation.
#[derive(Debug, Clone, Copy)]
enum Bound {
    /// This endpoint's own budget, with no `statement_timeout` in force.
    Endpoint,
    /// The session's `statement_timeout`, which is shorter.
    SessionTimeout,
}

impl Bound {
    /// The sentence the cancellation carries, which is the service's own wording for the
    /// bound that decided it.
    fn cancellation(self) -> &'static str {
        match self {
            Bound::Endpoint => ENDPOINT_DEADLINE_CANCELLATION,
            Bound::SessionTimeout => STATEMENT_TIMEOUT_CANCELLATION,
        }
    }

    /// `statement_timeout` as the session sets it: `0` disables it.
    fn statement_timeout_ms(self) -> u64 {
        match self {
            Bound::Endpoint => 0,
            Bound::SessionTimeout => SESSION_TIMEOUT_MS,
        }
    }

    /// The longest this row may take before the cancellation arrives: the bound itself, plus
    /// room for the session's round trip, and far short of what the statement asked for.
    fn within(self) -> Duration {
        match self {
            Bound::Endpoint => STATEMENT_DEADLINE * 3,
            Bound::SessionTimeout => Duration::from_millis(SESSION_TIMEOUT_MS) + STATEMENT_DEADLINE,
        }
    }
}

/// One row: a statement that waits, and the bound that must cut it off.
struct Row {
    case: &'static str,
    /// The statement, carrying a wait of [`ASKED_FOR`] seconds.
    sql: String,
    bound: Bound,
}

fn rows() -> Vec<Row> {
    let seconds = ASKED_FOR.as_secs();
    vec![
        Row {
            case: "a query that waits, with no statement_timeout in force",
            sql: format!("SELECT pg_sleep({seconds})"),
            bound: Bound::Endpoint,
        },
        Row {
            case: "an update whose own predicate waits",
            sql: format!(
                "UPDATE emu_waits SET note = 'changed' WHERE id = 1 AND pg_sleep({seconds}) \
                 IS NULL"
            ),
            bound: Bound::Endpoint,
        },
        Row {
            case: "a delete whose own predicate waits",
            sql: format!("DELETE FROM emu_waits WHERE pg_sleep({seconds}) IS NULL"),
            bound: Bound::Endpoint,
        },
        Row {
            case: "a query that waits, with the session's statement_timeout in force",
            sql: format!("SELECT pg_sleep({seconds})"),
            bound: Bound::SessionTimeout,
        },
    ]
}

/// Open a session as the master user on a PostgreSQL instance's published endpoint.
async fn session(address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted")
}

/// Run `sql`, which must succeed: the table and the row the predicates above are written
/// against.
async fn run(session: &mut PostgresSession, sql: &str) {
    if let Err(refusal) = session.query(sql).await {
        panic!("{sql:?} must run, got {refusal:?}");
    }
}

#[tokio::test]
async fn a_waiting_statement_is_cut_off_by_the_budget_its_session_runs_under() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-waiting-postgres",
        "postgres",
        &[("EngineVersion", "16.4".to_string())],
    )
    .await;
    let mut session = session(&address, port).await;
    run(
        &mut session,
        "CREATE TABLE emu_waits (id INT PRIMARY KEY, note TEXT)",
    )
    .await;
    run(
        &mut session,
        "INSERT INTO emu_waits (id, note) VALUES (1, 'before')",
    )
    .await;

    for row in rows() {
        run(
            &mut session,
            &format!("SET statement_timeout = {}", row.bound.statement_timeout_ms()),
        )
        .await;
        let started = Instant::now();
        let answer = session.query(&row.sql).await;
        let elapsed = started.elapsed();
        let refusal = answer.err().unwrap_or_else(|| {
            panic!(
                "case {:?}: a statement that waits {}s must be cancelled, not answered",
                row.case,
                ASKED_FOR.as_secs()
            )
        });
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            (QUERY_CANCELED, row.bound.cancellation()),
            "case {:?}: the cancellation must be the one {:?} decides, got {refusal:?}",
            row.case,
            row.bound
        );
        assert!(
            elapsed < row.bound.within(),
            "case {:?}: {:?} must cut the wait off within {:?}, took {elapsed:?}",
            row.case,
            row.bound,
            row.bound.within()
        );
    }

    // The row the cancelled statements named is the row they left alone: a statement the
    // budget cut off changed nothing.
    let answer = session
        .query("SELECT note FROM emu_waits WHERE id = 1")
        .await;
    let rows = answer.unwrap_or_else(|refusal| panic!("the row must be readable, got {refusal:?}"));
    assert_eq!(
        rows.only(),
        Some("before"),
        "a cancelled statement must leave the row as it was, got {rows:?}"
    );
}
