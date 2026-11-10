// SPDX-License-Identifier: CC0-1.0

//! Which DEFECT a malformed or inadmissible PartiQL statement is reported as, per class.
//!
//! Two rules are pinned here, each over a table of statements rather than one recorded
//! request:
//!
//! * a statement the dialect cannot parse is refused with the parser's own phrase for the
//!   POSITION the parse stopped at, so two statements that stop at different positions are
//!   two different diagnoses: a `SELECT` list with no `FROM`, an `UPDATE` stating neither
//!   action, and an `INSERT` stopping where its VALUE belongs are three classes, not one;
//! * a read addressed at an INDEX is admitted by no transaction, and the refusal names the
//!   `TransactStatements` slot that carried it. The same statement is a per-slot error of
//!   `BatchExecuteStatement` — a different surface with its own sentence — and is SERVED
//!   by the single-statement surface, which reads an index like any other query.
//!
//! Everything is driven over the wire, so nothing is asserted that a client could not
//! observe.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every statement below names, and the index on it the transaction rows read.
const TABLE: &str = "StatementClasses";
const INDEX: &str = "by-owner";

/// The fixed sentence a parse defect is stated with, over the parser's phrase for the
/// position the parse stopped at.
fn parse_failure(phrase: &str) -> String {
    format!("Statement wasn't well formed, can't be processed: {phrase}")
}

/// An emulator holding one active table with a global secondary index, and one item.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "owner", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": INDEX,
                "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the statement-classes table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"pk": {"S": "a"}, "owner": {"S": "o1"}, "n": {"N": "1"}},
        }),
    )
    .await;
    router
}

#[tokio::test]
async fn each_malformed_statement_class_states_its_own_diagnosis() {
    let router = seeded().await;

    // Each row: the case, the statement, and the whole message it must earn. The rows
    // differ in WHERE the statement stops — after a `SELECT` list, after an `UPDATE`'s
    // source, after an `INSERT`'s target, after its `VALUE` keyword — and in the verb
    // that opened it; a statement that parses and then asks for something the surface
    // does not serve states its own rule instead, which the last rows show.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a SELECT list with no FROM",
            format!("SELECT * \"{TABLE}\""),
            parse_failure("Expected FROM after SELECT list"),
        ),
        (
            "a SELECT list with no FROM and a named projection",
            format!("SELECT pk, n \"{TABLE}\""),
            parse_failure("Expected FROM after SELECT list"),
        ),
        (
            "an UPDATE stating neither SET nor REMOVE",
            format!("UPDATE \"{TABLE}\" WHERE pk = 'a'"),
            parse_failure("Expected data manipulation"),
        ),
        (
            "an UPDATE stating nothing after its source",
            format!("UPDATE \"{TABLE}\""),
            parse_failure("Expected data manipulation"),
        ),
        (
            "text that states no manipulation of data at all",
            format!("\"{TABLE}\""),
            parse_failure("Expected data manipulation"),
        ),
        (
            "an INSERT stopping after its target",
            format!("INSERT INTO \"{TABLE}\""),
            parse_failure("Unexpected term"),
        ),
        (
            "an INSERT stopping after its VALUE keyword",
            format!("INSERT INTO \"{TABLE}\" VALUE"),
            parse_failure("Unexpected term"),
        ),
        (
            "an INSERT stopping after VALUES",
            format!("INSERT INTO \"{TABLE}\" VALUES"),
            parse_failure("Unexpected term"),
        ),
        (
            "a DELETE stating no WHERE clause",
            format!("DELETE FROM \"{TABLE}\""),
            "Where clause does not contain a mandatory equality on all key attributes".to_string(),
        ),
    ];

    for (case, statement, expected) in rows {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?} ({statement}): expected {expected:?}, got {status} with {answered}"
        );
    }

    // The well-formed statements of the same classes are served, so the rows above are
    // the missing clause being reported rather than the verb being refused.
    for (case, statement) in [
        ("a complete INSERT", format!(
            "INSERT INTO \"{TABLE}\" VALUE {{'pk': 'b', 'owner': 'o2'}}"
        )),
        ("a complete UPDATE", format!(
            "UPDATE \"{TABLE}\" SET n = 2 WHERE pk = 'a'"
        )),
        ("a complete SELECT", format!(
            "SELECT * FROM \"{TABLE}\" WHERE pk = 'a'"
        )),
    ] {
        let (status, answered) =
            call(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({statement}): expected HTTP 200, got {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn an_index_read_inside_a_transaction_is_refused_at_the_slot_that_carried_it() {
    let router = seeded().await;

    // The rows differ in WHICH slot carries the index read, and so in the position the
    // message names: the statements beside it are well formed, and the one addressed at
    // the index is the one reported.
    let index_read = format!("SELECT * FROM \"{TABLE}\".\"{INDEX}\" WHERE owner = 'o1'");
    let table_read = format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'");
    for (case, statements, slot) in [
        ("the only statement", vec![index_read.clone()], 0usize),
        (
            "the second of two statements",
            vec![table_read.clone(), index_read.clone()],
            1usize,
        ),
        (
            "the first of three statements",
            vec![index_read.clone(), table_read.clone(), table_read.clone()],
            0usize,
        ),
    ] {
        let transact: Vec<Value> = statements
            .iter()
            .map(|statement| json!({"Statement": statement}))
            .collect();
        let expected = format!(
            "Validation failed in TransactStatements[{slot}]: Reads on indices are not supported \
             within transactions."
        );
        let (status, answered) = call(
            &router,
            "ExecuteTransaction",
            json!({"TransactStatements": transact}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
        // A top-level validation refused the call as a whole: the transaction was never
        // attempted, so there is nothing to report a cancellation of.
        assert_eq!(
            answered.get("CancellationReasons"),
            None,
            "case {case:?}: expected no cancellation, got {answered}"
        );
    }

    // The SAME statement on the other two surfaces: a per-slot error of the batch, with
    // that surface's own sentence and code, and a served read on the single-statement
    // surface, which reads an index like any other query.
    let (status, batched) = call(
        &router,
        "BatchExecuteStatement",
        json!({"Statements": [{"Statement": index_read}]}),
    )
    .await;
    assert_eq!(
        (
            status,
            batched["Responses"][0]["Error"]["Code"].clone(),
            batched["Responses"][0]["Error"]["Message"].clone()
        ),
        (
            200,
            json!("ValidationError"),
            json!(
                "Select statements within BatchExecuteStatement must specify the primary key in \
                 the where clause."
            )
        ),
        "case the batch surface: expected its own per-slot error, got {status} with {batched}"
    );

    let served = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": format!("SELECT * FROM \"{TABLE}\".\"{INDEX}\" WHERE owner = 'o1'")}),
    )
    .await;
    assert_eq!(
        served["Items"][0]["pk"].clone(),
        json!({"S": "a"}),
        "case the single-statement surface: an index read is served, got {served}"
    );
}
