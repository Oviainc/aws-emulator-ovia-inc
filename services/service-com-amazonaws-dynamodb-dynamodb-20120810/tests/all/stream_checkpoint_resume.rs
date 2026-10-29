// SPDX-License-Identifier: CC0-1.0

//! A stream-enabled table is reachable through the waiter every client runs, and the
//! records it publishes resume exactly where a consumer checkpointed.
//!
//! Two halves of one rule, pinned end to end over the wire:
//!
//! * `StreamSpecification` is an ordinary optional `CreateTable` member, so it changes
//!   what the table publishes and never whether the table exists: "Upon receiving a
//!   CreateTable request, DynamoDB immediately returns a response with a TableStatus of
//!   CREATING. After the table is created, DynamoDB sets the TableStatus to ACTIVE. ...
//!   You can use the DescribeTable action to check the table status". `DescribeTable`
//!   therefore answers for the table from the instant the create returned — its only
//!   client errors are `ResourceNotFoundException` and `InvalidEndpointException` — and
//!   the status reaches `ACTIVE`, which is what makes a bounded `table_exists` poll
//!   terminate instead of spending a client's whole budget.
//! * A record's `SequenceNumber` is the position a consumer checkpoints, so an iterator
//!   asked for that position delivers exactly from it: `AT_SEQUENCE_NUMBER` from that
//!   record inclusive, `AFTER_SEQUENCE_NUMBER` from the one after it, and the same
//!   checkpoint asked twice delivers the same records — reading a shard positions
//!   nothing for the next reader.
//!
//! The rows span what the rule generalises over rather than one recorded request: every
//! `StreamViewType` the model declares, tables with and without a sort key, and a
//! different number of published changes per row — and inside each row EVERY position in
//! the shard is used as the checkpoint, including the last (whose `AFTER` is empty). The
//! whole suite is driven through DynamoDB and DynamoDB Streams on one engine, so nothing
//! is asserted that a client could not observe.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;

use crate::emulator::{
    call, call_streams, describe_when_active, emulator_with_streams, error_code,
};

/// How many `GetRecords` pages one read of a shard may take before the suite calls it a
/// handler that never advances its iterator. Every row below publishes far fewer records
/// than that, so the bound only keeps a defect from hanging the test.
const MAX_PAGES: usize = 16;

/// One row of the table: the stream the table is created with, its key schema, and how
/// many item changes are published onto it.
struct Row {
    case: &'static str,
    /// A value of the model's `StreamViewType` enum, which
    /// [`STREAM_VIEW_TYPE_VALUES`] enumerates; the assertion at the top of the test
    /// holds this table to that list.
    view_type: &'static str,
    sort_key: bool,
    writes: usize,
}

/// Every view type, each with a key schema and a change count of its own: the rule is
/// indifferent to all three, so no row may answer differently.
const ROWS: &[Row] = &[
    Row {
        case: "NEW_AND_OLD_IMAGES, partition key only, three changes",
        view_type: "NEW_AND_OLD_IMAGES",
        sort_key: false,
        writes: 3,
    },
    Row {
        case: "KEYS_ONLY, composite key, one change",
        view_type: "KEYS_ONLY",
        sort_key: true,
        writes: 1,
    },
    Row {
        case: "NEW_IMAGE, composite key, five changes",
        view_type: "NEW_IMAGE",
        sort_key: true,
        writes: 5,
    },
    Row {
        case: "OLD_IMAGE, partition key only, two changes",
        view_type: "OLD_IMAGE",
        sort_key: false,
        writes: 2,
    },
];

/// The table name every row creates; each row runs on an emulator of its own.
const TABLE: &str = "checkpoints";

/// The `CreateTable` body for one row: a stream-enabled table, on demand.
fn create_body(row: &Row) -> Value {
    let mut key_schema = vec![json!({"AttributeName": "Pk", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "Pk", "AttributeType": "S"})];
    if row.sort_key {
        key_schema.push(json!({"AttributeName": "Sk", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "Sk", "AttributeType": "N"}));
    }
    json!({
        "TableName": TABLE,
        "KeySchema": key_schema,
        "AttributeDefinitions": definitions,
        "BillingMode": "PAY_PER_REQUEST",
        "StreamSpecification": {"StreamEnabled": true, "StreamViewType": row.view_type},
    })
}

/// The key of the `index`th item this row writes.
fn key_of(row: &Row, index: usize) -> Value {
    let mut key = json!({"Pk": {"S": format!("k{index}")}});
    if row.sort_key {
        key["Sk"] = json!({"N": index.to_string()});
    }
    key
}

/// That item, with a payload of its own.
fn item(row: &Row, index: usize) -> Value {
    let mut stored = key_of(row, index);
    stored["Payload"] = json!({"S": format!("v{index}")});
    stored
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

/// The one shard the table's stream serves, as `DescribeStream` reports it.
async fn sole_shard(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    stream_arn: &str,
) -> String {
    let (status, described) =
        call_streams(router, "DescribeStream", json!({"StreamArn": stream_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStream must resolve the table's stream, got {status} with \
         {described}"
    );
    let shards = described["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        shards.len(),
        1,
        "case {case:?}: a stream nothing has rolled serves one open shard, got {described}"
    );
    shards[0]["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ShardId in {described}"))
        .to_string()
}

/// Read one shard from `iterator_type` (at `sequence_number`, where the type takes one)
/// to its end, answering every record delivered.
async fn read_from(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    stream_arn: &str,
    shard: &str,
    iterator_type: &str,
    sequence_number: Option<&str>,
) -> Vec<Value> {
    let mut request = json!({
        "StreamArn": stream_arn,
        "ShardId": shard,
        "ShardIteratorType": iterator_type,
    });
    if let (Some(target), Some(number)) = (request.as_object_mut(), sequence_number) {
        target.insert("SequenceNumber".to_string(), json!(number));
    }
    let (status, minted) = call_streams(router, "GetShardIterator", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetShardIterator({iterator_type}, {sequence_number:?}) answered {status} \
         with {minted}"
    );
    let mut iterator = minted["ShardIterator"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !iterator.is_empty(),
        "case {case:?}: GetShardIterator({iterator_type}, {sequence_number:?}) must mint a \
         position, got {minted}"
    );

    let mut records: Vec<Value> = Vec::new();
    for _ in 0..MAX_PAGES {
        let (status, page) =
            call_streams(router, "GetRecords", json!({"ShardIterator": iterator})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRecords answered {status} with {page}"
        );
        let batch = page["Records"].as_array().cloned().unwrap_or_default();
        records.extend(batch.iter().cloned());
        match page["NextShardIterator"].as_str() {
            Some(next) if !batch.is_empty() => iterator = next.to_string(),
            _ => break,
        }
    }
    records
}

/// The `SequenceNumber` of every record, in the order they were delivered.
fn sequence_numbers(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            record["dynamodb"]["SequenceNumber"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_stream_enabled_table_is_describable_at_once_and_its_records_resume_exactly() {
    // The rows must span the model's own enum, so a value added to it is not silently
    // left untested.
    for declared in STREAM_VIEW_TYPE_VALUES {
        assert!(
            ROWS.iter().any(|row| row.view_type == *declared),
            "every StreamViewType the model declares needs a row; {declared} has none"
        );
    }

    for row in ROWS {
        let case = row.case;
        let router = emulator_with_streams();
        let created = ddb_ok(&router, case, "CreateTable", create_body(row)).await;
        let stream_arn = created["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stream_arn.is_empty(),
            "case {case:?}: an enabled stream must be reported, got {created}"
        );

        // The FIRST poll a client's table_exists waiter makes, with no wait before it:
        // a table the create accepted is addressable at once, never missing.
        let (status, first) = call(&router, "DescribeTable", json!({"TableName": TABLE})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first DescribeTable after the create must answer 200, got \
             {status} with {first}"
        );
        assert_ne!(
            error_code(&first),
            "ResourceNotFoundException",
            "case {case:?}: a table the create accepted is not missing, got {first}"
        );
        let reported = first["Table"]["TableStatus"].as_str().unwrap_or_default();
        assert!(
            reported == "CREATING" || reported == "ACTIVE",
            "case {case:?}: expected TableStatus CREATING or ACTIVE, got {reported:?} in {first}"
        );
        // And the poll TERMINATES: the status reaches ACTIVE within the bounded wait,
        // which is the whole point of the waiter.
        let active = describe_when_active(&router, case, TABLE).await;
        assert_eq!(
            active["LatestStreamArn"],
            json!(stream_arn),
            "case {case:?}: the active table must report the stream its create minted, got \
             {active}"
        );

        for index in 0..row.writes {
            ddb_ok(
                &router,
                case,
                "PutItem",
                json!({"TableName": TABLE, "Item": item(row, index)}),
            )
            .await;
        }

        let shard = sole_shard(&router, case, &stream_arn).await;
        let horizon = read_from(&router, case, &stream_arn, &shard, "TRIM_HORIZON", None).await;
        assert_eq!(
            horizon.len(),
            row.writes,
            "case {case:?}: the shard must carry one record per change, got {horizon:?}"
        );
        let numbers = sequence_numbers(&horizon);
        assert!(
            numbers.iter().all(|number| !number.is_empty()),
            "case {case:?}: every record carries the position it was written at, got {numbers:?}"
        );
        // One shard hands out its positions in order, and every number it hands out is
        // of one width, so a consumer can order two checkpoints by comparing them.
        assert!(
            numbers
                .windows(2)
                .all(|pair| pair[0].len() == pair[1].len() && pair[0] < pair[1]),
            "case {case:?}: the positions must increase along the shard, got {numbers:?}"
        );

        // EVERY position in the shard as the checkpoint, not only the first: AT delivers
        // from that record, AFTER from the next one, and the last record's AFTER is the
        // caught-up reader's empty page.
        for (checkpoint, number) in numbers.iter().enumerate() {
            let at = read_from(
                &router,
                case,
                &stream_arn,
                &shard,
                "AT_SEQUENCE_NUMBER",
                Some(number),
            )
            .await;
            assert_eq!(
                sequence_numbers(&at),
                numbers[checkpoint..],
                "case {case:?}: AT_SEQUENCE_NUMBER({number}) must deliver from that record on, \
                 got {at:?}"
            );
            assert_eq!(
                at.first().map(|record| record["dynamodb"]["Keys"].clone()),
                Some(key_of(row, checkpoint)),
                "case {case:?}: AT_SEQUENCE_NUMBER({number}) must land on the change at that \
                 position, got {at:?}"
            );

            let after = read_from(
                &router,
                case,
                &stream_arn,
                &shard,
                "AFTER_SEQUENCE_NUMBER",
                Some(number),
            )
            .await;
            assert_eq!(
                sequence_numbers(&after),
                numbers[checkpoint + 1..],
                "case {case:?}: AFTER_SEQUENCE_NUMBER({number}) must deliver from the next \
                 record on, got {after:?}"
            );
        }

        // Reading a shard positions nothing for the next reader: the same checkpoint
        // asked again delivers the same records.
        let again = read_from(
            &router,
            case,
            &stream_arn,
            &shard,
            "AT_SEQUENCE_NUMBER",
            numbers.first().map(String::as_str),
        )
        .await;
        assert_eq!(
            sequence_numbers(&again),
            numbers,
            "case {case:?}: a checkpoint resumed twice must deliver the same records, got \
             {again:?}"
        );
    }
}
