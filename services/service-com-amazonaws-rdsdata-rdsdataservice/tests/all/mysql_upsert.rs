// SPDX-License-Identifier: CC0-1.0

//! The UPSERT the RDS Data API serves, over the wire.
//!
//! RULE: the Data API serves Aurora MySQL DB clusters as well as Aurora PostgreSQL
//! ones, so a statement written in MySQL's dialect runs as that statement — and
//! MySQL's upsert, `INSERT … ON DUPLICATE KEY UPDATE <assignments>`, inserts the row
//! where no key of the table conflicts and otherwise updates the named columns of the
//! row already there, with `VALUES(<column>)` standing for the value the insert half
//! would have stored. A table whose declaration carries no key can raise no
//! duplicate-key condition, so the clause never fires and the insert alone runs. The
//! engine's own spelling of that same statement (`ON CONFLICT (<key>) DO UPDATE SET …`,
//! which is PostgreSQL's) keeps working, since both dialects' clients reach one data
//! plane.
//!
//! The rows span what the rule generalises over: a fresh row and a conflicting one,
//! an assignment reading `VALUES(<column>)`, a literal and an expression over the
//! stored row, one assignment and several, a single-column primary key, a composite
//! one, a UNIQUE key declared without a primary key, a table whose declaration carries no
//! key at all, and a multi-row insert in which one row conflicts and one does not.

use serde_json::json;

use crate::sql_data_api::{list, number, DataApi};

/// The rows one table holds after a statement, read back as `(id, val, hits)` text.
async fn rows_of(api: &DataApi, table: &str) -> Vec<String> {
    let body = api
        .run(json!({"sql": format!("SELECT * FROM {table} ORDER BY 1")}))
        .await;
    list(&body, "records")
        .iter()
        .map(::std::string::ToString::to_string)
        .collect()
}

/// An upsert inserts a row that conflicts with nothing and updates the one it
/// conflicts with, and the update half reads the values the insert half would have
/// stored.
#[tokio::test]
async fn a_mysql_upsert_inserts_a_new_row_and_updates_the_one_it_conflicts_with() {
    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE IF NOT EXISTS export_rows \
                (id INT PRIMARY KEY, val VARCHAR(50), hits INT)",
    }))
    .await;

    // The insert half: nothing conflicts, so the row is stored as written.
    let inserted = api
        .run(json!({
            "sql": "INSERT INTO export_rows (id, val, hits) VALUES (1, 'alpha', 1) \
                    ON DUPLICATE KEY UPDATE val = VALUES(val), hits = hits + 1",
        }))
        .await;
    assert_eq!(
        number(&inserted, "numberOfRecordsUpdated"),
        1,
        "case \"the insert half\": the upsert stored one row, got {inserted}"
    );
    let rows = rows_of(&api, "export_rows").await;
    assert!(
        rows.len() == 1 && rows[0].contains("alpha"),
        "case \"the insert half\": expected the row as written, got {rows:?}"
    );

    // The update half: the same key, so the named columns of the stored row are
    // updated — `VALUES(val)` is the value this statement would have inserted, and an
    // expression over the stored row reads what is already there. What is asserted is the
    // stored row: the count MySQL reports for a row an upsert UPDATED is the engine's own,
    // and this suite pins the effect rather than that count.
    api.run(json!({
        "sql": "INSERT INTO export_rows (id, val, hits) VALUES (1, 'beta', 1) \
                ON DUPLICATE KEY UPDATE val = VALUES(val), hits = hits + 1",
    }))
    .await;
    let rows = rows_of(&api, "export_rows").await;
    assert!(
        rows.len() == 1 && rows[0].contains("beta") && rows[0].contains('2'),
        "case \"the update half\": expected the stored row updated in place, got {rows:?}"
    );

    // A multi-row insert in which one row conflicts and one does not: the engine
    // applies the clause per row.
    api.run(json!({
        "sql": "INSERT INTO export_rows (id, val, hits) VALUES (1, 'gamma', 1), (2, 'delta', 1) \
                ON DUPLICATE KEY UPDATE val = VALUES(val)",
    }))
    .await;
    let rows = rows_of(&api, "export_rows").await;
    assert!(
        rows.len() == 2
            && rows[0].contains("gamma")
            && rows[0].contains('2')
            && rows[1].contains("delta"),
        "case \"one row conflicting and one not\": expected the first updated and the \
         second inserted, got {rows:?}"
    );
}

/// The forms of the clause, over a table whose key is composite and over one the
/// engine reports no key for.
#[tokio::test]
async fn the_upsert_conflicts_on_the_key_the_table_declares() {
    let api = DataApi::new();
    api.run(json!({
        "sql": "CREATE TABLE pairs (left_id INT, right_id INT, label VARCHAR(20), \
                PRIMARY KEY (left_id, right_id))",
    }))
    .await;
    api.run(json!({
        "sql": "INSERT INTO pairs (left_id, right_id, label) VALUES (1, 2, 'first') \
                ON DUPLICATE KEY UPDATE label = VALUES(label)",
    }))
    .await;
    // Only the row whose WHOLE key matches conflicts: one column in common is a
    // different row.
    api.run(json!({
        "sql": "INSERT INTO pairs (left_id, right_id, label) VALUES (1, 3, 'other') \
                ON DUPLICATE KEY UPDATE label = VALUES(label)",
    }))
    .await;
    api.run(json!({
        "sql": "INSERT INTO pairs (left_id, right_id, label) VALUES (1, 2, 'again') \
                ON DUPLICATE KEY UPDATE label = VALUES(label)",
    }))
    .await;
    let rows = rows_of(&api, "pairs").await;
    assert!(
        rows.len() == 2 && rows[0].contains("again") && rows[1].contains("other"),
        "case \"a composite key\": expected the whole key to decide the conflict, got {rows:?}"
    );

    // A table whose declaration carries a UNIQUE key but no primary key conflicts on THAT
    // key: MySQL's clause fires on any unique key of the table.
    api.run(json!({
        "sql": "CREATE TABLE tagged (slug VARCHAR(20) UNIQUE, hits INT)",
    }))
    .await;
    for (slug, hits) in [("one", 1), ("one", 2), ("two", 1)] {
        api.run(json!({
            "sql": format!(
                "INSERT INTO tagged (slug, hits) VALUES ('{slug}', {hits}) \
                 ON DUPLICATE KEY UPDATE hits = VALUES(hits)"
            ),
        }))
        .await;
    }
    let rows = rows_of(&api, "tagged").await;
    assert!(
        rows.len() == 2 && rows[0].contains('2'),
        "case \"a UNIQUE key and no primary key\": expected the conflicting row updated in \
         place, got {rows:?}"
    );

    // A table whose declaration carries NO key at all can raise no duplicate-key
    // condition, so the clause can never fire and the statement is the insert alone: the
    // row is stored, exactly as MySQL stores it.
    api.run(json!({"sql": "CREATE TABLE unkeyed (id INT, val VARCHAR(10))"}))
        .await;
    for value in ["a", "b"] {
        let (status, body) = api
            .execute(json!({
                "sql": format!(
                    "INSERT INTO unkeyed (id, val) VALUES (1, '{value}') \
                     ON DUPLICATE KEY UPDATE val = VALUES(val)"
                ),
            }))
            .await;
        assert_eq!(
            status, 200,
            "case \"a table with no key\": the insert MySQL runs for such a table must be \
             served, got {status} with {body}"
        );
    }
    let rows = rows_of(&api, "unkeyed").await;
    assert_eq!(
        rows.len(),
        2,
        "case \"a table with no key\": both rows are stored, since nothing can conflict, \
         got {rows:?}"
    );
}

/// The engine's own spelling of the same upsert — PostgreSQL's — still runs, so a
/// client of either dialect reaches the one data plane.
#[tokio::test]
async fn the_postgresql_spelling_of_the_upsert_still_runs() {
    let api = DataApi::new();
    api.run(json!({"sql": "CREATE TABLE counters (id INT PRIMARY KEY, hits INT)"}))
        .await;
    for (case, sql) in [
        (
            "do nothing",
            "INSERT INTO counters (id, hits) VALUES (1, 1) ON CONFLICT (id) DO NOTHING",
        ),
        (
            "do update",
            "INSERT INTO counters (id, hits) VALUES (1, 5) ON CONFLICT (id) \
             DO UPDATE SET hits = excluded.hits",
        ),
    ] {
        let (status, body) = api.execute(json!({"sql": sql})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the engine's own upsert spelling must run, got {status} with {body}"
        );
    }
    let rows = rows_of(&api, "counters").await;
    assert!(
        rows.len() == 1 && rows[0].contains('5'),
        "case \"the PostgreSQL spelling\": expected the stored row updated, got {rows:?}"
    );
}
