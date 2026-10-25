// SPDX-License-Identifier: CC0-1.0

//! A table's stream is ONE resource, minted once by the `CreateTable` that enabled it.
//!
//! The rule pinned here: a resource identity an operation mints is minted ONCE and is
//! then only REPORTED — every later read of the table or of the stream answers the same
//! `LatestStreamArn`/`LatestStreamLabel` the create returned, no read mints a second
//! stream or changes what a later caller sees, and the records the stream holds are a
//! durable log a consumer re-reads from its trim horizon rather than a queue a read
//! consumes ("the ARN that uniquely identifies the latest stream for this table",
//! `TableDescription$LatestStreamArn`; `GetShardIterator` returns "a shard iterator",
//! i.e. a POSITION in the shard, which is why two consumers of one stream see the same
//! change feed).
//!
//! That is the contract a change-data-capture consumer resolves a DynamoDB stream
//! through: it stores the ARN the create reported, and polls it later — possibly
//! alongside another consumer — so an identity that moved after the table settled, or a
//! read that drained the feed, would lose changes without any error being reported.
//!
//! The rows span what the rule generalises over rather than one recorded request: every
//! `StreamViewType` the model declares, both billing modes, tables with and without a
//! sort key, reads taken BEFORE and AFTER the asynchronous `CREATING` -> `ACTIVE`
//! transition, and repeated reads of each surface. The sibling suites
//! (`table_streams.rs`, `stream_enabled_table_round_trip.rs`,
//! `stream_specification_with_other_members.rs`) pin what the create STORES; this one
//! pins that nothing afterwards changes it.
//!
//! Everything is driven over the wire against DynamoDB and DynamoDB Streams on one
//! engine, so nothing is asserted that a client could not observe.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;

use crate::emulator::{
    call, call_streams, drain_stream, emulator_with_streams, event_names, REGION,
};

/// Every value the model's `StreamViewType` enum declares, read from the list the
/// streams crate derives from that model, so this suite cannot drift from the enum.
const VIEW_TYPES: &[&str] = STREAM_VIEW_TYPE_VALUES;

/// How many times each reporting surface is read per row. More than one, because the
/// rule is about what the SECOND read answers.
const READS: usize = 3;

/// The identity a table reports for its latest stream.
#[derive(Debug, PartialEq, Eq)]
struct StreamIdentity {
    arn: String,
    label: String,
}

/// Wait out the `CREATING` -> `ACTIVE` transition the clock owes a new table, so the
/// reads below span both sides of it. The margin is slack for a loaded machine, not
/// part of the rule.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
}

/// Send one DynamoDB operation that must answer 200, returning its body.
async fn ddb_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Send one DynamoDB STREAMS operation that must answer 200, returning its body.
async fn streams_ok(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_streams(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The string at `pointer`, or the empty string when the body carries none.
fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The stream identity a `TableDescription` reports.
fn identity_of(description: &Value) -> StreamIdentity {
    StreamIdentity {
        arn: text(description, "/LatestStreamArn"),
        label: text(description, "/LatestStreamLabel"),
    }
}

/// A `CreateTable` body enabling `view_type`, optionally with a sort key.
fn create_body(name: &str, with_sort_key: bool, view_type: &str, billing: &str) -> Value {
    let mut key_schema = vec![json!({"AttributeName": "Pk", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "Pk", "AttributeType": "S"})];
    if with_sort_key {
        key_schema.push(json!({"AttributeName": "Sk", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "Sk", "AttributeType": "N"}));
    }
    let mut body = json!({
        "TableName": name,
        "KeySchema": key_schema,
        "AttributeDefinitions": definitions,
        "BillingMode": billing,
        "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
    });
    if billing == "PROVISIONED" {
        body["ProvisionedThroughput"] = json!({"ReadCapacityUnits": 2, "WriteCapacityUnits": 2});
    }
    body
}

/// The item one of these tables holds, by key, with `value` as its payload.
fn item(with_sort_key: bool, pk: &str, value: &str) -> Value {
    let mut stored = json!({"Pk": {"S": pk}, "Payload": {"S": value}});
    if with_sort_key {
        stored["Sk"] = json!({"N": "7"});
    }
    stored
}

/// The `Key` map addressing that item.
fn key_of(with_sort_key: bool, pk: &str) -> Value {
    let mut key = json!({"Pk": {"S": pk}});
    if with_sort_key {
        key["Sk"] = json!({"N": "7"});
    }
    key
}

#[tokio::test]
async fn the_stream_identity_a_create_reported_is_what_every_later_read_reports() {
    for (position, view_type) in VIEW_TYPES.iter().enumerate() {
        // The axes the rule is indifferent to are varied across the rows: the key
        // schema and the capacity mode of the table the stream belongs to.
        let with_sort_key = position % 2 == 1;
        let billing = if position % 2 == 0 {
            "PAY_PER_REQUEST"
        } else {
            "PROVISIONED"
        };
        let case = format!("{view_type} (sort key: {with_sort_key}, billing: {billing})");
        let router = emulator_with_streams();
        let name = format!("cdc-{}", view_type.to_lowercase().replace('_', "-"));

        let created = ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body(&name, with_sort_key, view_type, billing),
        )
        .await;
        let minted = identity_of(&created["TableDescription"]);
        assert!(
            !minted.label.is_empty(),
            "case {case:?}: the create must report the stream's label, got {created}"
        );
        assert!(
            minted
                .arn
                .ends_with(&format!("table/{name}/stream/{}", minted.label)),
            "case {case:?}: LatestStreamArn must name this table's stream by its label, got \
             {minted:?}"
        );

        // Read the table repeatedly on BOTH sides of the asynchronous transition: the
        // identity is minted by the create, so neither a read nor the table becoming
        // ACTIVE replaces it.
        for phase in ["while CREATING", "once ACTIVE"] {
            for round in 0..READS {
                let described = ddb_ok(
                    &router,
                    &case,
                    "DescribeTable",
                    json!({"TableName": name}),
                )
                .await;
                assert_eq!(
                    identity_of(&described["Table"]),
                    minted,
                    "case {case:?}: DescribeTable {phase} (read {round}) must report the stream \
                     the create minted, got {described}"
                );
                assert_eq!(
                    described["Table"]["StreamSpecification"],
                    json!({"StreamEnabled": true, "StreamViewType": view_type}),
                    "case {case:?}: DescribeTable {phase} (read {round}) must report the stored \
                     specification, got {described}"
                );
                // The second phase is past the transition the clock owed the table, so
                // it must report the table settled. The first phase is not pinned to
                // `CREATING` in the other direction: how long the span has left when a
                // read lands is timing, not a rule, and the transition itself is pinned
                // by `stream_enabled_table_round_trip.rs`.
                if phase == "once ACTIVE" {
                    assert_eq!(
                        described["Table"]["TableStatus"],
                        json!("ACTIVE"),
                        "case {case:?}: DescribeTable {phase} (read {round}) must report the \
                         table settled, got {described}"
                    );
                }

                // ...and no read has added a second stream for this table.
                let listed = streams_ok(&router, &case, "ListStreams", json!({})).await;
                assert_eq!(
                    listed["Streams"],
                    json!([{
                        "StreamArn": minted.arn,
                        "StreamLabel": minted.label,
                        "TableName": name,
                    }]),
                    "case {case:?}: ListStreams {phase} (read {round}) must report exactly the \
                     one stream the create registered, got {listed}"
                );

                let streamed = streams_ok(
                    &router,
                    &case,
                    "DescribeStream",
                    json!({"StreamArn": minted.arn}),
                )
                .await;
                assert_eq!(
                    text(&streamed, "/StreamDescription/StreamArn"),
                    minted.arn,
                    "case {case:?}: DescribeStream {phase} (read {round}) must answer for the ARN \
                     it was asked about, got {streamed}"
                );
                assert_eq!(
                    text(&streamed, "/StreamDescription/StreamLabel"),
                    minted.label,
                    "case {case:?}: DescribeStream {phase} (read {round}) must report the table's \
                     stream label, got {streamed}"
                );
                assert_eq!(
                    text(&streamed, "/StreamDescription/StreamViewType"),
                    *view_type,
                    "case {case:?}: DescribeStream {phase} (read {round}) must report what the \
                     stream captures, got {streamed}"
                );
            }
            if phase == "while CREATING" {
                settle().await;
            }
        }
    }
}

#[tokio::test]
async fn two_consumers_of_one_stream_read_the_same_change_feed() {
    for (position, view_type) in VIEW_TYPES.iter().enumerate() {
        let with_sort_key = position % 2 == 1;
        let billing = if position % 2 == 0 {
            "PROVISIONED"
        } else {
            "PAY_PER_REQUEST"
        };
        let case = format!("{view_type} (sort key: {with_sort_key}, billing: {billing})");
        let router = emulator_with_streams();
        let name = format!("feed-{}", view_type.to_lowercase().replace('_', "-"));

        let created = ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body(&name, with_sort_key, view_type, billing),
        )
        .await;
        let stream_arn = text(&created["TableDescription"], "/LatestStreamArn");
        settle().await;

        // One change of each kind the model's `OperationType` declares.
        ddb_ok(
            &router,
            &case,
            "PutItem",
            json!({"TableName": name, "Item": item(with_sort_key, "a", "first")}),
        )
        .await;
        ddb_ok(
            &router,
            &case,
            "UpdateItem",
            json!({
                "TableName": name,
                "Key": key_of(with_sort_key, "a"),
                "UpdateExpression": "SET Payload = :value",
                "ExpressionAttributeValues": {":value": {"S": "second"}},
            }),
        )
        .await;
        ddb_ok(
            &router,
            &case,
            "DeleteItem",
            json!({"TableName": name, "Key": key_of(with_sort_key, "a")}),
        )
        .await;

        let first_pass = drain_stream(&router, &case, &stream_arn).await;
        assert_eq!(
            event_names(&first_pass),
            vec!["INSERT", "MODIFY", "REMOVE"],
            "case {case:?}: the stream must carry one record per change, in order, got \
             {first_pass:?}"
        );
        // The SAME drain again, from the trim horizon: a shard iterator is a position
        // in a durable log, so reading it neither consumed nor reordered the records a
        // second consumer — a filter, an enrichment, a replay — still has to see.
        let second_pass = drain_stream(&router, &case, &stream_arn).await;
        assert_eq!(
            second_pass, first_pass,
            "case {case:?}: a second consumer reading from the trim horizon must see the same \
             records, got {second_pass:?} after {first_pass:?}"
        );

        for record in &first_pass {
            let event_name = record["eventName"].as_str().unwrap_or_default();
            assert_eq!(
                record["eventSource"],
                json!("aws:dynamodb"),
                "case {case:?}: the {event_name} record must name the service the change came \
                 from, got {record}"
            );
            assert_eq!(
                record["awsRegion"],
                json!(REGION),
                "case {case:?}: the {event_name} record must name the region the change was made \
                 in, got {record}"
            );
        }

        // Reading the stream changed nothing about the table's own report of it.
        let described = ddb_ok(
            &router,
            &case,
            "DescribeTable",
            json!({"TableName": name}),
        )
        .await;
        assert_eq!(
            text(&described["Table"], "/LatestStreamArn"),
            stream_arn,
            "case {case:?}: the table must still report the stream that was just read, got \
             {described}"
        );
    }
}
