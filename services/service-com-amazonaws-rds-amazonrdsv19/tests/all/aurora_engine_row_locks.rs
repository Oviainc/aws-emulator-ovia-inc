// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ENGINE BEHAVIOUR a client observes on an Aurora DB cluster's
//! endpoint: the transaction each session holds, the rows one transaction holds against
//! another, and the refusals the engine answers for them.
//!
//! The rules under test, each asserted over a table of inputs spanning what it generalises
//! over:
//!
//! 1. a TRANSACTION belongs to the SESSION that opened it: two sessions hold two
//!    independent transactions at once, each `COMMIT`/`ROLLBACK` ends only its own, and a
//!    rollback puts back the rows that session wrote and no others;
//! 2. a row an open transaction has written or reserved is HELD against every other
//!    transaction until it ends: a statement that asked not to wait for it is refused
//!    `lock_not_available` (SQLSTATE `55P03`), a statement that waits gets the row once the
//!    holder ends, `SKIP LOCKED` leaves the held rows out, and a row the holder does not
//!    hold is not refused at all \u2014 the lock is on the ROW;
//! 3. a CYCLE of transactions each waiting for the other is broken by the engine refusing
//!    exactly ONE of them with `deadlock_detected` (SQLSTATE `40P01`), after which the
//!    others proceed;
//! 4. a row a constraint refused NAMES that constraint: `duplicate key value violates
//!    unique constraint "<name>"` (SQLSTATE `23505`) and the two sentences of a foreign-key
//!    violation (SQLSTATE `23503`), under the name the client gave the constraint or the one
//!    PostgreSQL gives a constraint it names itself.
//!
//! The client is the one the sibling endpoint suites drive
//! ([`crate::db_instance_engine_endpoint`]), written against the PostgreSQL wire itself, so
//! what is exercised is what a stock driver speaks \u2014 and everything runs in ONE process:
//! the cluster's listener is opened by the `CreateDBCluster` call below.

use std::time::Duration;

use crate::db_clusters::describe_one;
use crate::db_instance_engine_endpoint::{postgres_login, PostgresSession, Refusal, Transport};
use crate::db_instances::{call, emulator, expect_ok, text, wait_for_settle};

/// The master user every cluster below is created with, and its password: a test credential,
/// and the only place in this suite it appears.
const MASTER_USER: &str = "sim2real";
const MASTER_PASSWORD: &str = "Sim2RealPassw0rd";

/// The `DatabaseName` the clusters are created with, which every session here lands in.
const DATABASE: &str = "enginetruth";

/// How long a test waits for a statement another session is expected to be blocking.
const BLOCKED_FOR: Duration = Duration::from_millis(250);

/// Create an Aurora PostgreSQL cluster, wait for it to settle, and answer with the endpoint
/// it publishes.
async fn available_cluster(
    router: &smithy_server_runtime::MultiServiceRouter,
    identifier: &str,
) -> (String, u16) {
    expect_ok(
        identifier,
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", MASTER_USER),
                ("MasterUserPassword", MASTER_PASSWORD),
                ("DatabaseName", DATABASE),
                ("BackupRetentionPeriod", "1"),
            ],
        )
        .await,
    );
    // A client connects once the cluster reports `available`, which the clock makes due.
    wait_for_settle();
    let settled = describe_one(router, identifier, identifier).await;
    let status = text(&settled, "Status");
    assert_eq!(
        status, "available",
        "case {identifier:?}: the cluster must settle to available before a client connects, \
         got {status:?}"
    );
    let address = text(&settled, "Endpoint");
    let port = text(&settled, "Port");
    let port: u16 = port
        .parse()
        .unwrap_or_else(|_| panic!("case {identifier:?}: expected a port, got {port:?}"));
    (address, port)
}

/// One session on that endpoint, logged in as the cluster's master user.
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

/// Run one statement that must succeed.
async fn must_run(case: &str, session: &mut PostgresSession, sql: &str) {
    let answer = session.query(sql).await;
    assert!(
        answer.is_ok(),
        "case {case:?}: {sql:?} must run, got {answer:?}"
    );
}

/// The rows of a single-column answer, as text.
async fn column_of(case: &str, session: &mut PostgresSession, sql: &str) -> Vec<String> {
    let answer = session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
    answer
        .rows
        .iter()
        .map(|row| row.first().cloned().flatten().unwrap_or_default())
        .collect()
}

/// The two rows every locking table below starts from.
const SEED: [&str; 2] = [
    "CREATE TABLE s2r_rows (id integer PRIMARY KEY, n integer NOT NULL)",
    "INSERT INTO s2r_rows (id, n) VALUES (1, 10), (2, 20)",
];

// ---------------------------------------------------------------------------
// Rule 1: the transaction is the session's.
// ---------------------------------------------------------------------------

/// One row: what the first session does inside its transaction, how it ends it, and the rows
/// both sessions must then read.
struct TransactionRow {
    case: &'static str,
    /// The statements the first session runs inside its transaction.
    inside: &'static [&'static str],
    /// How it ends its transaction.
    ending: &'static str,
    /// `(id, n)` of every row of the table once both transactions have ended.
    settled: &'static [(&'static str, &'static str)],
}

/// The rows span what the rule generalises over: each way a transaction ends, and each kind
/// of change it can have made (an update, an insert, a delete, and a delete that cascades to
/// another table's rows).
const TRANSACTION_ROWS: &[TransactionRow] = &[
    TransactionRow {
        case: "an updated row, rolled back",
        inside: &["UPDATE s2r_rows SET n = 99 WHERE id = 1"],
        ending: "ROLLBACK",
        settled: &[("1", "10"), ("2", "20")],
    },
    TransactionRow {
        case: "an updated row, committed",
        inside: &["UPDATE s2r_rows SET n = 99 WHERE id = 1"],
        ending: "COMMIT",
        settled: &[("1", "99"), ("2", "20")],
    },
    TransactionRow {
        case: "an inserted row, rolled back",
        inside: &["INSERT INTO s2r_rows (id, n) VALUES (3, 30)"],
        ending: "ROLLBACK",
        settled: &[("1", "10"), ("2", "20")],
    },
    TransactionRow {
        case: "an inserted row, committed",
        inside: &["INSERT INTO s2r_rows (id, n) VALUES (3, 30)"],
        ending: "COMMIT",
        settled: &[("1", "10"), ("2", "20"), ("3", "30")],
    },
    TransactionRow {
        case: "a deleted row, rolled back",
        inside: &["DELETE FROM s2r_rows WHERE id = 2"],
        ending: "ROLLBACK",
        settled: &[("1", "10"), ("2", "20")],
    },
    TransactionRow {
        case: "every row deleted, rolled back",
        inside: &["DELETE FROM s2r_rows"],
        ending: "ROLLBACK",
        settled: &[("1", "10"), ("2", "20")],
    },
    TransactionRow {
        case: "several changes in one transaction, rolled back",
        inside: &[
            "UPDATE s2r_rows SET n = 99 WHERE id = 1",
            "INSERT INTO s2r_rows (id, n) VALUES (4, 40)",
            "DELETE FROM s2r_rows WHERE id = 2",
        ],
        ending: "ROLLBACK",
        settled: &[("1", "10"), ("2", "20")],
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transaction_is_the_sessions_own_and_its_rollback_puts_back_only_its_rows() {
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-transactions").await;
    let mut setup = session("setup", &address, port).await;
    for statement in SEED {
        must_run("setup", &mut setup, statement).await;
    }

    for row in TRANSACTION_ROWS {
        let case = row.case;
        // Each row starts from the seeded table, whatever the one before it left.
        must_run(case, &mut setup, "DELETE FROM s2r_rows").await;
        must_run(case, &mut setup, SEED[1]).await;

        let mut first = session(case, &address, port).await;
        must_run(case, &mut first, "BEGIN").await;
        for statement in row.inside {
            must_run(case, &mut first, statement).await;
        }
        must_run(case, &mut first, row.ending).await;

        let settled = column_of(
            case,
            &mut setup,
            "SELECT id || ':' || n FROM s2r_rows ORDER BY id",
        )
        .await;
        let expected: Vec<String> = row
            .settled
            .iter()
            .map(|(id, n)| format!("{id}:{n}"))
            .collect();
        assert_eq!(
            settled, expected,
            "case {case:?}: expected the rows {expected:?} once the transaction {:?}, got \
             {settled:?}",
            row.ending
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_sessions_transaction_ends_neither_another_sessions_nor_its_rows() {
    let case = "two sessions, two transactions";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-two-sessions").await;
    let mut setup = session(case, &address, port).await;
    for statement in SEED {
        must_run(case, &mut setup, statement).await;
    }
    let mut first = session(case, &address, port).await;
    let mut second = session(case, &address, port).await;

    // Both sessions hold a transaction at once, each over a row of its own.
    must_run(case, &mut first, "BEGIN").await;
    must_run(case, &mut first, "UPDATE s2r_rows SET n = 11 WHERE id = 1").await;
    must_run(case, &mut second, "BEGIN").await;
    must_run(case, &mut second, "UPDATE s2r_rows SET n = 22 WHERE id = 2").await;
    // Each session reads its OWN uncommitted row back, which is what being in a transaction
    // of its own means.
    let own = column_of(case, &mut first, "SELECT n FROM s2r_rows WHERE id = 1").await;
    assert_eq!(
        own,
        vec!["11".to_string()],
        "case {case:?}: a session must read its own uncommitted row back, got {own:?}"
    );

    // The second session commits; the first rolls back. Each ending touches only its own
    // transaction's rows.
    must_run(case, &mut second, "COMMIT").await;
    must_run(case, &mut first, "ROLLBACK").await;
    let settled = column_of(
        case,
        &mut setup,
        "SELECT id || ':' || n FROM s2r_rows ORDER BY id",
    )
    .await;
    assert_eq!(
        settled,
        vec!["1:10".to_string(), "2:22".to_string()],
        "case {case:?}: the rolled-back row must go back and the committed one must stand, \
         got {settled:?}"
    );
    // Both sessions are out of their transactions and neither reports the other's: the next
    // statement each runs is its own, and a COMMIT with nothing open is a success both
    // engines answer with a warning rather than a refusal.
    let after = first.query("COMMIT").await;
    assert!(
        after.is_ok(),
        "case {case:?}: a COMMIT with no transaction open must succeed, got {after:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rollback_puts_back_the_rows_a_cascading_delete_took_with_it() {
    let case = "a cascading delete, rolled back";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-cascade").await;
    let mut setup = session(case, &address, port).await;
    for statement in [
        "CREATE TABLE s2r_parent (id integer PRIMARY KEY)",
        "CREATE TABLE s2r_kid (id integer PRIMARY KEY, parent integer, CONSTRAINT \
         s2r_kid_parent_fk FOREIGN KEY (parent) REFERENCES s2r_parent (id) ON DELETE CASCADE)",
        "INSERT INTO s2r_parent (id) VALUES (1), (2)",
        "INSERT INTO s2r_kid (id, parent) VALUES (10, 1), (11, 1), (20, 2)",
    ] {
        must_run(case, &mut setup, statement).await;
    }

    // A delete of one parent takes only ITS dependents, which is what the cascade is.
    must_run(case, &mut setup, "BEGIN").await;
    must_run(case, &mut setup, "DELETE FROM s2r_parent WHERE id = 1").await;
    let during = column_of(case, &mut setup, "SELECT id FROM s2r_kid ORDER BY id").await;
    assert_eq!(
        during,
        vec!["20".to_string()],
        "case {case:?}: the cascade must take only the deleted parent's dependents, got \
         {during:?}"
    );
    // Rolling the transaction back puts the parent AND the rows the cascade took with it
    // back: they are the rows this transaction changed.
    must_run(case, &mut setup, "ROLLBACK").await;
    let after = column_of(case, &mut setup, "SELECT id FROM s2r_kid ORDER BY id").await;
    assert_eq!(
        after,
        vec!["10".to_string(), "11".to_string(), "20".to_string()],
        "case {case:?}: a rollback must put back the rows the cascade took, got {after:?}"
    );
    let parents = column_of(case, &mut setup, "SELECT id FROM s2r_parent ORDER BY id").await;
    assert_eq!(
        parents,
        vec!["1".to_string(), "2".to_string()],
        "case {case:?}: a rollback must put the deleted parent back, got {parents:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the row one transaction holds is held against another.
// ---------------------------------------------------------------------------

/// One row: how the holder takes row 1, and what the second session's statement on that row
/// must be refused with when it asks not to wait.
struct HeldRow {
    case: &'static str,
    /// The statement the holding transaction runs.
    holder: &'static str,
    /// The statement the second session runs on the same row.
    waiter: &'static str,
    /// The SQLSTATE the second session must read, or `None` where it must be served.
    refused_with: Option<&'static str>,
}

/// `lock_not_available`: what PostgreSQL answers a statement that asked for a row another
/// transaction holds and asked not to wait for it. Source: the condition name in
/// PostgreSQL's "Appendix A. PostgreSQL Error Codes".
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// The rows span what the rule generalises over: each way a transaction takes a row (a
/// write, a `FOR UPDATE` read, a delete), each form of the locking clause the second session
/// can ask with, and a shared reader beside an exclusive one.
const HELD_ROWS: &[HeldRow] = &[
    HeldRow {
        case: "a row the holder wrote, asked for FOR UPDATE NOWAIT",
        holder: "UPDATE s2r_rows SET n = 11 WHERE id = 1",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE NOWAIT",
        refused_with: Some(LOCK_NOT_AVAILABLE),
    },
    HeldRow {
        case: "a row the holder reserved with FOR UPDATE, asked for FOR UPDATE NOWAIT",
        holder: "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE NOWAIT",
        refused_with: Some(LOCK_NOT_AVAILABLE),
    },
    HeldRow {
        case: "a row the holder wrote, asked for FOR NO KEY UPDATE NOWAIT",
        holder: "UPDATE s2r_rows SET n = 11 WHERE id = 1",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR NO KEY UPDATE NOWAIT",
        refused_with: Some(LOCK_NOT_AVAILABLE),
    },
    HeldRow {
        case: "a row the holder reserved with FOR UPDATE, asked for FOR KEY SHARE NOWAIT",
        holder: "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR KEY SHARE NOWAIT",
        refused_with: Some(LOCK_NOT_AVAILABLE),
    },
    HeldRow {
        case: "ANOTHER row of a table whose row the holder deleted",
        holder: "DELETE FROM s2r_rows WHERE id = 1",
        waiter: "SELECT n FROM s2r_rows WHERE id = 2 FOR UPDATE NOWAIT",
        refused_with: None,
    },
    HeldRow {
        case: "a row the holder reserved with FOR UPDATE, asked for FOR SHARE NOWAIT",
        holder: "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR SHARE NOWAIT",
        refused_with: Some(LOCK_NOT_AVAILABLE),
    },
    HeldRow {
        case: "a row the holder only READ, asked for FOR UPDATE NOWAIT",
        holder: "SELECT n FROM s2r_rows WHERE id = 1",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE NOWAIT",
        refused_with: None,
    },
    HeldRow {
        case: "a row the holder reserved with FOR SHARE, asked for FOR SHARE NOWAIT",
        holder: "SELECT n FROM s2r_rows WHERE id = 1 FOR SHARE",
        waiter: "SELECT n FROM s2r_rows WHERE id = 1 FOR SHARE NOWAIT",
        refused_with: None,
    },
    HeldRow {
        case: "ANOTHER row of the same table, asked for FOR UPDATE NOWAIT",
        holder: "UPDATE s2r_rows SET n = 11 WHERE id = 1",
        waiter: "SELECT n FROM s2r_rows WHERE id = 2 FOR UPDATE NOWAIT",
        refused_with: None,
    },
    HeldRow {
        case: "a row the holder wrote, asked for with no row of its own",
        holder: "UPDATE s2r_rows SET n = 11 WHERE id = 1",
        waiter: "SELECT n FROM s2r_rows WHERE id = 99 FOR UPDATE NOWAIT",
        refused_with: None,
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_an_open_transaction_holds_is_refused_to_a_statement_that_will_not_wait() {
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-nowait").await;
    let mut setup = session("setup", &address, port).await;
    for statement in SEED {
        must_run("setup", &mut setup, statement).await;
    }

    for row in HELD_ROWS {
        let case = row.case;
        must_run(case, &mut setup, "DELETE FROM s2r_rows").await;
        must_run(case, &mut setup, SEED[1]).await;

        let mut holder = session(case, &address, port).await;
        let mut waiter = session(case, &address, port).await;
        must_run(case, &mut holder, "BEGIN").await;
        must_run(case, &mut holder, row.holder).await;

        let answer = waiter.query(row.waiter).await;
        match row.refused_with {
            Some(code) => {
                let refusal = answer.err().unwrap_or_else(|| {
                    panic!("case {case:?}: {:?} must be refused, it was served", row.waiter)
                });
                assert_eq!(
                    refusal.code, code,
                    "case {case:?}: expected SQLSTATE {code}, got {refusal:?}"
                );
                assert_eq!(
                    refusal.message, "could not obtain lock on row in relation \"s2r_rows\"",
                    "case {case:?}: expected the engine's own sentence for {code}, got \
                     {refusal:?}"
                );
            }
            None => assert!(
                answer.is_ok(),
                "case {case:?}: {:?} must be served, got {answer:?}",
                row.waiter
            ),
        }

        // Once the holder ends its transaction the row is free, so the same statement is
        // served: the hold lasts exactly as long as the transaction.
        must_run(case, &mut holder, "ROLLBACK").await;
        let after = waiter.query(row.waiter).await;
        assert!(
            after.is_ok(),
            "case {case:?}: once the holder ended, {:?} must be served, got {after:?}",
            row.waiter
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_that_waits_gets_the_row_when_the_holder_ends_its_transaction() {
    let case = "a waiting update";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-wait").await;
    let mut holder = session(case, &address, port).await;
    for statement in SEED {
        must_run(case, &mut holder, statement).await;
    }
    let mut waiter = session(case, &address, port).await;
    must_run(case, &mut holder, "BEGIN").await;
    must_run(
        case,
        &mut holder,
        "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE",
    )
    .await;

    // The waiter's statement names no `NOWAIT`, so it waits for the row rather than being
    // refused: it is still running when the holder commits, and then it is served.
    let waiting = async {
        waiter
            .query("UPDATE s2r_rows SET n = 77 WHERE id = 1")
            .await
    };
    let releasing = async {
        tokio::time::sleep(BLOCKED_FOR).await;
        holder.query("COMMIT").await
    };
    let (waited, released) = tokio::join!(waiting, releasing);
    assert!(
        released.is_ok(),
        "case {case:?}: the holder's COMMIT must run, got {released:?}"
    );
    assert!(
        waited.is_ok(),
        "case {case:?}: the waiting statement must be served once the row is free, got \
         {waited:?}"
    );
    let settled = column_of(case, &mut holder, "SELECT n FROM s2r_rows WHERE id = 1").await;
    assert_eq!(
        settled,
        vec!["77".to_string()],
        "case {case:?}: the waiting statement's own write must be the row's value, got \
         {settled:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skip_locked_leaves_out_the_rows_another_transaction_holds() {
    let case = "SKIP LOCKED";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-skip").await;
    let mut holder = session(case, &address, port).await;
    for statement in SEED {
        must_run(case, &mut holder, statement).await;
    }
    let mut taker = session(case, &address, port).await;
    must_run(case, &mut holder, "BEGIN").await;
    must_run(
        case,
        &mut holder,
        "SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE",
    )
    .await;

    let taken = column_of(
        case,
        &mut taker,
        "SELECT id FROM s2r_rows ORDER BY id FOR UPDATE SKIP LOCKED",
    )
    .await;
    assert_eq!(
        taken,
        vec!["2".to_string()],
        "case {case:?}: the rows another transaction holds must be left out, got {taken:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_locking_select_is_served_over_the_extended_query_protocol_too() {
    let case = "the extended query protocol";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-extended").await;
    let mut holder = session(case, &address, port).await;
    for statement in SEED {
        must_run(case, &mut holder, statement).await;
    }
    let mut waiter = session(case, &address, port).await;

    // The exchange a stock driver uses for a statement it executes: Parse, Bind, Describe,
    // Execute. The rows a locking `SELECT` answers with are the query's own.
    must_run(case, &mut holder, "BEGIN").await;
    let reserved = holder
        .query_with_params("SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE", &[])
        .await
        .unwrap_or_else(|refusal| {
            panic!("case {case:?}: a locking SELECT must be served, got {refusal:?}")
        });
    assert_eq!(
        (reserved.columns.as_slice(), reserved.only()),
        (["n".to_string()].as_slice(), Some("10")),
        "case {case:?}: expected the row the query names, got {reserved:?}"
    );

    // And the refusal is the same one on this protocol: the clause is served by the server,
    // not handed to an engine with no grammar for it.
    let refused = waiter
        .query_with_params("SELECT n FROM s2r_rows WHERE id = 1 FOR UPDATE NOWAIT", &[])
        .await
        .err()
        .unwrap_or_else(|| panic!("case {case:?}: the held row must be refused"));
    assert_eq!(
        refused.code, LOCK_NOT_AVAILABLE,
        "case {case:?}: expected SQLSTATE {LOCK_NOT_AVAILABLE}, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a refused row names the constraint it violated.
// ---------------------------------------------------------------------------

/// `unique_violation` and `foreign_key_violation`: the two conditions a refused row carries.
/// Source: the condition names in PostgreSQL's "Appendix A. PostgreSQL Error Codes".
const UNIQUE_VIOLATION: &str = "23505";
const FOREIGN_KEY_VIOLATION: &str = "23503";

/// One row: the schema it needs, the statement the engine must refuse, and the condition and
/// sentence the client must read.
struct ConstraintRow {
    case: &'static str,
    schema: &'static [&'static str],
    refused: &'static str,
    code: &'static str,
    message: &'static str,
}

/// The rows span what the rule generalises over: each kind of constraint (unique, primary
/// key, foreign key), a constraint the client NAMED beside one it left to the engine to name,
/// a constraint over one column beside one over two, a constraint declared on the column
/// itself beside one declared on the table, and both ends of a foreign key.
const CONSTRAINT_ROWS: &[ConstraintRow] = &[
    ConstraintRow {
        case: "a named unique constraint",
        schema: &[
            "CREATE TABLE s2r_users (id integer PRIMARY KEY, email text, CONSTRAINT \
             s2r_users_email_uq UNIQUE (email))",
            "INSERT INTO s2r_users (id, email) VALUES (1, 'a@example.com')",
        ],
        refused: "INSERT INTO s2r_users (id, email) VALUES (2, 'a@example.com')",
        code: UNIQUE_VIOLATION,
        message: "duplicate key value violates unique constraint \"s2r_users_email_uq\"",
    },
    ConstraintRow {
        case: "a unique constraint the client did not name",
        schema: &[
            "CREATE TABLE s2r_plain (id integer PRIMARY KEY, email text UNIQUE)",
            "INSERT INTO s2r_plain (id, email) VALUES (1, 'a@example.com')",
        ],
        refused: "INSERT INTO s2r_plain (id, email) VALUES (2, 'a@example.com')",
        code: UNIQUE_VIOLATION,
        // The name PostgreSQL gives a unique constraint it names itself.
        message: "duplicate key value violates unique constraint \"s2r_plain_email_key\"",
    },
    ConstraintRow {
        case: "a primary key",
        schema: &[
            "CREATE TABLE s2r_keys (id integer PRIMARY KEY, label text)",
            "INSERT INTO s2r_keys (id, label) VALUES (1, 'one')",
        ],
        refused: "INSERT INTO s2r_keys (id, label) VALUES (1, 'again')",
        code: UNIQUE_VIOLATION,
        message: "duplicate key value violates unique constraint \"s2r_keys_pkey\"",
    },
    ConstraintRow {
        case: "a unique constraint over two columns",
        schema: &[
            "CREATE TABLE s2r_pairs (id integer PRIMARY KEY, left_side text, right_side \
             text, UNIQUE (left_side, right_side))",
            "INSERT INTO s2r_pairs (id, left_side, right_side) VALUES (1, 'a', 'b')",
        ],
        refused: "INSERT INTO s2r_pairs (id, left_side, right_side) VALUES (2, 'a', 'b')",
        code: UNIQUE_VIOLATION,
        message: "duplicate key value violates unique constraint \
                  \"s2r_pairs_left_side_right_side_key\"",
    },
    ConstraintRow {
        case: "a named foreign key, on the row that would break it",
        schema: &[
            "CREATE TABLE s2r_parents (id integer PRIMARY KEY)",
            "CREATE TABLE s2r_children (id integer PRIMARY KEY, parent integer, CONSTRAINT \
             s2r_children_parent_fk FOREIGN KEY (parent) REFERENCES s2r_parents (id))",
            "INSERT INTO s2r_parents (id) VALUES (1)",
        ],
        refused: "INSERT INTO s2r_children (id, parent) VALUES (10, 999)",
        code: FOREIGN_KEY_VIOLATION,
        message: "insert or update on table \"s2r_children\" violates foreign key constraint \
                  \"s2r_children_parent_fk\"",
    },
    ConstraintRow {
        case: "a foreign key the client did not name, declared on the column",
        schema: &[
            "CREATE TABLE s2r_owners (id integer PRIMARY KEY)",
            "CREATE TABLE s2r_owned (id integer PRIMARY KEY, owner integer REFERENCES \
             s2r_owners (id))",
            "INSERT INTO s2r_owners (id) VALUES (1)",
        ],
        refused: "INSERT INTO s2r_owned (id, owner) VALUES (10, 999)",
        code: FOREIGN_KEY_VIOLATION,
        // The name PostgreSQL gives a foreign key it names itself.
        message: "insert or update on table \"s2r_owned\" violates foreign key constraint \
                  \"s2r_owned_owner_fkey\"",
    },
    ConstraintRow {
        case: "a foreign key, on the parent row another row still refers to",
        schema: &[
            "CREATE TABLE s2r_held (id integer PRIMARY KEY)",
            "CREATE TABLE s2r_holder (id integer PRIMARY KEY, held integer, CONSTRAINT \
             s2r_holder_held_fk FOREIGN KEY (held) REFERENCES s2r_held (id))",
            "INSERT INTO s2r_held (id) VALUES (1)",
            "INSERT INTO s2r_holder (id, held) VALUES (10, 1)",
        ],
        refused: "DELETE FROM s2r_held WHERE id = 1",
        code: FOREIGN_KEY_VIOLATION,
        message: "update or delete on table \"s2r_held\" violates foreign key constraint \
                  \"s2r_holder_held_fk\" on table \"s2r_holder\"",
    },
    ConstraintRow {
        case: "an updated row that would break a named foreign key",
        schema: &[
            "CREATE TABLE s2r_books (id integer PRIMARY KEY)",
            "CREATE TABLE s2r_pages (id integer PRIMARY KEY, book integer, CONSTRAINT \
             s2r_pages_book_fk FOREIGN KEY (book) REFERENCES s2r_books (id))",
            "INSERT INTO s2r_books (id) VALUES (1)",
            "INSERT INTO s2r_pages (id, book) VALUES (10, 1)",
        ],
        refused: "UPDATE s2r_pages SET book = 999 WHERE id = 10",
        code: FOREIGN_KEY_VIOLATION,
        message: "insert or update on table \"s2r_pages\" violates foreign key constraint \
                  \"s2r_pages_book_fk\"",
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_row_names_the_constraint_it_violated() {
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-constraints").await;
    let mut client = session("constraints", &address, port).await;

    for row in CONSTRAINT_ROWS {
        let case = row.case;
        for statement in row.schema {
            must_run(case, &mut client, statement).await;
        }
        let answer = client.query(row.refused).await;
        let refusal = answer.err().unwrap_or_else(|| {
            panic!(
                "case {case:?}: {:?} must be refused, it was served",
                row.refused
            )
        });
        assert_eq!(
            refusal.code, row.code,
            "case {case:?}: expected SQLSTATE {}, got {refusal:?}",
            row.code
        );
        assert_eq!(
            refusal.message, row.message,
            "case {case:?}: expected the engine's own sentence, got {refusal:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a cycle of waiting transactions has exactly one victim.
// ---------------------------------------------------------------------------

/// `deadlock_detected`: what PostgreSQL answers the transaction it ends to break a cycle.
/// Source: the condition name in PostgreSQL's "Appendix A. PostgreSQL Error Codes".
const DEADLOCK_DETECTED: &str = "40P01";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cycle_of_two_waiting_transactions_ends_with_exactly_one_victim() {
    let case = "two transactions in a cycle";
    let router = emulator();
    let (address, port) = available_cluster(&router, "emu-truth-deadlock").await;
    let mut setup = session(case, &address, port).await;
    for statement in SEED {
        must_run(case, &mut setup, statement).await;
    }
    let mut first = session(case, &address, port).await;
    let mut second = session(case, &address, port).await;

    // Each transaction takes one row, then asks for the row the other holds: whichever of
    // them closes the cycle is the one the engine ends.
    must_run(case, &mut first, "BEGIN").await;
    must_run(case, &mut first, "UPDATE s2r_rows SET n = 11 WHERE id = 1").await;
    must_run(case, &mut second, "BEGIN").await;
    must_run(case, &mut second, "UPDATE s2r_rows SET n = 22 WHERE id = 2").await;

    let crossing_first = async { first.query("UPDATE s2r_rows SET n = 12 WHERE id = 2").await };
    let crossing_second = async { second.query("UPDATE s2r_rows SET n = 21 WHERE id = 1").await };
    let (one, two) = tokio::join!(crossing_first, crossing_second);

    let victims: Vec<&Refusal> = [one.as_ref().err(), two.as_ref().err()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(
        victims.len(),
        1,
        "case {case:?}: exactly one of the two transactions must be ended, got {one:?} and \
         {two:?}"
    );
    let victim = victims[0];
    assert_eq!(
        victim.code, DEADLOCK_DETECTED,
        "case {case:?}: the victim must read SQLSTATE {DEADLOCK_DETECTED}, got {victim:?}"
    );
    assert_eq!(
        victim.message, "deadlock detected",
        "case {case:?}: expected the engine's own sentence for {DEADLOCK_DETECTED}, got \
         {victim:?}"
    );

    // The transaction that was NOT ended holds its rows and commits them; the victim's own
    // changes are gone with its transaction.
    let winner_first = one.is_ok();
    let ended = if winner_first {
        first.query("COMMIT").await
    } else {
        second.query("COMMIT").await
    };
    assert!(
        ended.is_ok(),
        "case {case:?}: the transaction the engine kept must commit, got {ended:?}"
    );
    let settled = column_of(
        case,
        &mut setup,
        "SELECT id || ':' || n FROM s2r_rows ORDER BY id",
    )
    .await;
    let expected = if winner_first {
        vec!["1:11".to_string(), "2:12".to_string()]
    } else {
        vec!["1:21".to_string(), "2:22".to_string()]
    };
    assert_eq!(
        settled, expected,
        "case {case:?}: only the kept transaction's rows may stand, got {settled:?}"
    );
}
