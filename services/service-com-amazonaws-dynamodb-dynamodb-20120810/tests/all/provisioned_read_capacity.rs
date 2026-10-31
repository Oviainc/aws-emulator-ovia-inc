// SPDX-License-Identifier: CC0-1.0

//! The read capacity a PROVISIONED table is provisioned for bounds the reads it serves.
//!
//! RULE: "provisioned throughput is the maximum amount of capacity that an application can
//! consume from a table or index" per second (DynamoDB Developer Guide, "Read/write
//! capacity mode"), in reads as in writes, with up to 300 seconds of unused capacity kept
//! for a burst ("Using burst capacity effectively"). A read past what the table's read
//! capacity can cover is refused with `ProvisionedThroughputExceededException` (HTTP 400, a
//! modelled error of every data-plane read) carrying the `ThrottlingReason` of the ceiling
//! that bound, instead of being served; a table on-demand (`PAY_PER_REQUEST`) is
//! provisioned for no capacity to exceed, so no read of one is refused for capacity.
//!
//! The rows span the surfaces the rule holds for — `GetItem`, `Query`, `Scan`,
//! `BatchGetItem`, `TransactGetItems` and the PartiQL `SELECT` — and each is driven against
//! BOTH capacity modes, because what the rule turns on is the capacity the table is
//! provisioned for and not the call.
//!
//! A throttled read changes nothing a later caller sees: once the budget has refilled, the
//! same read is served and reports the same item, which the last case here pins.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, ok};

/// The read capacity the provisioned table under test is given: the smallest capacity a
/// table can carry, so the documented burst is spent in a handful of reads.
const PROVISIONED_READ_UNITS: i64 = 1;

/// The bytes one read capacity unit covers for a STRONGLY consistent read: "one read
/// request unit represents one strongly consistent read request ... for an item up to 4 KB
/// in size" (DynamoDB Developer Guide, "Read/write capacity mode"). An item of
/// [`ITEM_BYTES`] therefore costs about 100 units, so a 1-RCU table's whole 300-second
/// burst is three such reads.
const ITEM_BYTES: usize = 399 * 1024;

/// How many reads a loop drives before it gives up on ever being throttled: far past the
/// burst a 1-RCU table can hold, so a table that throttles nothing is reported as such
/// rather than looping.
const MOST_READS: usize = 24;

/// The table for a row, created in `mode` with an item big enough that each read of it
/// costs about a hundred read capacity units.
async fn seeded_table(router: &MultiServiceRouter, name: &str, mode: &str) {
    let mut create = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        "BillingMode": mode,
    });
    if mode == "PROVISIONED" {
        create["ProvisionedThroughput"] = json!({
            "ReadCapacityUnits": PROVISIONED_READ_UNITS,
            // Write capacity enough that seeding the item is never the refusal under test.
            "WriteCapacityUnits": 4000,
        });
    }
    ok(router, "CreateTable", create).await;
    describe_when_active(router, name, name).await;
    ok(
        router,
        "PutItem",
        json!({
            "TableName": name,
            "Item": {"pk": {"S": "k"}, "blob": {"S": "x".repeat(ITEM_BYTES)}},
        }),
    )
    .await;
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The read each row drives, as a request body against `table`.
fn request(operation: &str, table: &str) -> Value {
    match operation {
        "GetItem" => json!({
            "TableName": table,
            "Key": {"pk": {"S": "k"}},
            "ConsistentRead": true,
        }),
        "Query" => json!({
            "TableName": table,
            "KeyConditionExpression": "pk = :p",
            "ExpressionAttributeValues": {":p": {"S": "k"}},
            "ConsistentRead": true,
        }),
        "Scan" => json!({"TableName": table, "ConsistentRead": true}),
        "BatchGetItem" => json!({
            "RequestItems": {table: {"Keys": [{"pk": {"S": "k"}}], "ConsistentRead": true}},
        }),
        "TransactGetItems" => json!({
            "TransactItems": [{"Get": {"TableName": table, "Key": {"pk": {"S": "k"}}}}],
        }),
        "ExecuteStatement" => json!({
            "Statement": format!("SELECT * FROM \"{table}\" WHERE pk = 'k'"),
            "ConsistentRead": true,
        }),
        other => panic!("no request shape for {other}"),
    }
}

/// Drive `operation` against `table` until it is refused, or until [`MOST_READS`] reads
/// have been served. Answers the refusal body, or `None` when every read was served.
async fn read_until_throttled(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    table: &str,
) -> Option<Value> {
    for attempt in 0..MOST_READS {
        let (status, body) = call(router, operation, request(operation, table)).await;
        if status == 200 {
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: read {attempt} was refused with an unexpected status, got \
             {status} {body}"
        );
        return Some(body);
    }
    None
}

#[tokio::test]
async fn a_read_past_the_provisioned_read_capacity_is_refused() {
    let router = emulator();
    // Each row gets its OWN table: a budget is per table, and a row that spent one must
    // not decide the next row's.
    let operations = [
        "GetItem",
        "Query",
        "Scan",
        "BatchGetItem",
        "TransactGetItems",
        "ExecuteStatement",
    ];

    for operation in operations {
        let provisioned = format!("Provisioned{operation}");
        seeded_table(&router, &provisioned, "PROVISIONED").await;
        let case = format!("{operation} against a {PROVISIONED_READ_UNITS}-RCU table");
        let refused = read_until_throttled(&router, &case, operation, &provisioned)
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a ProvisionedThroughputExceededException within \
                     {MOST_READS} reads of a {ITEM_BYTES}-byte item, every read was served"
                )
            });
        assert_eq!(
            error_code(&refused),
            "ProvisionedThroughputExceededException",
            "case {case:?}: expected the modelled throughput refusal, got {refused}"
        );
        let message = refused["message"]
            .as_str()
            .or_else(|| refused["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("provisioned throughput for the table was exceeded"),
            "case {case:?}: expected the sentence DynamoDB states for a table past its \
             provisioning, got {refused}"
        );
        // "The throttling reason follows a specific format: ResourceType + OperationType +
        // LimitType" (`ThrottlingReason$reason`), naming the ARN of the table that
        // experienced the event.
        assert_eq!(
            refused["ThrottlingReasons"][0]["reason"],
            json!("TableReadProvisionedThroughputExceeded"),
            "case {case:?}: expected the read ceiling's own reason, got {refused}"
        );
        assert_eq!(
            refused["ThrottlingReasons"][0]["resource"],
            json!(format!(
                "arn:aws:dynamodb:us-east-1:000000000000:table/{provisioned}"
            )),
            "case {case:?}: expected the throttled table's ARN, got {refused}"
        );

        // The same load against an ON-DEMAND table is never refused for capacity: it is
        // provisioned for none to exceed.
        let on_demand = format!("OnDemand{operation}");
        seeded_table(&router, &on_demand, "PAY_PER_REQUEST").await;
        let case = format!("{operation} against an on-demand table");
        let throttled = read_until_throttled(&router, &case, operation, &on_demand).await;
        assert!(
            throttled.is_none(),
            "case {case:?}: an on-demand table has no read capacity to exceed, got {:?}",
            throttled
        );
    }
}

#[tokio::test]
async fn a_refused_transactional_read_debits_no_tables_budget() {
    let router = emulator();
    // Two 1-RCU tables: one with its whole burst, one with none left.
    seeded_table(&router, "TxFull", "PROVISIONED").await;
    seeded_table(&router, "TxSpent", "PROVISIONED").await;
    // A second item in the table that keeps its burst, so a later read can ask for more
    // than the refused transaction would have taken from it.
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": "TxFull",
            "Item": {"pk": {"S": "k2"}, "blob": {"S": "x".repeat(ITEM_BYTES)}},
        }),
    )
    .await;
    read_until_throttled(&router, "spending TxSpent", "GetItem", "TxSpent")
        .await
        .expect("the burst of a 1-RCU table is spent within the bounded loop");

    // One transaction over both: "the entire transaction is rejected" because one of its
    // tables has no capacity left, and the refusal names that table.
    let (status, refused) = call(
        &router,
        "TransactGetItems",
        json!({
            "TransactItems": [
                {"Get": {"TableName": "TxFull", "Key": {"pk": {"S": "k"}}}},
                {"Get": {"TableName": "TxSpent", "Key": {"pk": {"S": "k"}}}},
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case a transaction one of whose tables is out of capacity: expected the modelled \
         throughput refusal, got {status} {refused}"
    );
    assert_eq!(
        refused["ThrottlingReasons"][0]["resource"],
        json!("arn:aws:dynamodb:us-east-1:000000000000:table/TxSpent"),
        "case a transaction one of whose tables is out of capacity: the refusal names the \
         table that had none, got {refused}"
    );

    // The OTHER table was not debited: a transaction that did nothing consumed nothing
    // anywhere, so a read asking for twice what the refused transaction would have taken
    // from it is still served.
    let (status, scanned) = call(
        &router,
        "Scan",
        json!({"TableName": "TxFull", "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        (status, scanned["Count"].clone()),
        (200, json!(2)),
        "case the table the refused transaction did not reach: its whole burst is still \
         there, got {status} {scanned}"
    );
}

#[tokio::test]
async fn a_batch_get_hands_back_exactly_the_keys_it_had_no_capacity_for() {
    let router = emulator();
    seeded_table(&router, "Residue", "PROVISIONED").await;
    // Four items of about a hundred read capacity units each, against a burst of three
    // hundred: a batch over all four can read three of them and no more.
    for key in ["k2", "k3", "k4"] {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": "Residue",
                "Item": {"pk": {"S": key}, "blob": {"S": "x".repeat(ITEM_BYTES)}},
            }),
        )
        .await;
    }

    let requested = json!({
        "RequestItems": {
            "Residue": {
                "Keys": [
                    {"pk": {"S": "k"}},
                    {"pk": {"S": "k2"}},
                    {"pk": {"S": "k3"}},
                    {"pk": {"S": "k4"}},
                ],
                "ConsistentRead": true,
            },
        },
    });
    let answered = ok(&router, "BatchGetItem", requested.clone()).await;
    // "A map of tables and their respective keys that were not processed": the keys the
    // table had no capacity left for, in the form they were sent, so the map can be
    // handed straight to a later BatchGetItem.
    let residue = answered["UnprocessedKeys"]["Residue"]["Keys"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let read = answered["Responses"]["Residue"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        (read.len(), residue.len()),
        (3, 1),
        "case a batch past the table's burst: expected the keys that fit answered and the \
         rest handed back, got {answered}"
    );
    assert_eq!(
        residue[0], json!({"pk": {"S": "k4"}}),
        "case a batch past the table's burst: the residue is exactly the keys not read, in \
         the spelling the request sent, got {answered}"
    );
    assert_eq!(
        answered["UnprocessedKeys"]["Residue"]["ConsistentRead"],
        json!(true),
        "case a batch past the table's burst: the residue carries the read options it was \
         sent with, got {answered}"
    );

    // The residue DRAINS: resubmitted once the table has the capacity, it reports the
    // item and hands nothing further back — which is what lets a client's retry loop
    // terminate.
    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Residue",
            "ProvisionedThroughput": {"ReadCapacityUnits": 4000, "WriteCapacityUnits": 4000},
        }),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let drained = ok(
        &router,
        "BatchGetItem",
        json!({"RequestItems": answered["UnprocessedKeys"].clone()}),
    )
    .await;
    assert_eq!(
        (
            drained["Responses"]["Residue"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            drained["UnprocessedKeys"]
                .as_object()
                .map(serde_json::Map::len)
                .unwrap_or_default(),
        ),
        (1, 0),
        "case the residue resubmitted: expected the item and an empty residue, got {drained}"
    );
    assert_eq!(
        drained["Responses"]["Residue"][0]["pk"],
        json!({"S": "k4"}),
        "case the residue resubmitted: expected the key the first call handed back, got \
         {drained}"
    );
}

#[tokio::test]
async fn a_batch_statement_read_reports_the_throttle_in_its_own_slot() {
    let router = emulator();
    seeded_table(&router, "SlotThrottle", "PROVISIONED").await;

    // Spend the burst through the single-statement surface, so what is left to assert is
    // only how the BATCH surface reports a read the capacity cannot cover.
    read_until_throttled(&router, "spending the burst", "GetItem", "SlotThrottle")
        .await
        .expect("the burst of a 1-RCU table is spent within the bounded loop");

    // "The entire batch must consist of either read statements or write statements": one
    // read statement, which the table has no capacity left for. A batch reports a
    // per-statement refusal in that statement's own slot, with HTTP 200 for the call.
    let (status, answered) = call(
        &router,
        "BatchExecuteStatement",
        json!({
            "Statements": [{
                "Statement": "SELECT * FROM \"SlotThrottle\" WHERE pk = 'k'",
                "ConsistentRead": true,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a throttled batch statement: the CALL is answered, got {status} {answered}"
    );
    assert_eq!(
        answered["Responses"][0]["Error"]["Code"],
        json!("ProvisionedThroughputExceeded"),
        "case a throttled batch statement: expected the throttling code in the slot, got \
         {answered}"
    );
    // A PartiQL read TRANSACTION is bounded by the same capacity, and like
    // `TransactGetItems` it is refused whole.
    let (status, refused) = call(
        &router,
        "ExecuteTransaction",
        json!({
            "TransactStatements": [{
                "Statement": "SELECT * FROM \"SlotThrottle\" WHERE pk = 'k'",
            }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case a read transaction against a table out of capacity: expected the modelled \
         throughput refusal for the whole call, got {status} {refused}"
    );
}

#[tokio::test]
async fn a_throttled_read_changes_nothing_a_later_read_sees() {
    let router = emulator();
    seeded_table(&router, "Refills", "PROVISIONED").await;

    // Spend the burst.
    let refused = read_until_throttled(&router, "spending the burst", "GetItem", "Refills")
        .await
        .expect("the burst of a 1-RCU table is spent within the bounded loop");
    assert_eq!(
        error_code(&refused),
        "ProvisionedThroughputExceededException",
        "case spending the burst: expected the modelled throughput refusal, got {refused}"
    );

    // A read the budget cannot cover takes nothing from it, and changes nothing the table
    // HOLDS. Raising the capacity is what lets the same read through — the budget is
    // measured against the capacity in force, not the one the table was created with — and
    // what it then reports is the item that was there all along.
    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Refills",
            "ProvisionedThroughput": {"ReadCapacityUnits": 4000, "WriteCapacityUnits": 4000},
        }),
    )
    .await;
    // Capacity is a RATE, so the raised capacity has to run for a moment before it covers
    // a read of this size: a hundred units at four thousand a second is a few
    // milliseconds' worth.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (status, body) = call(&router, "GetItem", request("GetItem", "Refills")).await;
    assert_eq!(
        (status, body["Item"]["pk"].clone()),
        (200, json!({"S": "k"})),
        "case capacity raised: the read the new capacity covers reports the item the \
         throttled read did not change, got {status} {body}"
    );
}
