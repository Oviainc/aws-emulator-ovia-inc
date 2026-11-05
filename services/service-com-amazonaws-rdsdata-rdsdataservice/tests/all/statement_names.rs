// SPDX-License-Identifier: CC0-1.0

//! The names a statement uses, and what `columnMetadata` says about where a
//! result column lives.
//!
//! Every call goes through the router, so routing, restJson1 deserialization, the
//! handler and serialization are all exercised; the client harness is the one
//! `sql_data_api` builds.
//!
//! The rule under test is stated once above the test that pins it, and every
//! table row is one input of the class the rule generalises over.

use serde_json::{json, Value};

use crate::sql_data_api::{error_code, list, number, DataApi};

/// Words PostgreSQL admits as a name.
///
/// The first group is PostgreSQL's NON-RESERVED key words, which its "SQL Key
/// Words" appendix defines as the words that "can be used as identifiers" —
/// including ones that are a key word of the engine behind this API, so a
/// statement using one as a name is a statement the two grammars read
/// differently. The second group is words PostgreSQL does not list at all, which
/// are therefore names too.
///
/// Spanning entries, not a complete list: statement words (`commit`, `rollback`,
/// `insert`, `delete`, `set`, `escape`), object words (`index`, `transaction`,
/// `temp`, `release`, `key`, `action`), words a query clause uses (`filter`,
/// `over`, `nothing`) and the two the appendix does not carry (`isnull`,
/// `notnull`).
const POSTGRESQL_NAMES: [&str; 17] = [
    "commit",
    "rollback",
    "insert",
    "delete",
    "set",
    "escape",
    "index",
    "transaction",
    "temp",
    "release",
    "key",
    "action",
    "filter",
    "over",
    "nothing",
    "isnull",
    "notnull",
];

/// RULE: a word PostgreSQL admits as a name is a usable name here, in every
/// position a name appears in — an alias, a table, a column, a qualified
/// reference, a wildcard's prefix, an assignment target, an upsert's conflict
/// target, a column added after the fact — whatever word the engine behind the API
/// reserves for its own grammar; the statement runs and the result column reports
/// that name.
///
/// The rows differ along the NAME (the catalog above) and, for each, along the
/// POSITION it is used in.
#[tokio::test]
async fn a_postgresql_name_is_a_name_whatever_the_engine_reserves() {
    for name in POSTGRESQL_NAMES {
        let api = DataApi::new();

        // An alias: the name the result column reports.
        let aliased = api
            .run(json!({
                "sql": format!("SELECT 1 AS {name}"),
                "includeResultMetadata": true,
            }))
            .await;
        let metadata = list(&aliased, "columnMetadata");
        assert_eq!(
            metadata.first().map(|column| column["label"].clone()),
            Some(json!(name)),
            "case {name}: expected the alias as the column's label, got {aliased}"
        );
        assert_eq!(
            aliased["records"],
            json!([[{"longValue": 1}]]),
            "case {name}: expected the selected value, got {aliased}"
        );

        // A table and a column of that name, written and read back.
        for statement in [
            format!("CREATE TABLE {name} ({name} TEXT PRIMARY KEY, seen INT)"),
            format!("INSERT INTO {name} ({name}, seen) VALUES ('stored', 1)"),
            format!("UPDATE {name} SET {name} = 'updated' WHERE seen = 1"),
            // An upsert names the column it conflicts on and the columns it then
            // assigns.
            format!(
                "INSERT INTO {name} ({name}, seen) VALUES ('updated', 2) \
                 ON CONFLICT ({name}) DO UPDATE SET seen = 1"
            ),
            // A column added after the fact is named the same way a declared one is.
            format!("ALTER TABLE {name} ADD COLUMN {name}_note TEXT"),
        ] {
            let answered = api.run(json!({"sql": statement.clone()})).await;
            assert_eq!(
                answered["records"],
                Value::Null,
                "case {name}: {statement} returns no result set, got {answered}"
            );
        }
        for read in [
            format!("SELECT {name} FROM {name} WHERE {name} = 'updated' ORDER BY {name}"),
            format!("SELECT t.{name} AS {name} FROM {name} AS t"),
            format!("SELECT {name} FROM {name} GROUP BY {name} HAVING count(*) = 1"),
            // The prefix of a qualified wildcard is the alias it stands for.
            format!("SELECT {name}.* FROM {name} AS {name}"),
        ] {
            let selected = api
                .run(json!({"sql": read.clone(), "includeResultMetadata": true}))
                .await;
            assert_eq!(
                selected["records"][0][0],
                json!({"stringValue": "updated"}),
                "case {name}: expected the stored row from {read}, got {selected}"
            );
            let metadata = list(&selected, "columnMetadata");
            assert_eq!(
                metadata.first().map(|column| column["name"].clone()),
                Some(json!(name)),
                "case {name}: expected the column's own name from {read}, got {selected}"
            );
        }

        // The names the statement used addressed one row, which is gone again.
        let deleted = api.run(json!({"sql": format!("DELETE FROM {name}")})).await;
        assert_eq!(
            number(&deleted, "numberOfRecordsUpdated"),
            1,
            "case {name}: the stored row was deleted, got {deleted}"
        );
    }
}

/// RULE: a name that resolves to nothing is refused — there is no column or table
/// of that name — and is never read as a text value, because a name is a name in
/// whatever position and whatever notation the caller wrote it in.
///
/// The rows differ along the position the unresolvable name is in and along
/// whether the caller delimited it.
#[tokio::test]
async fn a_name_that_resolves_to_nothing_is_refused() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE present (here TEXT)"}))
        .await;
    api.run(json!({"sql": "INSERT INTO present (here) VALUES ('stored')"}))
        .await;

    for sql in [
        "SELECT absent",
        "SELECT \"absent\"",
        "SELECT absent FROM present",
        "SELECT present.absent FROM present",
        "SELECT here FROM absent",
        "SELECT here FROM \"absent\"",
        "INSERT INTO present (absent) VALUES ('x')",
        "UPDATE present SET absent = 'x'",
        "SELECT here FROM present ORDER BY absent",
    ] {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(
            status, 400,
            "case {sql}: an unresolvable name is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {sql}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            body["records"],
            Value::Null,
            "case {sql}: a refused statement returns no records, got {body}"
        );
    }
}

/// RULE: a statement whose text asks the statement tree for more levels than one
/// call may build is REFUSED, with the modelled answer and no process death: the
/// bounds are applied to the text before a tree exists, so the parse, the
/// delimiting of its names and the render that follows all run over a tree whose
/// depth is already settled.
///
/// The rows differ along WHICH way the text asks for depth: the queries a set
/// operation combines, a chain of operators (which the parse builds iteratively,
/// so no parser limit sees it), nested parentheses (which it builds recursively),
/// and a chain just inside the bound, which is answered by the engine rather than
/// refused up front.
#[tokio::test]
async fn a_statement_deeper_than_one_call_may_build_is_refused() {
    let api = DataApi::new();
    let cases = [
        (
            // 4000 branches, which is under the modelled 65536-character limit on
            // `sql`, so it is the DEPTH bound that answers and not the length one.
            "a chain of 4000 combined queries",
            format!("SELECT 1{}", " UNION SELECT 1".repeat(4_000)),
        ),
        (
            "a chain of 5000 operators",
            format!("SELECT 1{}", " AND 1".repeat(5_000)),
        ),
        (
            "5000 nested parentheses",
            format!("SELECT {}1{}", "(".repeat(5_000), ")".repeat(5_000)),
        ),
        (
            "a chain of 900 operators, inside the bound",
            format!("SELECT 1{}", " AND 1".repeat(900)),
        ),
    ];

    for (case, sql) in cases {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {body}"
        );
    }

    // The data plane is still serving after every one of them.
    let selected = api.run(json!({"sql": "SELECT 1 AS alive"})).await;
    assert_eq!(
        selected["records"],
        json!([[{"longValue": 1}]]),
        "case after the refusals: the data plane still answers, got {selected}"
    );
}

/// RULE: `columnMetadata` reports, for EVERY result column, where the column
/// lives and whether a NULL may appear in it: `tableName` is the table that
/// stores it, empty for a column no table stores, and `nullable` is
/// `java.sql.ResultSetMetaData`'s code — 0 where the declaration forbids a NULL
/// (`NOT NULL`, and a primary key, which rejects one whatever else its
/// declaration says) and 1 everywhere else, a column that accepts one and a
/// computed column alike.
///
/// The rows differ along what the column is: a key, a `NOT NULL` column, a
/// column with neither, and computed columns of each form (a literal, a function
/// of a stored column, an aggregate).
#[tokio::test]
async fn column_metadata_reports_where_a_column_lives_and_whether_a_null_fits() {
    struct Case {
        sql: &'static str,
        table_name: &'static str,
        nullable: i64,
    }

    let cases = [
        Case {
            sql: "SELECT id FROM ledger",
            table_name: "ledger",
            nullable: 0,
        },
        Case {
            sql: "SELECT label FROM ledger",
            table_name: "ledger",
            nullable: 0,
        },
        Case {
            sql: "SELECT note FROM ledger",
            table_name: "ledger",
            nullable: 1,
        },
        Case {
            sql: "SELECT 1 AS computed",
            table_name: "",
            nullable: 1,
        },
        Case {
            sql: "SELECT upper(note) AS shouted FROM ledger",
            table_name: "",
            nullable: 1,
        },
        Case {
            sql: "SELECT count(*) AS counted FROM ledger",
            table_name: "",
            nullable: 1,
        },
    ];

    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE ledger (id INT PRIMARY KEY, label TEXT NOT NULL, note TEXT)",
    }))
    .await;
    api.run(json!({
        "sql": "INSERT INTO ledger (id, label, note) VALUES (1, 'first', NULL)",
    }))
    .await;

    for case in cases {
        let selected = api
            .run(json!({"sql": case.sql, "includeResultMetadata": true}))
            .await;
        let metadata = list(&selected, "columnMetadata");
        let column = metadata
            .first()
            .unwrap_or_else(|| panic!("case {}: no columnMetadata in {selected}", case.sql));
        assert_eq!(
            column["tableName"],
            json!(case.table_name),
            "case {}: expected tableName {:?}, got {column}",
            case.sql,
            case.table_name
        );
        assert_eq!(
            number(column, "nullable"),
            case.nullable,
            "case {}: expected nullable {}, got {column}",
            case.sql,
            case.nullable
        );
    }
}
