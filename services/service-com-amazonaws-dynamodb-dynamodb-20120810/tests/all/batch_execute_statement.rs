// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchExecuteStatement`: several PartiQL statements in one
//! call, each served on its own against the SAME item store the classic item API and
//! `ExecuteStatement` read and write.
//!
//! The rules these tables assert are the batch's own, and each table spans what the
//! rule generalises over rather than one recorded statement:
//!
//! 1. A statement the emulator cannot serve is reported in ITS OWN slot — by the
//!    `BatchStatementErrorCodeEnum` code for what was wrong with it — while the slots
//!    beside it are served and the call still answers HTTP 200. The read table below
//!    spans the ways one statement of a batch can fail (a key not pinned, a source that
//!    is not the table, a table that is not there, text that does not parse) beside the
//!    ways one can succeed (literal and `?`-marker keys, a projection, a key holding no
//!    item).
//! 2. A batch read addresses ONE item by an equality on every key attribute, so it
//!    reports at most one item.
//! 3. A batch is refused as a WHOLE — and applies nothing — when the request itself is
//!    wrong: reads mixed with writes, or one item addressed by two writes.
//! 4. A write statement's effect is visible to every later read, classic or PartiQL.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    BURST_CAPACITY_WINDOW, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, ok};
use crate::table_items::create_active_table;

/// Run one batch that must answer HTTP 200 and return its parsed body.
async fn batch(router: &MultiServiceRouter, body: Value) -> Value {
    ok(router, "BatchExecuteStatement", body).await
}

/// The `Statements` entry one statement of a batch is sent as.
fn statement(text: &str) -> Value {
    json!({"Statement": text})
}

/// The slot at `at` of a batch's `Responses`.
fn slot(answered: &Value, at: usize) -> Value {
    answered["Responses"][at].clone()
}

/// The `Error.Code` a slot carries, or `None` when it carries no error.
fn slot_error_code(slot: &Value) -> Option<String> {
    slot["Error"]["Code"].as_str().map(ToString::to_string)
}

/// One row of the read table: the statement, the parameters it names, the item its slot
/// must report, and the `Error.Code` its slot must carry.
struct ReadRow {
    case: &'static str,
    statement: &'static str,
    parameters: Option<Value>,
    item: Option<Value>,
    code: Option<&'static str>,
}

/// The rows differ in how the statement addresses its item (literals, `?` markers, a
/// projection, a further predicate), in whether the addressed key holds an item at all,
/// and in HOW each failing row fails: a sort key not pinned, a key pinned by an
/// inequality, a source that is a secondary index, an order asked of a one-item read, a
/// table that is not there, and text that does not parse.
fn read_table() -> Vec<ReadRow> {
    vec![
        ReadRow {
            case: "every key attribute pinned by a literal",
            statement: "SELECT * FROM items WHERE pk = 'a' AND sk = 1",
            parameters: None,
            item: Some(json!({
                "pk": {"S": "a"},
                "sk": {"N": "1"},
                "owner": {"S": "x"},
                "note": {"S": "first"},
            })),
            code: None,
        },
        ReadRow {
            case: "every key attribute pinned by a ? marker",
            statement: "SELECT * FROM items WHERE pk = ? AND sk = ?",
            parameters: Some(json!([{"S": "b"}, {"N": "7"}])),
            item: Some(json!({
                "pk": {"S": "b"},
                "sk": {"N": "7"},
                "note": {"S": "third"},
            })),
            code: None,
        },
        ReadRow {
            case: "a projection reports only the attributes it names",
            statement: "SELECT note FROM items WHERE pk = 'b' AND sk = 7",
            parameters: None,
            item: Some(json!({"note": {"S": "third"}})),
            code: None,
        },
        ReadRow {
            case: "the addressed key holds no item",
            statement: "SELECT * FROM items WHERE pk = 'a' AND sk = 99",
            parameters: None,
            item: None,
            code: None,
        },
        ReadRow {
            case: "a further predicate the stored item does not satisfy",
            statement: "SELECT * FROM items WHERE pk = 'a' AND sk = 1 AND note = 'other'",
            parameters: None,
            item: None,
            code: None,
        },
        ReadRow {
            case: "the sort key is not pinned",
            statement: "SELECT * FROM items WHERE pk = 'a'",
            parameters: None,
            item: None,
            code: Some("ValidationError"),
        },
        ReadRow {
            case: "the sort key is pinned by an inequality",
            statement: "SELECT * FROM items WHERE pk = 'a' AND sk > 0",
            parameters: None,
            item: None,
            code: Some("ValidationError"),
        },
        ReadRow {
            case: "the source is a secondary index",
            statement: "SELECT * FROM \"items\".\"by-owner\" WHERE owner = 'x'",
            parameters: None,
            item: None,
            code: Some("ValidationError"),
        },
        ReadRow {
            case: "an order is asked of a read that reports one item",
            statement: "SELECT * FROM items WHERE pk = 'a' AND sk = 1 ORDER BY sk",
            parameters: None,
            item: None,
            code: Some("ValidationError"),
        },
        ReadRow {
            case: "the table is not there",
            statement: "SELECT * FROM absent WHERE pk = 'a' AND sk = 1",
            parameters: None,
            item: None,
            code: Some("ResourceNotFound"),
        },
        ReadRow {
            case: "the statement does not parse",
            statement: "SELEKT * FROM items",
            parameters: None,
            item: None,
            code: Some("ValidationError"),
        },
    ]
}

/// The read table's table: one partition and one sort key, plus a global secondary
/// index for the index-qualified row to address.
async fn create_read_table(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": "items",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "owner", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": "by-owner",
                "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    for item in [
        json!({"pk": {"S": "a"}, "sk": {"N": "1"}, "owner": {"S": "x"}, "note": {"S": "first"}}),
        json!({"pk": {"S": "a"}, "sk": {"N": "2"}, "owner": {"S": "y"}}),
        json!({"pk": {"S": "b"}, "sk": {"N": "7"}, "note": {"S": "third"}}),
    ] {
        ok(router, "PutItem", json!({"TableName": "items", "Item": item})).await;
    }
}

#[tokio::test]
async fn every_read_statement_reports_into_its_own_slot() {
    let router = emulator();
    create_read_table(&router).await;

    let rows = read_table();
    let statements: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut sent = statement(row.statement);
            if let Some(parameters) = row.parameters.clone() {
                sent["Parameters"] = parameters;
            }
            sent
        })
        .collect();
    let (status, answered) = call(
        &router,
        "BatchExecuteStatement",
        json!({"Statements": statements}),
    )
    .await;
    // "A HTTP 200 response does not mean that all statements in the
    // BatchExecuteStatement succeeded": the batch answers 200 even though rows of it
    // failed.
    assert_eq!(
        status, 200,
        "case batch of {} read statements: expected HTTP 200, got {status} with {answered}",
        rows.len()
    );
    let reported = answered["Responses"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        reported.len(),
        rows.len(),
        "case slot count: one slot per statement, got {answered}"
    );

    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let answered_slot = slot(&answered, at);
        assert_eq!(
            slot_error_code(&answered_slot).as_deref(),
            row.code,
            "case {case}: expected Error.Code {:?}, got {answered_slot}",
            row.code
        );
        let item = answered_slot.get("Item").cloned();
        assert_eq!(
            item, row.item,
            "case {case}: expected Item {:?}, got {answered_slot}",
            row.item
        );
        // A slot reports the table its statement addressed — whether the statement ran
        // or failed — so a batch spread over several tables can be attributed slot by
        // slot. Only a statement that never resolved a table names none.
        let expected_table = match row.case {
            "the statement does not parse" => None,
            "the table is not there" => Some("absent"),
            _ => Some("items"),
        };
        assert_eq!(
            answered_slot.get("TableName").and_then(Value::as_str),
            expected_table,
            "case {case}: expected TableName {expected_table:?}, got {answered_slot}"
        );
    }
}

/// One row of the write table: the statement, the `Error.Code` its slot must carry, and
/// the item a later `GetItem` of `key` must report.
struct WriteRow {
    case: &'static str,
    statement: &'static str,
    code: Option<&'static str>,
    key: &'static str,
    stored: Option<Value>,
}

/// The rows differ in which write the statement is (`INSERT`, `UPDATE`, `DELETE`), in
/// whether the item it addresses is already stored, and in what each failing row fails
/// on — a primary key already held, and a `WHERE` clause no stored item satisfies.
fn write_table() -> Vec<WriteRow> {
    vec![
        WriteRow {
            case: "INSERT of a key nothing holds",
            statement: "INSERT INTO stock VALUE {'pk': 'fresh', 'count': 5}",
            code: None,
            key: "fresh",
            stored: Some(json!({"pk": {"S": "fresh"}, "count": {"N": "5"}})),
        },
        WriteRow {
            case: "INSERT of a key already held",
            statement: "INSERT INTO stock VALUE {'pk': 'held', 'count': 99}",
            code: Some("DuplicateItem"),
            key: "held",
            stored: Some(json!({"pk": {"S": "held"}, "count": {"N": "1"}})),
        },
        WriteRow {
            case: "UPDATE of a stored item",
            statement: "UPDATE stock SET count = 9 WHERE pk = 'kept'",
            code: None,
            key: "kept",
            stored: Some(json!({"pk": {"S": "kept"}, "count": {"N": "9"}})),
        },
        WriteRow {
            case: "UPDATE of a key nothing holds",
            statement: "UPDATE stock SET count = 1 WHERE pk = 'ghost'",
            code: Some("ConditionalCheckFailed"),
            key: "ghost",
            stored: None,
        },
        WriteRow {
            case: "DELETE of a stored item",
            statement: "DELETE FROM stock WHERE pk = 'doomed'",
            code: None,
            key: "doomed",
            stored: None,
        },
        WriteRow {
            case: "DELETE of a key nothing holds",
            statement: "DELETE FROM stock WHERE pk = 'vanished'",
            code: Some("ConditionalCheckFailed"),
            key: "vanished",
            stored: None,
        },
        WriteRow {
            case: "UPDATE whose REMOVE clause drops an attribute",
            statement: "UPDATE stock REMOVE note WHERE pk = 'noted'",
            code: None,
            key: "noted",
            stored: Some(json!({"pk": {"S": "noted"}, "count": {"N": "3"}})),
        },
    ]
}

#[tokio::test]
async fn every_write_statement_lands_in_the_store_and_reports_into_its_own_slot() {
    let router = emulator();
    create_active_table(&router, "stock", &[("pk", "HASH", "S")]).await;
    for item in [
        json!({"pk": {"S": "held"}, "count": {"N": "1"}}),
        json!({"pk": {"S": "kept"}, "count": {"N": "0"}}),
        json!({"pk": {"S": "doomed"}, "count": {"N": "2"}}),
        json!({"pk": {"S": "noted"}, "count": {"N": "3"}, "note": {"S": "drop me"}}),
    ] {
        ok(&router, "PutItem", json!({"TableName": "stock", "Item": item})).await;
    }

    let rows = write_table();
    let answered = batch(
        &router,
        json!({
            "Statements": rows
                .iter()
                .map(|row| statement(row.statement))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;

    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let answered_slot = slot(&answered, at);
        assert_eq!(
            slot_error_code(&answered_slot).as_deref(),
            row.code,
            "case {case}: expected Error.Code {:?}, got {answered_slot}",
            row.code
        );
        // The round trip: what the batch wrote is what a later classic read reports.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "stock", "Key": {"pk": {"S": row.key}}}),
        )
        .await;
        assert_eq!(
            read.get("Item").cloned(),
            row.stored,
            "case {case}: expected a later GetItem to report {:?}, got {read}",
            row.stored
        );
    }
}

#[tokio::test]
async fn a_write_reports_what_its_returning_clause_asks_for() {
    let router = emulator();
    create_active_table(&router, "stock", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "stock", "Item": {"pk": {"S": "one"}, "count": {"N": "1"}}}),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "stock", "Item": {"pk": {"S": "two"}, "count": {"N": "2"}}}),
    )
    .await;

    let answered = batch(
        &router,
        json!({"Statements": [
            statement("UPDATE stock SET count = 11 WHERE pk = 'one' RETURNING ALL OLD *"),
            statement("UPDATE stock SET count = 22 WHERE pk = 'two' RETURNING ALL NEW *"),
        ]}),
    )
    .await;
    assert_eq!(
        slot(&answered, 0)["Item"],
        json!({"pk": {"S": "one"}, "count": {"N": "1"}}),
        "case RETURNING ALL OLD: the item as it stood before the write, got {answered}"
    );
    assert_eq!(
        slot(&answered, 1)["Item"],
        json!({"pk": {"S": "two"}, "count": {"N": "22"}}),
        "case RETURNING ALL NEW: the item as it stands after the write, got {answered}"
    );
}

#[tokio::test]
async fn a_condition_failure_carries_the_stored_item_only_when_all_old_was_asked_for() {
    let router = emulator();
    create_active_table(&router, "stock", &[("pk", "HASH", "S")]).await;
    for key in ["asked", "unasked"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "stock", "Item": {"pk": {"S": key}, "count": {"N": "1"}}}),
        )
        .await;
    }

    let answered = batch(
        &router,
        json!({"Statements": [
            {
                "Statement": "UPDATE stock SET count = 2 WHERE pk = 'asked' AND count = 100",
                "ReturnValuesOnConditionCheckFailure": "ALL_OLD",
            },
            {
                "Statement": "UPDATE stock SET count = 2 WHERE pk = 'unasked' AND count = 100",
                "ReturnValuesOnConditionCheckFailure": "NONE",
            },
        ]}),
    )
    .await;
    for (at, case, expected) in [
        (
            0usize,
            "ALL_OLD",
            Some(json!({"pk": {"S": "asked"}, "count": {"N": "1"}})),
        ),
        (1usize, "NONE", None),
    ] {
        let answered_slot = slot(&answered, at);
        assert_eq!(
            slot_error_code(&answered_slot).as_deref(),
            Some("ConditionalCheckFailed"),
            "case {case}: a predicate the stored item fails is the condition failure, got \
             {answered_slot}"
        );
        assert_eq!(
            answered_slot["Error"].get("Item").cloned(),
            expected,
            "case {case}: expected Error.Item {expected:?}, got {answered_slot}"
        );
    }
}

/// One row of the whole-batch refusal table: the statements it sends, and the item key
/// that must still be absent afterwards because nothing of a refused batch is applied.
struct RefusedBatch {
    case: &'static str,
    statements: Vec<&'static str>,
    unwritten: &'static str,
}

#[tokio::test]
async fn a_batch_the_request_itself_is_wrong_about_is_refused_whole() {
    let rows = vec![
        RefusedBatch {
            case: "reads mixed with writes",
            statements: vec![
                "SELECT * FROM stock WHERE pk = 'held'",
                "INSERT INTO stock VALUE {'pk': 'mixed', 'count': 1}",
            ],
            unwritten: "mixed",
        },
        RefusedBatch {
            case: "two writes addressing one item",
            statements: vec![
                "INSERT INTO stock VALUE {'pk': 'twice', 'count': 1}",
                "UPDATE stock SET count = 2 WHERE pk = 'twice'",
            ],
            unwritten: "twice",
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator();
        create_active_table(&router, "stock", &[("pk", "HASH", "S")]).await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": "stock", "Item": {"pk": {"S": "held"}, "count": {"N": "1"}}}),
        )
        .await;

        let (status, refused) = call(
            &router,
            "BatchExecuteStatement",
            json!({
                "Statements": row
                    .statements
                    .iter()
                    .map(|text| statement(text))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused}"
        );
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "stock", "Key": {"pk": {"S": row.unwritten}}}),
        )
        .await;
        assert!(
            read.get("Item").is_none(),
            "case {case}: a refused batch applies none of its statements, got {read}"
        );
    }
}

#[tokio::test]
async fn consumed_capacity_is_reported_per_statement_as_the_request_asked() {
    let router = emulator();
    create_read_table(&router).await;

    // Two statements that read an item and one the emulator refuses in its own slot,
    // which consumed nothing because it reached no table.
    let statements = json!([
        statement("SELECT * FROM items WHERE pk = 'a' AND sk = 1"),
        statement("SELECT * FROM items WHERE pk = 'b' AND sk = 7"),
        statement("SELEKT * FROM items"),
    ]);

    for (case, requested, entries, detailed) in [
        ("no ReturnConsumedCapacity", Value::Null, 0usize, false),
        ("NONE", json!("NONE"), 0, false),
        ("TOTAL", json!("TOTAL"), 2, false),
        ("INDEXES", json!("INDEXES"), 2, true),
    ] {
        let mut body = json!({"Statements": statements.clone()});
        if !requested.is_null() {
            body["ReturnConsumedCapacity"] = requested;
        }
        let answered = batch(&router, body).await;
        let reported = answered["ConsumedCapacity"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            entries,
            "case {case}: expected {entries} ConsumedCapacity entries, one per statement that \
             consumed capacity, got {answered}"
        );
        for entry in &reported {
            assert_eq!(
                entry["TableName"],
                json!("items"),
                "case {case}: each entry names the table its statement read, got {entry}"
            );
            assert!(
                entry["CapacityUnits"].as_f64().unwrap_or_default() > 0.0,
                "case {case}: each entry reports the units its statement consumed, got {entry}"
            );
            assert_eq!(
                entry.get("Table").is_some(),
                detailed,
                "case {case}: the per-table breakdown is reported only for INDEXES, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_write_is_visible_to_a_later_batch_read() {
    let router = emulator();
    create_active_table(&router, "stock", &[("pk", "HASH", "S")]).await;

    batch(
        &router,
        json!({"Statements": [
            statement("INSERT INTO stock VALUE {'pk': 'later', 'count': 4}"),
        ]}),
    )
    .await;
    let read = batch(
        &router,
        json!({"Statements": [statement("SELECT * FROM stock WHERE pk = 'later'")]}),
    )
    .await;
    assert_eq!(
        slot(&read, 0)["Item"],
        json!({"pk": {"S": "later"}, "count": {"N": "4"}}),
        "case batch write then batch read: a statement's write is visible to every later \
         read, got {read}"
    );

    // And to the single-statement surface, which reads the same store.
    let selected = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM stock WHERE pk = 'later'"}),
    )
    .await;
    assert_eq!(
        selected["Items"],
        json!([{"pk": {"S": "later"}, "count": {"N": "4"}}]),
        "case batch write then ExecuteStatement: one store behind both surfaces, got {selected}"
    );
}

/// A table with one partition key, in the billing mode `billing` names, waited out to
/// `ACTIVE`.
async fn create_billed_table(router: &MultiServiceRouter, name: &str, billing: &Value) {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    for (member, value) in billing.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    ok(router, "CreateTable", body).await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
}

#[tokio::test]
async fn a_write_past_the_capacity_its_table_is_provisioned_for_is_throttled() {
    // The rows differ in the capacity the table is provisioned for: a table on-demand
    // is provisioned for no capacity to exceed, so the SAME load is served in full,
    // while a provisioned one serves its units per second plus the documented burst.
    let rows = [
        (
            "PROVISIONED at one write unit per second",
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
            true,
        ),
        (
            "PAY_PER_REQUEST",
            json!({"BillingMode": "PAY_PER_REQUEST"}),
            false,
        ),
    ];
    // One write unit covers a one-item write of up to 1 KB, so a table provisioned for
    // one unit per second covers about this many of these inserts before its budget is
    // spent.
    let burst_units = usize::try_from(BURST_CAPACITY_WINDOW.as_secs()).unwrap_or(usize::MAX);
    let per_batch = 25usize;
    let batches = 20usize;

    for (case, billing, throttles) in rows {
        let router = emulator();
        create_billed_table(&router, "metered", &billing).await;

        let mut served = 0usize;
        let mut codes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut refused_key: Option<String> = None;
        for batch_at in 0..batches {
            let statements: Vec<Value> = (0..per_batch)
                .map(|slot_at| {
                    let key = format!("k{}", batch_at * per_batch + slot_at);
                    json!({"Statement": format!("INSERT INTO metered VALUE {{'pk': '{key}'}}")})
                })
                .collect();
            let answered = batch(&router, json!({"Statements": statements})).await;
            for slot_at in 0..per_batch {
                let answered_slot = slot(&answered, slot_at);
                match slot_error_code(&answered_slot) {
                    Some(code) => {
                        codes.insert(code);
                        refused_key
                            .get_or_insert_with(|| format!("k{}", batch_at * per_batch + slot_at));
                    }
                    None => served += 1,
                }
            }
        }

        if throttles {
            assert_eq!(
                codes.iter().map(String::as_str).collect::<Vec<&str>>(),
                vec!["ProvisionedThroughputExceeded"],
                "case {case}: a write past the provisioned capacity is throttled in its own \
                 slot and nothing else fails, got {codes:?} over {served} served writes"
            );
            // The burst is what the table had not consumed, so the load is served up to
            // it and refused past it.
            assert!(
                served >= burst_units && served < batches * per_batch,
                "case {case}: expected the burst ({burst_units} units) to be served and the \
                 rest refused, got {served} of {} served",
                batches * per_batch
            );
            let refused_key = refused_key.expect("a throttled slot was observed");
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": "metered", "Key": {"pk": {"S": refused_key}}}),
            )
            .await;
            assert!(
                read.get("Item").is_none(),
                "case {case}: a throttled statement writes nothing, got {read}"
            );
        } else {
            assert!(
                codes.is_empty() && served == batches * per_batch,
                "case {case}: a table provisioned for no capacity throttles nothing, got \
                 {codes:?} over {served} served writes"
            );
        }
    }
}
