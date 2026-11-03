// SPDX-License-Identifier: CC0-1.0

//! The ARM every `ConsumedCapacity` reports its figures in, beside the aggregate.
//!
//! RULE: `ConsumedCapacity` carries an aggregate (`CapacityUnits`, "the total number of
//! capacity units consumed by the operation") and one figure per KIND —
//! `ReadCapacityUnits` is "the total number of read capacity units consumed by the
//! operation" and `WriteCapacityUnits` the write equivalent — and so does every `Capacity`
//! line of the per-table and per-index breakdown. A data-plane operation spends capacity of
//! ONE kind, so the figure of that kind equals the aggregate and the other kind's is absent:
//! a read reports the read arm, a write reports the write arm.
//!
//! The one place the kind is not the operation's own direction is a `TransactWriteItems`
//! replay under a `ClientRequestToken`: "the initial TransactWriteItems call returns the
//! amount of write capacity units consumed in making the changes. Subsequent
//! TransactWriteItems calls with the same client token return the number of READ capacity
//! units consumed in reading the item"
//! (`smithy_query(query="doc",shape="TransactWriteItemsInput",member="ClientRequestToken")`)
//! — the replay applies nothing, so it is billed as the read it was, which is how a caller
//! tells which of several identical idempotent calls committed.
//!
//! The rows span what the rule generalises over: both kinds, every data-plane surface that
//! reports capacity (the item API, the batch and transactional APIs and PartiQL), the
//! single-table and the per-table-list shapes of the member, and both detail levels
//! (`TOTAL` and `INDEXES`, which adds the `Table` and per-index lines).

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, ok};

/// Which kind of capacity an operation's figures must be reported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Read,
    Write,
}

impl Kind {
    /// The member this kind's figure is carried in, and the one that must be absent.
    fn arms(self) -> (&'static str, &'static str) {
        match self {
            Self::Read => ("ReadCapacityUnits", "WriteCapacityUnits"),
            Self::Write => ("WriteCapacityUnits", "ReadCapacityUnits"),
        }
    }
}

/// Assert the rule over ONE `ConsumedCapacity` object: the aggregate is positive, the
/// kind's arm equals it, the other kind's arm is absent, and every `Capacity` line of the
/// breakdown reports its own figure in the same arm.
fn assert_arms(case: &str, kind: Kind, consumed: &Value) {
    let (reported, absent) = kind.arms();
    let aggregate = consumed["CapacityUnits"].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a CapacityUnits aggregate, got {consumed}")
    });
    assert!(
        aggregate > 0.0,
        "case {case:?}: an operation that did work consumed capacity, got {consumed}"
    );
    assert_eq!(
        consumed[reported].as_f64(),
        Some(aggregate),
        "case {case:?}: expected {reported} to equal CapacityUnits ({aggregate}), got {consumed}"
    );
    assert_eq!(
        consumed.get(absent),
        None,
        "case {case:?}: an operation consumes capacity of one kind, so {absent} is absent, \
         got {consumed}"
    );
    // The per-line members a `ConsumedCapacity` can carry beside the aggregate; `Table`
    // is the one this check covers.
    let lines = ["Table"];
    for line in lines {
        if let Some(capacity) = consumed.get(line) {
            assert_eq!(
                capacity[reported].as_f64(),
                capacity["CapacityUnits"].as_f64(),
                "case {case:?}: the {line} line reports its figure in {reported} too, got \
                 {consumed}"
            );
            assert_eq!(
                capacity.get(absent),
                None,
                "case {case:?}: the {line} line carries no {absent}, got {consumed}"
            );
        }
    }
    for map in ["LocalSecondaryIndexes", "GlobalSecondaryIndexes"] {
        let Some(indexes) = consumed.get(map).and_then(Value::as_object) else {
            continue;
        };
        for (index, capacity) in indexes {
            assert_eq!(
                capacity[reported].as_f64(),
                capacity["CapacityUnits"].as_f64(),
                "case {case:?}: the {map} line for {index} reports its figure in \
                 {reported} too, got {consumed}"
            );
            assert_eq!(
                capacity.get(absent),
                None,
                "case {case:?}: the {map} line for {index} carries no {absent}, got {consumed}"
            );
        }
    }
}

/// Create a table with one global secondary index, so an `INDEXES` request has an index
/// line to report, and wait for it to be usable.
async fn table_with_index(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "owner", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 50, "WriteCapacityUnits": 50},
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByOwner",
                "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 50, "WriteCapacityUnits": 50},
            }],
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
}

#[tokio::test]
async fn every_data_plane_operation_reports_its_capacity_in_its_own_arm() {
    let router = emulator();
    table_with_index(&router, "Arms").await;

    let item = |key: &str| {
        json!({"pk": {"S": key}, "owner": {"S": "o1"}, "v": {"S": "x".repeat(32)}})
    };

    // (case, operation, body, the kind its figures are reported in)
    let rows: Vec<(&str, &str, Value, Kind)> = vec![
        (
            "PutItem, TOTAL",
            "PutItem",
            json!({
                "TableName": "Arms",
                "Item": item("a"),
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Write,
        ),
        (
            "PutItem, INDEXES (the table and its index lines)",
            "PutItem",
            json!({
                "TableName": "Arms",
                "Item": item("b"),
                "ReturnConsumedCapacity": "INDEXES",
            }),
            Kind::Write,
        ),
        (
            "UpdateItem, INDEXES",
            "UpdateItem",
            json!({
                "TableName": "Arms",
                "Key": {"pk": {"S": "a"}},
                "UpdateExpression": "SET v = :v",
                "ExpressionAttributeValues": {":v": {"S": "updated"}},
                "ReturnConsumedCapacity": "INDEXES",
            }),
            Kind::Write,
        ),
        (
            "DeleteItem, TOTAL",
            "DeleteItem",
            json!({
                "TableName": "Arms",
                "Key": {"pk": {"S": "b"}},
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Write,
        ),
        (
            "GetItem, TOTAL",
            "GetItem",
            json!({
                "TableName": "Arms",
                "Key": {"pk": {"S": "a"}},
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
        (
            "GetItem, INDEXES (the table line of a base-table read)",
            "GetItem",
            json!({
                "TableName": "Arms",
                "Key": {"pk": {"S": "a"}},
                "ConsistentRead": true,
                "ReturnConsumedCapacity": "INDEXES",
            }),
            Kind::Read,
        ),
        (
            "Query, TOTAL",
            "Query",
            json!({
                "TableName": "Arms",
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "a"}},
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
        (
            "Scan of the base table, INDEXES",
            "Scan",
            json!({"TableName": "Arms", "ReturnConsumedCapacity": "INDEXES"}),
            Kind::Read,
        ),
        (
            "Scan of a secondary index, INDEXES",
            "Scan",
            json!({
                "TableName": "Arms",
                "IndexName": "ByOwner",
                "ReturnConsumedCapacity": "INDEXES",
            }),
            Kind::Read,
        ),
        (
            "ExecuteStatement SELECT, TOTAL",
            "ExecuteStatement",
            json!({
                "Statement": "SELECT * FROM \"Arms\" WHERE pk = 'a'",
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
        (
            "ExecuteStatement UPDATE, TOTAL",
            "ExecuteStatement",
            json!({
                "Statement": "UPDATE \"Arms\" SET v = 'partiql' WHERE pk = 'a'",
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Write,
        ),
    ];

    for (case, operation, body, kind) in rows {
        let answered = ok(&router, operation, body).await;
        let consumed = &answered["ConsumedCapacity"];
        assert!(
            consumed.is_object(),
            "case {case:?}: a request that asked for capacity gets one object, got {answered}"
        );
        assert_arms(case, kind, consumed);
    }

    // The surfaces that report a LIST of per-table entries follow the same rule, entry by
    // entry.
    let batched: Vec<(&str, &str, Value, Kind)> = vec![
        (
            "BatchWriteItem",
            "BatchWriteItem",
            json!({
                "RequestItems": {"Arms": [{"PutRequest": {"Item": item("c")}}]},
                "ReturnConsumedCapacity": "INDEXES",
            }),
            Kind::Write,
        ),
        (
            "BatchGetItem",
            "BatchGetItem",
            json!({
                "RequestItems": {"Arms": {"Keys": [{"pk": {"S": "c"}}]}},
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
        (
            "TransactGetItems",
            "TransactGetItems",
            json!({
                "TransactItems": [{"Get": {"TableName": "Arms", "Key": {"pk": {"S": "c"}}}}],
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
        (
            "TransactWriteItems",
            "TransactWriteItems",
            json!({
                "TransactItems": [{"Put": {"TableName": "Arms", "Item": item("d")}}],
                "ReturnConsumedCapacity": "INDEXES",
            }),
            Kind::Write,
        ),
        (
            "BatchExecuteStatement",
            "BatchExecuteStatement",
            json!({
                "Statements": [{"Statement": "SELECT * FROM \"Arms\" WHERE pk = 'c'"}],
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
        (
            "ExecuteTransaction",
            "ExecuteTransaction",
            json!({
                "TransactStatements": [{
                    "Statement": "SELECT * FROM \"Arms\" WHERE pk = 'c'",
                }],
                "ReturnConsumedCapacity": "TOTAL",
            }),
            Kind::Read,
        ),
    ];

    for (case, operation, body, kind) in batched {
        let answered = ok(&router, operation, body).await;
        let entries = answered["ConsumedCapacity"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a ConsumedCapacity list, got {answered}")
            })
            .clone();
        assert!(
            !entries.is_empty(),
            "case {case:?}: an operation that did work reports at least one entry, got \
             {answered}"
        );
        for entry in &entries {
            assert_arms(case, kind, entry);
        }
    }

    // A request that asks for NONE — or asks for nothing at all — still reports no
    // capacity, arms included.
    for (case, requested) in [
        ("NONE", json!({"ReturnConsumedCapacity": "NONE"})),
        ("the member left out", json!({})),
    ] {
        let mut body = json!({"TableName": "Arms", "Key": {"pk": {"S": "a"}}});
        if let Some(extra) = requested.as_object() {
            for (member, value) in extra {
                body[member] = value.clone();
            }
        }
        let answered = ok(&router, "GetItem", body).await;
        assert_eq!(
            answered.get("ConsumedCapacity"),
            None,
            "case {case:?}: a request that asked for no capacity gets none, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_token_replay_of_a_transaction_is_billed_as_the_read_it_was() {
    let router = emulator();
    table_with_index(&router, "Replays").await;

    let transaction = json!({
        "TransactItems": [{
            "Put": {
                "TableName": "Replays",
                "Item": {"pk": {"S": "k"}, "owner": {"S": "o"}, "v": {"S": "first"}},
            },
        }],
        "ClientRequestToken": "token-one",
        "ReturnConsumedCapacity": "INDEXES",
    });

    // The FIRST use of the token applies the transaction, so it is billed as the write it
    // made.
    let first = ok(&router, "TransactWriteItems", transaction.clone()).await;
    let first_entries = first["ConsumedCapacity"]
        .as_array()
        .unwrap_or_else(|| panic!("case first use: expected a ConsumedCapacity list, got {first}"))
        .clone();
    for entry in &first_entries {
        assert_arms("first use of the token", Kind::Write, entry);
    }

    // Every REPLAY applies nothing and is billed as a read of the same items — the arm is
    // how a caller tells the call that committed from the ones that did not.
    for attempt in 1..=3 {
        let case = format!("replay {attempt} of the token");
        let replayed = ok(&router, "TransactWriteItems", transaction.clone()).await;
        let entries = replayed["ConsumedCapacity"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {replayed}"))
            .clone();
        assert_eq!(
            entries.len(),
            first_entries.len(),
            "case {case:?}: the replay reports what the first call recorded, got {replayed}"
        );
        for (entry, first_entry) in entries.iter().zip(&first_entries) {
            assert_arms(&case, Kind::Read, entry);
            assert_eq!(
                entry["CapacityUnits"].as_f64(),
                first_entry["CapacityUnits"].as_f64(),
                "case {case:?}: the replay reports the figures the first call recorded, got \
                 {replayed}"
            );
        }
    }

    // The item the first call wrote is the one that stands: a replay changed nothing.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Replays", "Key": {"pk": {"S": "k"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["v"],
        json!({"S": "first"}),
        "case replayed transaction: the item the committed call wrote stands, got {read}"
    );

    // A token the service has not seen is a first use again, billed as a write.
    let other = ok(
        &router,
        "TransactWriteItems",
        json!({
            "TransactItems": [{
                "Put": {
                    "TableName": "Replays",
                    "Item": {"pk": {"S": "k2"}, "owner": {"S": "o"}, "v": {"S": "second"}},
                },
            }],
            "ClientRequestToken": "token-two",
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;
    for entry in other["ConsumedCapacity"]
        .as_array()
        .unwrap_or_else(|| panic!("case a second token: expected a list, got {other}"))
    {
        assert_arms("a second token's first use", Kind::Write, entry);
    }

    // A conditional transaction that is REFUSED reports no capacity at all, so nothing
    // above can be read as "every answer carries an arm".
    let (status, refused) = call(
        &router,
        "TransactWriteItems",
        json!({
            "TransactItems": [{
                "Put": {
                    "TableName": "Replays",
                    "Item": {"pk": {"S": "k"}, "owner": {"S": "o"}},
                    "ConditionExpression": "attribute_not_exists(pk)",
                },
            }],
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a cancelled transaction: expected 400, got {status} {refused}"
    );
}
