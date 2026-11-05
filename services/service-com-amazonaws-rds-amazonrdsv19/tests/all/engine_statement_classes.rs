// SPDX-License-Identifier: CC0-1.0

//! The statement CLASSES a DB instance's engine endpoint serves, over the engine's own
//! wire: installing an extension, creating and dropping a database, and ending a
//! transaction that was never opened.
//!
//! The rule under test: a statement the engine the endpoint reports itself as DEFINES is
//! served from the instance's own state and answered as that engine answers it — and a
//! statement it refuses carries that engine's own code and sentence for the condition,
//! never the embedded engine's message under a code class the engine does not use. So
//! `CREATE EXTENSION` installs an extension the instance ships into the database the
//! session is on, `CREATE DATABASE` creates a schema a later session can connect to, a
//! `COMMIT` with no transaction open succeeds, and a statement that is genuinely
//! unreadable is refused in the words its own engine words a parse error in.
//!
//! The tables span what that rule generalises over: both wires, the conditions each
//! statement class defines (already there, not there, not available, a requirement not
//! installed, something still depending on it), the forms a client writes a name in
//! (bare, quoted, hyphenated), and a transaction that is open against one that is not.
//! Every client here is the wire client the sibling engine-endpoint suite drives, so what
//! these tests exercise is the protocol a stock driver speaks.

use crate::db_instance_engine_endpoint::{
    available_instance, mysql_login, postgres_login, MySqlSession, PostgresSession, Rows,
    Transport, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::emulator;

/// One statement a session runs, and what the server must answer it with.
#[derive(Debug, Clone, Copy)]
struct Case {
    case: &'static str,
    statement: &'static str,
    /// The code and the message of the refusal, or `None` where the statement succeeds.
    refusal: Option<(&'static str, &'static str)>,
}

/// Run one table of cases on a PostgreSQL session, in order.
async fn run_postgres(session: &mut PostgresSession, cases: &[Case]) {
    for row in cases {
        let answer = session.query(row.statement).await;
        check(row, answer);
    }
}

/// Run one table of cases on a MySQL session, in order.
async fn run_mysql(session: &mut MySqlSession, cases: &[Case]) {
    for row in cases {
        let answer = session.query(row.statement).await;
        check(row, answer);
    }
}

/// Hold one case's answer against what the engine must have answered.
fn check(row: &Case, answer: Result<Rows, crate::db_instance_engine_endpoint::Refusal>) {
    match (row.refusal, &answer) {
        (None, Ok(_)) => {}
        (Some((code, message)), Err(refusal)) => {
            assert_eq!(
                (refusal.code.as_str(), refusal.message.as_str()),
                (code, message),
                "case {:?}: expected the refusal ({code}, {message:?}), got {refusal:?}",
                row.case
            );
        }
        (None, Err(refusal)) => panic!(
            "case {:?}: {:?} must be served, got {refusal:?}",
            row.case, row.statement
        ),
        (Some(expected), Ok(answer)) => panic!(
            "case {:?}: {:?} must be refused with {expected:?}, got {answer:?}",
            row.case, row.statement
        ),
    }
}

/// The values of a one-column answer, in the order the server sent them.
fn column(answer: &Rows) -> Vec<String> {
    answer
        .rows
        .iter()
        .map(|row| row.first().cloned().flatten().unwrap_or_default())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_postgres_session_installs_the_extensions_the_instance_ships() {
    let router = emulator();
    let (address, port) = available_instance(&router, "emu-pg-extensions", "postgres", &[]).await;
    let mut session = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");

    // The catalog a PostgreSQL database is created with: it is there before the client
    // installs anything, carrying the extension the database was created with.
    let shipped = session
        .query("SELECT extname FROM pg_extension")
        .await
        .expect("the catalog read runs on a database nothing was installed in");
    assert_eq!(
        column(&shipped),
        vec!["plpgsql".to_string()],
        "case {:?}: a database must carry the extension it was created with, got \
         {shipped:?}",
        "the catalog of a fresh database"
    );

    // The documented procedure for the S3 integration, then the conditions the statement
    // class defines. Each row runs on the one session, in order.
    let cases = [
        Case {
            case: "the S3 integration's extension, with its requirement installed by CASCADE",
            statement: "CREATE EXTENSION IF NOT EXISTS aws_s3 CASCADE",
            refusal: None,
        },
        Case {
            case: "the same install again, which IF NOT EXISTS makes a no-op",
            statement: "CREATE EXTENSION IF NOT EXISTS aws_s3 CASCADE",
            refusal: None,
        },
        Case {
            case: "an install of an extension that is already there",
            statement: "CREATE EXTENSION aws_s3",
            refusal: Some(("42710", "extension \"aws_s3\" already exists")),
        },
        Case {
            // An entry of the shipped catalog the S3 flows do not use, with no
            // requirement of its own.
            case: "an extension with no requirements, installed without CASCADE",
            statement: "CREATE EXTENSION pgcrypto",
            refusal: None,
        },
        Case {
            case: "an extension whose name has to be written quoted",
            statement: "CREATE EXTENSION \"uuid-ossp\"",
            refusal: None,
        },
        Case {
            // `aws_lambda` requires `aws_commons`, which the CASCADE above installed, so
            // this one needs no CASCADE of its own.
            case: "an extension whose requirement is already installed",
            statement: "CREATE EXTENSION aws_lambda",
            refusal: None,
        },
        Case {
            case: "a name the instance ships no extension for",
            statement: "CREATE EXTENSION not_an_extension",
            refusal: Some((
                "0A000",
                "extension \"not_an_extension\" is not available",
            )),
        },
        Case {
            // The version a client pins the install to, rather than the one the
            // extension declares as its default.
            case: "an install that names the version it wants",
            statement: "CREATE EXTENSION hstore VERSION '1.7'",
            refusal: None,
        },
        Case {
            case: "a drop of an extension another installed extension requires",
            statement: "DROP EXTENSION aws_commons",
            refusal: Some((
                "2BP01",
                "cannot drop extension aws_commons because other objects depend on it",
            )),
        },
        Case {
            case: "a drop of an extension nothing depends on",
            statement: "DROP EXTENSION pgcrypto",
            refusal: None,
        },
        Case {
            case: "a drop of an extension that is not installed",
            statement: "DROP EXTENSION pgcrypto",
            refusal: Some(("42704", "extension \"pgcrypto\" does not exist")),
        },
        Case {
            case: "the same drop under IF EXISTS",
            statement: "DROP EXTENSION IF EXISTS pgcrypto",
            refusal: None,
        },
    ];
    run_postgres(&mut session, &cases).await;

    // What the client reads back from the catalog: every install above, each with the
    // version the extension declares, and nothing the drops removed.
    let installed = session
        .query("SELECT extname, extversion FROM pg_extension ORDER BY extname")
        .await
        .expect("the catalog read runs");
    let reported: Vec<(String, String)> = installed
        .rows
        .iter()
        .map(|row| {
            (
                row.first().cloned().flatten().unwrap_or_default(),
                row.get(1).cloned().flatten().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![
            ("aws_commons".to_string(), "1.2".to_string()),
            ("aws_lambda".to_string(), "1.0".to_string()),
            ("aws_s3".to_string(), "1.1".to_string()),
            ("hstore".to_string(), "1.7".to_string()),
            ("plpgsql".to_string(), "1.0".to_string()),
            ("uuid-ossp".to_string(), "1.1".to_string()),
        ],
        "case {:?}: the catalog must carry exactly the extensions installed above, each at \
         the version it was installed at, got {reported:?}",
        "pg_extension read-back"
    );

    // An extension belongs to the DATABASE it was installed in: the engine's own default
    // database has none of them, so the one that needs `aws_commons` needs CASCADE there.
    let mut other = postgres_login(
        &address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some("postgres"),
    )
    .await
    .expect("the master user must be admitted on the engine's own database");
    let cases = [
        Case {
            case: "an extension with a requirement, in a database that has none, without CASCADE",
            statement: "CREATE EXTENSION aws_s3",
            refusal: Some((
                "42704",
                "required extension \"aws_commons\" is not installed",
            )),
        },
        Case {
            case: "the same install with CASCADE",
            statement: "CREATE EXTENSION aws_s3 CASCADE",
            refusal: None,
        },
    ];
    run_postgres(&mut other, &cases).await;
    let installed = other
        .query("SELECT extname FROM pg_extension ORDER BY extname")
        .await
        .expect("the catalog read runs");
    assert_eq!(
        column(&installed),
        vec![
            "aws_commons".to_string(),
            "aws_s3".to_string(),
            "plpgsql".to_string()
        ],
        "case {:?}: the other database must carry only what was installed in it, got \
         {installed:?}",
        "per-database extensions"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mysql_session_creates_the_schemas_a_later_session_connects_to() {
    let router = emulator();
    let (address, port) = available_instance(&router, "emu-my-schemas", "mysql", &[]).await;
    let mut session = mysql_login(
        &address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");

    // The forms a client writes the statement in, and every condition it defines.
    let cases = [
        Case {
            case: "a schema named plainly",
            statement: "CREATE DATABASE orders_2024",
            refusal: None,
        },
        Case {
            case: "a schema created with the SCHEMA spelling of the same statement",
            statement: "CREATE SCHEMA metrics",
            refusal: None,
        },
        Case {
            case: "a schema whose name has to be written quoted",
            statement: "CREATE DATABASE `ledger-archive`",
            refusal: None,
        },
        Case {
            case: "a schema created with the options a client's DDL carries",
            statement: "CREATE DATABASE reports DEFAULT CHARACTER SET utf8mb4 \
                        COLLATE utf8mb4_0900_ai_ci",
            refusal: None,
        },
        Case {
            case: "a second create of a schema that is already there",
            statement: "CREATE DATABASE orders_2024",
            refusal: Some((
                "1007",
                "Can't create database 'orders_2024'; database exists",
            )),
        },
        Case {
            case: "the same create under IF NOT EXISTS",
            statement: "CREATE DATABASE IF NOT EXISTS orders_2024",
            refusal: None,
        },
        Case {
            case: "a create naming the database the instance was created with",
            statement: "CREATE DATABASE appdb",
            refusal: Some(("1007", "Can't create database 'appdb'; database exists")),
        },
        Case {
            case: "a drop of a schema the instance holds",
            statement: "DROP DATABASE reports",
            refusal: None,
        },
        Case {
            case: "a second drop of the same schema",
            statement: "DROP DATABASE reports",
            refusal: Some((
                "1008",
                "Can't drop database 'reports'; database doesn't exist",
            )),
        },
        Case {
            case: "the same drop under IF EXISTS",
            statement: "DROP DATABASE IF EXISTS reports",
            refusal: None,
        },
        Case {
            case: "a drop of one of the server's own schemas",
            statement: "DROP DATABASE performance_schema",
            refusal: Some((
                "1044",
                "Access denied for user 'emuadmin'@'127.0.0.1' to database \
                 'performance_schema'",
            )),
        },
    ];
    run_mysql(&mut session, &cases).await;

    // The schemas the server now reports: what the instance was created with, the
    // server's own, and the ones the statements above created — and not the dropped one.
    let listed = session
        .query("SHOW DATABASES")
        .await
        .expect("SHOW DATABASES runs");
    assert_eq!(
        column(&listed),
        vec![
            "appdb".to_string(),
            "information_schema".to_string(),
            "ledger-archive".to_string(),
            "metrics".to_string(),
            "mysql".to_string(),
            "orders_2024".to_string(),
            "performance_schema".to_string(),
            "sys".to_string(),
        ],
        "case {:?}: the listing must be the schemas the instance holds, got {listed:?}",
        "SHOW DATABASES"
    );

    // A schema is a namespace of its own: a table created in one is not in another.
    for statement in [
        "USE orders_2024",
        "CREATE TABLE lines (id int PRIMARY KEY, sku varchar(32))",
        "INSERT INTO lines (id, sku) VALUES (1, 'widget')",
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    session
        .query("USE appdb")
        .await
        .expect("the instance's own database is one it has");
    let elsewhere = session
        .query("SELECT id FROM lines")
        .await
        .expect_err("a table of another schema is not in this one");
    assert_eq!(
        elsewhere.code, "1146",
        "case {:?}: a table the session's schema does not have must be refused as one that \
         does not exist, got {elsewhere:?}",
        "schema isolation"
    );

    // And a client names a table of another schema by qualifying it, from whichever
    // schema its session is on.
    for statement in [
        "CREATE TABLE metrics.readings (id int PRIMARY KEY, value decimal(6,2))",
        "INSERT INTO metrics.readings (id, value) VALUES (1, 2.50)",
    ] {
        session
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    let qualified = session
        .query("SELECT readings.value FROM metrics.readings WHERE readings.id = 1")
        .await
        .expect("a qualified read runs");
    assert_eq!(
        column(&qualified),
        vec!["2.5".to_string()],
        "case {:?}: a qualified name must reach the table in the schema it names, got \
         {qualified:?}",
        "qualified table name"
    );
    let across = session
        .query("SELECT 1 FROM metrics.readings, orders_2024.lines")
        .await
        .expect_err("one statement over two schemas is not one this endpoint serves");
    assert_eq!(
        across.code, "1105",
        "case {:?}: a statement over two schemas must be refused as this endpoint's own \
         limit, got {across:?}",
        "cross-schema statement"
    );
    session
        .query("USE metrics")
        .await
        .expect("the created schema is one the instance has");
    let unqualified = session
        .query("SELECT value FROM readings")
        .await
        .expect("the same table, unqualified, from the schema it is in");
    assert_eq!(
        column(&unqualified),
        vec!["2.5".to_string()],
        "case {:?}: the row written through a qualified name must be the row the schema \
         holds, got {unqualified:?}",
        "qualified write read back unqualified"
    );
    session
        .query("USE appdb")
        .await
        .expect("the instance's own database is one it has");

    // And the created schema is the instance's, not the session's: a new login names it.
    let mut later = mysql_login(
        &address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some("orders_2024"),
    )
    .await
    .expect("a created schema must be one a later session can connect to");
    let rows = later
        .query("SELECT id, sku FROM lines")
        .await
        .expect("the read-back runs");
    assert_eq!(
        rows.rows,
        vec![vec![Some("1".to_string()), Some("widget".to_string())]],
        "case {:?}: the rows written in the created schema must be the rows a later session \
         reads, got {rows:?}",
        "created schema round trip"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ending_a_transaction_that_was_never_opened_succeeds_on_both_wires() {
    let router = emulator();
    let (my_address, my_port) = available_instance(&router, "emu-my-txn", "mysql", &[]).await;
    let mut mysql = mysql_login(
        &my_address,
        my_port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");

    // What a driver does around a statement batch, with autocommit on and no explicit
    // transaction: it commits anyway, and the server answers OK.
    let cases = [
        Case {
            case: "a commit with no transaction open",
            statement: "COMMIT",
            refusal: None,
        },
        Case {
            case: "a rollback with no transaction open",
            statement: "ROLLBACK",
            refusal: None,
        },
        Case {
            case: "a table created outside any transaction",
            statement: "CREATE TABLE gauges (id int PRIMARY KEY, label varchar(32))",
            refusal: None,
        },
        Case {
            case: "a write a driver commits defensively",
            statement: "INSERT INTO gauges (id, label) VALUES (1, 'autocommitted')",
            refusal: None,
        },
        Case {
            case: "the commit that follows it",
            statement: "COMMIT",
            refusal: None,
        },
        Case {
            case: "a transaction opened in the engine's own words",
            statement: "START TRANSACTION",
            refusal: None,
        },
        Case {
            case: "a write inside it",
            statement: "INSERT INTO gauges (id, label) VALUES (2, 'committed')",
            refusal: None,
        },
        Case {
            // "Beginning a transaction causes any pending transaction to be committed."
            case: "a begin while a transaction is already open",
            statement: "BEGIN",
            refusal: None,
        },
        Case {
            case: "a write inside the transaction that begin opened",
            statement: "INSERT INTO gauges (id, label) VALUES (3, 'rolled back')",
            refusal: None,
        },
        Case {
            case: "the rollback of that one",
            statement: "ROLLBACK",
            refusal: None,
        },
        Case {
            case: "a commit after everything was already ended",
            statement: "COMMIT",
            refusal: None,
        },
    ];
    run_mysql(&mut mysql, &cases).await;
    let gauges = mysql
        .query("SELECT label FROM gauges ORDER BY id")
        .await
        .expect("the read-back runs");
    assert_eq!(
        column(&gauges),
        vec!["autocommitted".to_string(), "committed".to_string()],
        "case {:?}: the committed rows must be there and the rolled-back one must not, got \
         {gauges:?}",
        "mysql transaction control"
    );

    // The same on the PostgreSQL wire, where ending a transaction that was never opened
    // succeeds with a warning rather than an error.
    let (pg_address, pg_port) = available_instance(&router, "emu-pg-txn", "postgres", &[]).await;
    let mut postgres = postgres_login(
        &pg_address,
        pg_port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");
    for (case, statement, expected) in [
        (
            "a commit with no transaction open",
            "COMMIT",
            Some(("25P01", "there is no transaction in progress")),
        ),
        (
            "a rollback with no transaction open",
            "ROLLBACK",
            Some(("25P01", "there is no transaction in progress")),
        ),
        ("a transaction opened", "BEGIN", None),
        (
            "a begin while a transaction is already open",
            "BEGIN",
            Some(("25001", "there is already a transaction in progress")),
        ),
        ("the commit that ends it", "COMMIT", None),
    ] {
        let answer = postgres
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: {statement:?} must be served, got {refusal:?}"));
        let reported: Vec<(String, String)> = answer
            .notices
            .iter()
            .map(|notice| (notice.code.clone(), notice.message.clone()))
            .collect();
        match expected {
            Some((code, message)) => assert_eq!(
                reported,
                vec![(code.to_string(), message.to_string())],
                "case {case:?}: the server must say so and serve the statement, got {reported:?}"
            ),
            None => assert!(
                reported.is_empty(),
                "case {case:?}: the server has nothing to say about this one, got {reported:?}"
            ),
        }
    }
    for statement in [
        "CREATE TABLE ledger (entry text)",
        "BEGIN",
        "INSERT INTO ledger (entry) VALUES ('committed')",
        "COMMIT",
        "ROLLBACK",
    ] {
        postgres
            .query(statement)
            .await
            .unwrap_or_else(|refusal| panic!("{statement:?} must run, got {refusal:?}"));
    }
    let ledger = postgres
        .query("SELECT entry FROM ledger")
        .await
        .expect("the read-back runs");
    assert_eq!(
        column(&ledger),
        vec!["committed".to_string()],
        "case {:?}: the committed row must be there, got {ledger:?}",
        "postgres transaction control"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statement_that_cannot_be_read_is_refused_in_its_own_engines_words() {
    let router = emulator();
    let (my_address, my_port) = available_instance(&router, "emu-my-parse", "mysql", &[]).await;
    let mut mysql = mysql_login(
        &my_address,
        my_port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");
    let cases = [
        Case {
            case: "a verb no engine has",
            statement: "SELEKT 1",
            refusal: Some((
                "1064",
                "You have an error in your SQL syntax; check the manual that corresponds to \
                 your MySQL server version for the right syntax to use near 'SELEKT 1' at \
                 line 1",
            )),
        },
        Case {
            case: "a statement of the other engine's grammar",
            statement: "CREATE EXTENSION aws_s3",
            refusal: Some((
                "1064",
                "You have an error in your SQL syntax; check the manual that corresponds to \
                 your MySQL server version for the right syntax to use near 'EXTENSION \
                 aws_s3' at line 1",
            )),
        },
    ];
    run_mysql(&mut mysql, &cases).await;
    // A statement that goes wrong on its second line is reported on that line.
    let multiline = mysql
        .query("SELECT 1\nFROM FROM gauges")
        .await
        .expect_err("a statement the engine cannot read must be refused");
    assert_eq!(
        multiline.code, "1064",
        "case {:?}: a parse failure must carry the server's parse-error number, got \
         {multiline:?}",
        "multi-line parse failure"
    );
    assert!(
        multiline.message.starts_with(
            "You have an error in your SQL syntax; check the manual that corresponds to your \
             MySQL server version for the right syntax to use near 'FROM"
        ) && multiline.message.ends_with("at line 2"),
        "case {:?}: the message must quote the statement from where the server stopped and \
         name the line it stopped on, got {multiline:?}",
        "multi-line parse failure"
    );

    let (pg_address, pg_port) = available_instance(&router, "emu-pg-parse", "postgres", &[]).await;
    let mut postgres = postgres_login(
        &pg_address,
        pg_port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted");
    let cases = [
        Case {
            case: "a verb no engine has",
            statement: "SELEKT 1",
            refusal: Some(("42601", "syntax error at or near \"SELEKT\"")),
        },
        Case {
            case: "a statement that leaves out what it is about",
            statement: "UPDATE SET entry = 'x'",
            refusal: Some(("42601", "syntax error at or near \"SET\"")),
        },
    ];
    run_postgres(&mut postgres, &cases).await;
}
