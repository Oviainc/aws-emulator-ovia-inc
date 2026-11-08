// SPDX-License-Identifier: CC0-1.0

//! The statements the Data API's OWN ENGINES spell, and the one result a batch of no
//! parameter sets answers with, over the real wire.
//!
//! Every call goes through the router, so routing, restJson1 deserialization, the
//! handler and serialization are all exercised; the client harness is the one
//! `sql_data_api` builds.

use serde_json::{json, Value};

use crate::sql_data_api::{error_code, list, number, parameter, DataApi};

/// RULE: the Data API runs a statement written in the dialect of either engine it
/// serves, so an IDENTITY column is declared in that engine's own spelling — MySQL's
/// `AUTO_INCREMENT` column attribute or PostgreSQL's `serial` family — and either way
/// the engine assigns the column's value when an INSERT leaves it out. The value the
/// engine generated for MySQL's `AUTO_INCREMENT` key is what `generatedFields` carries
/// (one entry per execution, so one per `parameterSets` entry of a batch), while
/// PostgreSQL's is read through a RETURNING clause, because
/// `ExecuteStatementResponse.generatedFields`'s own documentation says the data "isn't
/// supported by Aurora PostgreSQL. To get the values of generated fields, use the
/// RETURNING clause".
///
/// The rows span the spellings of the rule and the widths each dialect declares an
/// identity column at; the keys asserted are the engine's own, read back through a
/// SELECT as well as off the response.
#[tokio::test]
async fn an_identity_column_is_declared_in_either_engines_spelling() {
    // (case, the column's declaration, whether the engine's generated key is the one
    //  `generatedFields` reports)
    let cases = [
        ("MySQL's integer identity key", "INT AUTO_INCREMENT", true),
        (
            "MySQL's identity key, spelled with the full type name",
            "INTEGER AUTO_INCREMENT",
            true,
        ),
        ("MySQL's wide identity key", "BIGINT AUTO_INCREMENT", true),
        (
            "MySQL's narrow identity key",
            "SMALLINT AUTO_INCREMENT",
            true,
        ),
        ("MySQL's attribute in lower case", "int auto_increment", true),
        ("PostgreSQL's sequence column", "serial", false),
        ("PostgreSQL's wide sequence column", "bigserial", false),
        ("PostgreSQL's narrow sequence column", "smallserial", false),
    ];

    for (index, (case, declared, reports_the_key)) in cases.into_iter().enumerate() {
        let api = DataApi::new();
        let table = format!("identity_{index}");
        let (status, created) = api
            .execute(json!({
                "sql": format!(
                    "CREATE TABLE {table} (id {declared} PRIMARY KEY, name VARCHAR(32))"
                ),
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the declaration must be accepted, got {status} {created}"
        );

        // The first row of a fresh table takes the first value of the sequence, and
        // each later row the next one.
        for expected_key in 1..=3_i64 {
            let inserted = api
                .run(json!({
                    "sql": format!("INSERT INTO {table} (name) VALUES (:name)"),
                    "parameters": [parameter(
                        "name",
                        json!({"stringValue": format!("row {expected_key}")}),
                    )],
                }))
                .await;
            let expected_fields = if reports_the_key {
                json!([{"longValue": expected_key}])
            } else {
                json!([])
            };
            assert_eq!(
                json!(list(&inserted, "generatedFields")),
                expected_fields,
                "case {case:?} at key {expected_key}: expected generatedFields \
                 {expected_fields}, got {inserted}"
            );
            assert_eq!(
                number(&inserted, "numberOfRecordsUpdated"),
                1,
                "case {case:?} at key {expected_key}: one row was stored, got {inserted}"
            );
        }

        // A batch runs the statement once per parameter set, so each set gets its own
        // generated key: distinct, and increasing with the sets.
        let (status, batch) = api
            .batch(json!({
                "sql": format!("INSERT INTO {table} (name) VALUES (:name)"),
                "parameterSets": [
                    [parameter("name", json!({"stringValue": "row 4"}))],
                    [parameter("name", json!({"stringValue": "row 5"}))],
                ],
            }))
            .await;
        assert_eq!(status, 200, "case {case:?}: the batch failed with {batch}");
        let batched: Vec<Value> = list(&batch, "updateResults")
            .iter()
            .map(|result| json!(list(result, "generatedFields")))
            .collect();
        let expected_batch = if reports_the_key {
            vec![json!([{"longValue": 4}]), json!([{"longValue": 5}])]
        } else {
            vec![json!([]), json!([])]
        };
        assert_eq!(
            batched, expected_batch,
            "case {case:?}: expected one generatedFields per parameter set \
             ({expected_batch:?}), got {batch}"
        );

        // And the keys the engine really assigned, read back through the engine.
        let stored = api
            .run(json!({
                "sql": format!("SELECT id FROM {table} ORDER BY id"),
                "includeResultMetadata": true,
            }))
            .await;
        assert_eq!(
            stored["records"],
            json!([
                [{"longValue": 1}],
                [{"longValue": 2}],
                [{"longValue": 3}],
                [{"longValue": 4}],
                [{"longValue": 5}],
            ]),
            "case {case:?}: every row took the next value of the sequence, got {stored}"
        );
        assert_eq!(
            list(&stored, "columnMetadata")[0]["isAutoIncrement"],
            json!(true),
            "case {case:?}: the column's value is assigned by the engine, got {stored}"
        );
    }
}

/// RULE: a statement about the cluster's DATABASES is applied to the cluster, so
/// `CREATE DATABASE <name>` makes a database the cluster HAS — one a later call selects
/// with its `database` member, whose tables are its own and invisible to every other
/// database — creating one that is already there is the database's duplicate-database
/// error unless the statement says `IF NOT EXISTS`, and `DROP DATABASE` takes it away
/// again. A call quoting a `transactionId` is served only while that transaction is
/// RUNNING, whatever its statement does.
///
/// The rows differ along the name's form (how the statement writes it) and along which
/// of the two statements is run; an unquoted name is folded to lower case, as
/// PostgreSQL folds every unquoted identifier.
#[tokio::test]
async fn a_statement_can_create_and_drop_a_database_of_the_cluster() {
    // (case, the name the statement writes, the name the `database` member selects)
    let cases = [
        ("a lower-case name", "shop", "shop"),
        ("a name with digits", "shop2", "shop2"),
        ("an unquoted name, which folds", "Shop_Three", "shop_three"),
        ("a delimited name, which does not", "\"ShopFour\"", "ShopFour"),
    ];

    for (case, written, selected) in cases {
        let api = DataApi::new();
        let created = api
            .run(json!({"sql": format!("CREATE DATABASE {written}")}))
            .await;
        assert_eq!(
            number(&created, "numberOfRecordsUpdated"),
            0,
            "case {case:?}: a statement that creates a database changes no rows, \
             got {created}"
        );

        // The database is one the cluster has, so a call may select it — and what is
        // created in it is its own.
        api.run(json!({
            "sql": "CREATE TABLE orders (id INT)",
            "database": selected,
        }))
        .await;
        api.run(json!({
            "sql": "INSERT INTO orders (id) VALUES (7)",
            "database": selected,
        }))
        .await;
        let inside = api
            .run(json!({"sql": "SELECT id FROM orders", "database": selected}))
            .await;
        assert_eq!(
            inside["records"],
            json!([[{"longValue": 7}]]),
            "case {case:?}: the row is in the database that was created, got {inside}"
        );

        // The connection's own database never saw that table.
        let (status, outside) = api.execute(json!({"sql": "SELECT id FROM orders"})).await;
        assert_eq!(
            (status, error_code(&outside)),
            (400, "DatabaseErrorException"),
            "case {case:?}: another database does not hold that table, got {status} \
             {outside}"
        );

        // Creating it again is the duplicate-database condition, in the database's own
        // words, and `IF NOT EXISTS` is what asks for it not to be.
        let (status, again) = api
            .execute(json!({"sql": format!("CREATE DATABASE {written}")}))
            .await;
        assert_eq!(
            (status, error_code(&again)),
            (400, "DatabaseErrorException"),
            "case {case:?}: a database that is already there, got {status} {again}"
        );
        assert_eq!(
            again["message"],
            json!(format!(
                "ERROR: database \"{selected}\" already exists; SQLState: 42P04"
            )),
            "case {case:?}: got {again}"
        );
        let tolerated = api
            .run(json!({"sql": format!("CREATE DATABASE IF NOT EXISTS {written}")}))
            .await;
        assert_eq!(
            number(&tolerated, "numberOfRecordsUpdated"),
            0,
            "case {case:?}: IF NOT EXISTS leaves the database as it is, got {tolerated}"
        );

        // And dropping it takes the database away, with everything it held: a call
        // naming it reaches a database that holds no such table.
        api.run(json!({"sql": format!("DROP DATABASE {written}")}))
            .await;
        let (status, gone) = api
            .execute(json!({"sql": "SELECT id FROM orders", "database": selected}))
            .await;
        assert_eq!(
            (status, error_code(&gone)),
            (400, "DatabaseErrorException"),
            "case {case:?}: the dropped database holds nothing, got {status} {gone}"
        );
        assert_eq!(
            gone["message"],
            json!("ERROR: relation \"orders\" does not exist; SQLState: 42P01"),
            "case {case:?}: got {gone}"
        );
    }

    // Dropping a database the cluster does not have is the database's own condition
    // for it, unless the statement says `IF EXISTS`.
    let api = DataApi::new();    let (status, absent) = api
        .execute(json!({"sql": "DROP DATABASE never_created"}))
        .await;
    assert_eq!(
        (status, error_code(&absent)),
        (400, "DatabaseErrorException"),
        "case \"a database that was never created\": got {status} {absent}"
    );
    assert_eq!(
        absent["message"],
        json!("ERROR: database \"never_created\" does not exist; SQLState: 3D000"),
        "case \"a database that was never created\": got {absent}"
    );
    let tolerated = api
        .run(json!({"sql": "DROP DATABASE IF EXISTS never_created"}))
        .await;
    assert_eq!(
        number(&tolerated, "numberOfRecordsUpdated"),
        0,
        "case \"IF EXISTS on a database that was never created\": got {tolerated}"
    );

    // A call quoting a transaction that is no longer RUNNING is refused whatever its
    // statement does, so a statement about the cluster's databases is refused too \u2014 and
    // the database it names is not created.
    let transaction = api.begin_transaction(None).await;
    let (status, committed) = api.commit(&transaction).await;
    assert_eq!(status, 200, "committing failed with {committed}");
    for (operation, (status, body)) in [
        (
            "ExecuteStatement",
            api.execute(json!({
                "sql": "CREATE DATABASE after_the_commit",
                "transactionId": transaction.clone(),
            }))
            .await,
        ),
        (
            "BatchExecuteStatement",
            api.batch(json!({
                "sql": "CREATE DATABASE after_the_commit",
                "transactionId": transaction.clone(),
            }))
            .await,
        ),
    ] {
        assert_eq!(
            (status, error_code(&body)),
            (404, "TransactionNotFoundException"),
            "case \"a database statement in a transaction that ended\" on {operation}: \
             got {status} {body}"
        );
    }
    let (status, absent) = api
        .execute(json!({"sql": "DROP DATABASE after_the_commit"}))
        .await;
    assert_eq!(
        (status, error_code(&absent)),
        (400, "DatabaseErrorException"),
        "case \"a database statement in a transaction that ended\": the refused call \
         created nothing, got {status} {absent}"
    );
}

/// RULE: `BatchExecuteStatement`'s `parameterSets` is optional, and a call that carries
/// none still RUNS the statement: it binds nothing, so there is exactly one execution
/// of it and the response carries exactly one `UpdateResult` — whose `generatedFields`
/// is empty for a statement that generates nothing — and no other member, because
/// `numberOfRecordsUpdated`, `records` and `formattedRecords` belong to
/// `ExecuteStatement`.
///
/// The rows differ along how the call carries no sets (the member omitted, and an empty
/// list) and along the statement's effect, so what is asserted is one execution of
/// whatever was sent and not one shape of statement.
#[tokio::test]
async fn a_batch_of_no_parameter_sets_runs_the_statement_once() {
    // (case, the statement, the rows the table holds afterwards)
    let statements = [
        (
            "an insert of its own values",
            "INSERT INTO stock (id, label) VALUES (3, 'three')",
            json!([
                [{"longValue": 1}, {"stringValue": "one"}],
                [{"longValue": 2}, {"stringValue": "two"}],
                [{"longValue": 3}, {"stringValue": "three"}],
            ]),
        ),
        (
            "an update",
            "UPDATE stock SET label = 'same'",
            json!([
                [{"longValue": 1}, {"stringValue": "same"}],
                [{"longValue": 2}, {"stringValue": "same"}],
            ]),
        ),
        (
            "a delete",
            "DELETE FROM stock WHERE id = 1",
            json!([[{"longValue": 2}, {"stringValue": "two"}]]),
        ),
    ];

    for (case, sql, remaining) in statements {
        for sets in ["omitted", "an empty list"] {
            let api = DataApi::new();
            api.run(json!({"sql": "CREATE TABLE stock (id INT, label VARCHAR(8))"}))
                .await;
            api.run(json!({
                "sql": "INSERT INTO stock (id, label) VALUES (1, 'one'), (2, 'two')",
            }))
            .await;

            let mut members = json!({"sql": sql});
            if sets == "an empty list" {
                members["parameterSets"] = json!([]);
            }
            let (status, body) = api.batch(members).await;
            assert_eq!(
                status, 200,
                "case {case:?} with parameterSets {sets}: got {status} {body}"
            );
            assert_eq!(
                body,
                json!({"updateResults": [{"generatedFields": []}]}),
                "case {case:?} with parameterSets {sets}: expected exactly one empty \
                 UpdateResult and no other member, got {body}"
            );

            let after = api
                .run(json!({"sql": "SELECT id, label FROM stock ORDER BY id"}))
                .await;
            assert_eq!(
                after["records"], remaining,
                "case {case:?} with parameterSets {sets}: the statement ran once, \
                 expected {remaining}, got {after}"
            );
        }
    }
}
