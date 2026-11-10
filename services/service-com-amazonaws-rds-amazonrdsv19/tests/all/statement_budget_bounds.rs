// SPDX-License-Identifier: CC0-1.0

//! The BUDGET every statement an engine endpoint runs is bounded by, and what a session can
//! and cannot do to it.
//!
//! The rule under test: a statement on a DB instance's engine endpoint — and every wait
//! inside it, the one in the predicate that identifies the rows it locks included — runs
//! under this endpoint's own statement budget, narrowed by the session's `statement_timeout`
//! only when that names a SHORTER one. A number a session supplies never widens that budget,
//! and one no clock can carry as an instant narrows nothing: the endpoint's own budget
//! stands, and the session is answered rather than left waiting on a parked engine.
//!
//! The table spans what the rule generalises over: the timeout disabled, a timeout the
//! statement finishes inside, a timeout the statement runs past, a timeout LONGER than the
//! endpoint's budget, and the two session-supplied numbers at the top of the range a
//! `statement_timeout` is carried in — each with a wait the engine really performs
//! (`pg_sleep`), which is the one statement whose length the session itself chooses.
//!
//! Every client here is the wire client the sibling engine-endpoint suite drives
//! ([`crate::db_instance_engine_endpoint`]), so what these tests exercise is the protocol a
//! stock driver speaks.

use std::time::{Duration, Instant};

use service_com_amazonaws_rds_amazonrdsv19::state::data_plane::sql_engine::STATEMENT_DEADLINE;

use crate::db_instance_engine_endpoint::{
    available_instance, postgres_login, PostgresSession, Transport, DATABASE, MASTER_PASSWORD,
    MASTER_USER,
};
use crate::db_instances::emulator;

/// `canceling statement due to statement timeout` / `57014`: what PostgreSQL answers a
/// statement its own `statement_timeout` cancelled. Source: PostgreSQL's "Appendix A.
/// PostgreSQL Error Codes" (`query_canceled`) and the message `statement_timeout` is
/// documented to cancel with.
const QUERY_CANCELED: &str = "57014";

/// How much longer than a statement's budget an answer may take to come back before the
/// endpoint is holding the session rather than bounding it: the budget is enforced between
/// the engine's own operations, so an answer arrives after it rather than at it.
const SLACK: Duration = Duration::from_secs(3);

/// One row: the `statement_timeout` the session sets, the wait the statement then asks the
/// engine for, and whether the rule says that statement is cancelled.
struct Row {
    case: &'static str,
    /// The value the session sets `statement_timeout` to, in milliseconds, as the client
    /// writes it.
    timeout_ms: &'static str,
    /// The seconds the statement asks `pg_sleep` for, as the client writes them.
    sleep_seconds: &'static str,
    /// Whether the statement must come back cancelled rather than served.
    cancelled: bool,
}

/// The rows span the classes a `statement_timeout` falls in against the endpoint's own
/// budget, and the two session-supplied numbers at the top of the range: `i64::MAX` and
/// `u64::MAX` milliseconds, neither of which any clock can carry as an instant.
const ROWS: &[Row] = &[
    Row {
        case: "the timeout disabled, a wait inside the endpoint's budget",
        timeout_ms: "0",
        sleep_seconds: "0.2",
        cancelled: false,
    },
    Row {
        case: "a timeout the statement finishes inside",
        timeout_ms: "4000",
        sleep_seconds: "0.2",
        cancelled: false,
    },
    Row {
        case: "a timeout the statement runs past",
        timeout_ms: "150",
        sleep_seconds: "4",
        cancelled: true,
    },
    Row {
        case: "a timeout longer than the endpoint's own budget",
        timeout_ms: "600000",
        sleep_seconds: "0.2",
        cancelled: false,
    },
    Row {
        case: "a timeout longer than the endpoint's budget, and a wait past that budget",
        timeout_ms: "600000",
        sleep_seconds: "30",
        cancelled: true,
    },
    Row {
        case: "a timeout no clock can carry as an instant",
        timeout_ms: "18446744073709551615",
        sleep_seconds: "0.2",
        cancelled: false,
    },
    Row {
        case: "the largest signed millisecond count a client can write",
        timeout_ms: "9223372036854775807",
        sleep_seconds: "0.2",
        cancelled: false,
    },
];

/// Open a session as the master user on a PostgreSQL instance's published endpoint.
async fn session(case: &str, address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!("case {case:?}: the master user must be admitted, got {refusal:?}")
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_statement_timeout_never_widens_the_endpoints_statement_budget() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-budget-postgres",
        "postgres",
        &[("EngineVersion", "16.4".to_string())],
    )
    .await;

    for row in ROWS {
        let case = row.case;
        // Each row is its own session, so the timeout one row sets is never the timeout
        // another row's statement runs under.
        let mut client = session(case, &address, port).await;
        let set = format!("SET statement_timeout = {}", row.timeout_ms);
        let applied = client.query(&set).await;
        assert!(
            applied.is_ok(),
            "case {case:?}: {set:?} must be accepted, got {applied:?}"
        );

        let statement = format!("SELECT pg_sleep({})", row.sleep_seconds);
        let began = Instant::now();
        let answer = client.query(&statement).await;
        let took = began.elapsed();

        match (row.cancelled, &answer) {
            (false, Ok(_)) => {}
            (true, Err(refusal)) => assert_eq!(
                refusal.code.as_str(),
                QUERY_CANCELED,
                "case {case:?}: a cancelled statement must carry PostgreSQL's own \
                 query_canceled, got {refusal:?}"
            ),
            (expected, got) => panic!(
                "case {case:?}: expected a cancellation = {expected} for {statement:?}, got \
                 {got:?} after {took:?}"
            ),
        }
        // However the row ends, it ends inside the budget this endpoint serves a statement
        // under: a session's own number cannot hold the engine for longer than that.
        assert!(
            took <= STATEMENT_DEADLINE + SLACK,
            "case {case:?}: {statement:?} must be answered inside the endpoint's budget, \
             got {took:?}"
        );
        // And the session itself is still there: the statement was bounded, not the
        // connection.
        let after = client.query("SELECT 1").await;
        assert!(
            after.is_ok(),
            "case {case:?}: the session must outlive its bounded statement, got {after:?}"
        );
    }
}

/// The table the locking statement below reads, and the row it locks.
const SEED: [&str; 2] = [
    "CREATE TABLE budget_rows (id integer PRIMARY KEY, n integer NOT NULL)",
    "INSERT INTO budget_rows (id, n) VALUES (1, 10)",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_predicate_that_identifies_locked_rows_runs_under_the_statements_own_budget() {
    let case = "a locking statement whose predicate waits";
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-budget-locks-postgres",
        "postgres",
        &[("EngineVersion", "16.4".to_string())],
    )
    .await;
    let mut setup = session(case, &address, port).await;
    for statement in SEED {
        let answer = setup.query(statement).await;
        assert!(
            answer.is_ok(),
            "case {case:?}: {statement:?} must run, got {answer:?}"
        );
    }

    // The rows this statement locks are the ones its own predicate selects, so the wait
    // inside that predicate is the session's statement to bound: with a short
    // `statement_timeout` in force the answer comes back inside it, rather than after the
    // wait the predicate asked the engine for.
    let mut client = session(case, &address, port).await;
    let applied = client.query("SET statement_timeout = 150").await;
    assert!(
        applied.is_ok(),
        "case {case:?}: the timeout must be accepted, got {applied:?}"
    );
    let statement = "SELECT n FROM budget_rows WHERE pg_sleep(4) IS NULL FOR UPDATE";
    let began = Instant::now();
    let answer = client.query(statement).await;
    let took = began.elapsed();
    assert!(
        answer.is_err(),
        "case {case:?}: {statement:?} asks for a wait past its own timeout, so it must not \
         be served, got {answer:?} after {took:?}"
    );
    assert!(
        took < Duration::from_secs(2),
        "case {case:?}: the predicate's wait must be bounded by the statement's own budget, \
         but the answer took {took:?}"
    );

    // With the timeout disabled the same shape of statement — a predicate the engine really
    // waits in — is SERVED, under the endpoint's own budget.
    let applied = client.query("SET statement_timeout = 0").await;
    assert!(
        applied.is_ok(),
        "case {case:?}: the timeout must be clearable, got {applied:?}"
    );
    let statement = "SELECT n FROM budget_rows WHERE pg_sleep(0.2) IS NULL FOR UPDATE";
    let began = Instant::now();
    let answer = client.query(statement).await;
    let took = began.elapsed();
    let served = answer
        .unwrap_or_else(|refusal| panic!("case {case:?}: {statement:?} must run, got {refusal:?}"));
    let values: Vec<String> = served
        .rows
        .iter()
        .map(|row| row.first().cloned().flatten().unwrap_or_default())
        .collect();
    assert_eq!(
        values,
        vec!["10".to_string()],
        "case {case:?}: the locking statement must answer with the row it locked, got \
         {values:?} after {took:?}"
    );
}
