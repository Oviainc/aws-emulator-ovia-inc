// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH writes a table's Kinesis streaming destination delivers,
//! and HOW the records of one item are placed in the stream's shards.
//!
//! The delivery itself — the record's members, the unit its creation time is stated in,
//! and the destination statuses that deliver nothing — is pinned by
//! `kinesis_destination_record_delivery`, over `PutItem` and `DeleteItem`. This suite
//! widens the same rule along the two axes that one does not:
//!
//! 1. an item change is delivered whichever operation made it: the singleton item API,
//!    the batch API, the transactional API and the PartiQL API all land items through one
//!    store, so each owes the destination the same one record per change, naming the
//!    modification it was and carrying the images the change left;
//! 2. the records of ONE item are delivered under one partition key, so a stream with
//!    several shards carries each item's changes in a single shard
//!    (`PutRecordInput$PartitionKey`: "all data records with the same partition key map to
//!    the same shard within the stream").
//!
//! Each rule is asserted over a SET of inputs — the operation that made the change and
//! the modification it was; four items over a four-shard stream — rather than one example.
//! Every call is driven over the wire (`POST /` with
//! `X-Amz-Target: <Service>.<OperationName>`) through the same `MultiServiceRouter` the
//! emulator's front doors use, with the DynamoDB and the Kinesis service mounted on ONE
//! shared engine, and the records are read back through Kinesis's own
//! `GetShardIterator`/`GetRecords`, where a consumer reads them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::{create_table, settle};
use crate::emulator::{emulator, ok, REGION};

/// The partition-key attribute of the tables this suite creates
/// (`crate::contributor_insights::create_table`).
const KEY: &str = "Part";

/// The Kinesis data stream `name` with `shards` shards, as the harness creates one
/// ([`crate::emulator::ensure_kinesis_stream_with_shards`]), answering the ARN that service
/// reports for it.
///
/// The shard COUNT is this suite's own parameter because rule 2 is about the shard map: the
/// stream serves that map once its activation falls due, which the destination's own
/// registration span then waits out ([`replicate`]).
async fn delivering_stream(router: &MultiServiceRouter, name: &str, shards: u32) -> String {
    crate::emulator::ensure_kinesis_stream_with_shards(router, name, shards).await
}

/// Start `table` replicating to `stream_arn` and wait out the registration span, so the
/// destination is delivering when this returns.
async fn replicate(router: &MultiServiceRouter, table: &str, stream_arn: &str) {
    ok(
        router,
        "EnableKinesisStreamingDestination",
        json!({"TableName": table, "StreamArn": stream_arn}),
    )
    .await;
    settle().await;
}

/// Every record the Kinesis data stream `stream` holds, as the shard it landed in and the
/// change document it carries — the harness's own drain
/// ([`crate::emulator::drain_kinesis_stream`]), because rule 2 below is about WHERE a
/// record landed as well as what it says.
async fn delivered(
    router: &MultiServiceRouter,
    case: &str,
    stream: &str,
) -> Vec<(String, Value)> {
    crate::emulator::drain_kinesis_stream(router, case, stream).await
}

/// One row of the delivery table: the operation that makes the change, whether an item is
/// seeded before replication starts, the modification the record must name, and which
/// images it must carry.
struct Mutation {
    case: &'static str,
    /// The operation the change is made with, and its request body for the item `Item1`.
    operation: &'static str,
    request: fn(&str) -> Value,
    /// Whether the item exists before the destination starts delivering. Seeded BEFORE the
    /// enable, so the seed itself is not delivered and the row's own write is the only
    /// change on the stream.
    seeded: bool,
    event_name: &'static str,
    new_image: bool,
    old_image: bool,
}

/// The item the rows below write, as the table's own key plus one payload attribute.
fn item(table_key: &str, payload: &str) -> Value {
    json!({KEY: {"S": table_key}, "Payload": {"S": payload}})
}

/// Rule 1: every item-level change on a delivering table reaches the destination's stream,
/// whichever operation made it, carrying the key and the images the change left.
#[tokio::test]
async fn a_delivering_destination_delivers_whichever_write_made_the_change() {
    let mutations: Vec<Mutation> = vec![
        Mutation {
            case: "UpdateItem edits an item that was there",
            operation: "UpdateItem",
            request: |table| {
                json!({
                    "TableName": table,
                    "Key": {KEY: {"S": "Item1"}},
                    "UpdateExpression": "SET Payload = :payload",
                    "ExpressionAttributeValues": {":payload": {"S": "edited"}},
                })
            },
            seeded: true,
            event_name: "MODIFY",
            new_image: true,
            old_image: true,
        },
        Mutation {
            case: "UpdateItem creates the item it addresses",
            operation: "UpdateItem",
            request: |table| {
                json!({
                    "TableName": table,
                    "Key": {KEY: {"S": "Item1"}},
                    "UpdateExpression": "SET Payload = :payload",
                    "ExpressionAttributeValues": {":payload": {"S": "created"}},
                })
            },
            seeded: false,
            event_name: "INSERT",
            new_image: true,
            old_image: false,
        },
        Mutation {
            case: "BatchWriteItem puts an item",
            operation: "BatchWriteItem",
            request: |table| {
                json!({
                    "RequestItems": {
                        table: [{"PutRequest": {"Item": item("Item1", "batched")}}],
                    },
                })
            },
            seeded: false,
            event_name: "INSERT",
            new_image: true,
            old_image: false,
        },
        Mutation {
            case: "BatchWriteItem deletes an item",
            operation: "BatchWriteItem",
            request: |table| {
                json!({
                    "RequestItems": {
                        table: [{"DeleteRequest": {"Key": {KEY: {"S": "Item1"}}}}],
                    },
                })
            },
            seeded: true,
            event_name: "REMOVE",
            new_image: false,
            old_image: true,
        },
        Mutation {
            case: "TransactWriteItems puts an item",
            operation: "TransactWriteItems",
            request: |table| {
                json!({
                    "TransactItems": [{
                        "Put": {"TableName": table, "Item": item("Item1", "transacted")},
                    }],
                })
            },
            seeded: false,
            event_name: "INSERT",
            new_image: true,
            old_image: false,
        },
        Mutation {
            case: "ExecuteStatement inserts an item",
            operation: "ExecuteStatement",
            request: |table| {
                json!({
                    "Statement": format!(
                        "INSERT INTO \"{table}\" VALUE {{'{KEY}': 'Item1', 'Payload': 'stated'}}"
                    ),
                })
            },
            seeded: false,
            event_name: "INSERT",
            new_image: true,
            old_image: false,
        },
    ];

    for (index, mutation) in mutations.into_iter().enumerate() {
        let case = mutation.case;
        let router = emulator();
        let table = format!("Delivering{index}");
        create_table(&router, &table, &[]).await;
        let stream = format!("cdc-{index}");
        let stream_arn = delivering_stream(&router, &stream, 1).await;
        // A new table serves item writes once its creation falls due, which the seed below
        // and the row's own write both need.
        settle().await;
        if mutation.seeded {
            ok(
                &router,
                "PutItem",
                json!({"TableName": table, "Item": item("Item1", "seeded")}),
            )
            .await;
        }
        replicate(&router, &table, &stream_arn).await;

        ok(&router, mutation.operation, (mutation.request)(&table)).await;
        let records = delivered(&router, case, &stream).await;
        assert_eq!(
            records.len(),
            1,
            "case {case:?}: the change must reach the destination's stream as one record, \
             got {records:?}"
        );
        let (_, document) = &records[0];
        assert_eq!(
            document["eventName"],
            json!(mutation.event_name),
            "case {case:?}: expected the modification {:?}, got {document}",
            mutation.event_name
        );
        assert_eq!(
            (
                document["tableName"].clone(),
                document["awsRegion"].clone(),
                document["eventSource"].clone(),
            ),
            (json!(table), json!(REGION), json!("aws:dynamodb")),
            "case {case:?}: the record names where the change was made, got {document}"
        );
        assert_eq!(
            document["dynamodb"]["Keys"],
            json!({KEY: {"S": "Item1"}}),
            "case {case:?}: the record carries the item's primary key, got {document}"
        );
        assert_eq!(
            document["dynamodb"]["NewImage"].is_object(),
            mutation.new_image,
            "case {case:?}: expected a new image {}, got {document}",
            mutation.new_image
        );
        assert_eq!(
            document["dynamodb"]["OldImage"].is_object(),
            mutation.old_image,
            "case {case:?}: expected an old image {}, got {document}",
            mutation.old_image
        );
        // The images are the items themselves, so each carries the key as well as the
        // payload the write stored.
        if mutation.new_image {
            assert_eq!(
                document["dynamodb"]["NewImage"][KEY],
                json!({"S": "Item1"}),
                "case {case:?}: the new image is the stored item, got {document}"
            );
        }
        if mutation.old_image {
            assert_eq!(
                document["dynamodb"]["OldImage"]["Payload"],
                json!({"S": "seeded"}),
                "case {case:?}: the old image is the item as it was, got {document}"
            );
        }
    }
}

/// Rule 2: the changes of one item are delivered under one partition key, so a stream with
/// several shards still carries each item's changes in a single shard.
#[tokio::test]
async fn the_changes_of_one_item_land_in_one_shard() {
    let router = emulator();
    create_table(&router, "Sharded", &[]).await;
    // Several shards, so the rule has something to hold over: with one shard every record
    // lands in it whatever the partition key is.
    let stream_arn = delivering_stream(&router, "sharded", 4).await;
    replicate(&router, "Sharded", &stream_arn).await;

    // Three changes of each of four items, so the assertion is about the GROUPING rather
    // than about one key's placement.
    let keys = ["Alpha", "Beta", "Gamma", "Delta"];
    for key in keys {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Sharded", "Item": item(key, "first")}),
        )
        .await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Sharded", "Item": item(key, "second")}),
        )
        .await;
        ok(
            &router,
            "DeleteItem",
            json!({"TableName": "Sharded", "Key": {KEY: {"S": key}}}),
        )
        .await;
    }

    let records = delivered(&router, "one shard per item", "sharded").await;
    assert_eq!(
        records.len(),
        keys.len() * 3,
        "case one shard per item: every change must be delivered, got {records:?}"
    );
    for key in keys {
        let shards: Vec<String> = records
            .iter()
            .filter(|(_, document)| document["dynamodb"]["Keys"][KEY] == json!({"S": key}))
            .map(|(shard, _)| shard.clone())
            .collect();
        assert_eq!(
            shards.len(),
            3,
            "case item {key:?}: expected its three changes, got {shards:?}"
        );
        assert!(
            shards.iter().all(|shard| shard == &shards[0]),
            "case item {key:?}: the changes of one item land in one shard, got {shards:?}"
        );
    }
}
