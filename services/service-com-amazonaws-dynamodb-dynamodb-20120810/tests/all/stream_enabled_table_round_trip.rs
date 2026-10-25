// SPDX-License-Identifier: CC0-1.0

//! A stream-enabled table is a WORKING table, end to end.
//!
//! The rule pinned here: a table `CreateTable` accepted is addressable by
//! `DescribeTable` from the instant the create returned — "You can use the
//! DescribeTable action to check the table status", whose only client errors are
//! `ResourceNotFoundException` and `InvalidEndpointException` — it reaches `ACTIVE`
//! after the asynchronous creation span ("Upon receiving a CreateTable request,
//! DynamoDB immediately returns a response with a TableStatus of CREATING. After the
//! table is created, DynamoDB sets the TableStatus to ACTIVE"), and once it is active
//! every item-level change its data plane accepts appears on the stream a
//! `StreamSpecification` enabled: `INSERT` for a key that held no item, `MODIFY` for
//! one that did and still does, `REMOVE` for one that no longer does (the model's
//! `OperationType`), carrying the `Keys` of the item and only the images the stream's
//! `StreamViewType` captures. `StreamSpecification` is an ordinary optional member: it
//! changes what the table publishes, never whether the table exists or when it becomes
//! describable.
//!
//! Each suite below is a TABLE spanning what the rule generalises over — every
//! `StreamViewType` the model declares, tables with and without a sort key, both
//! billing modes, and every surface an item change can arrive through — rather than one
//! recorded request. Everything is driven over the wire against DynamoDB and DynamoDB
//! Streams on one engine, so nothing is asserted that a client could not observe.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;

use crate::emulator::{call, call_in, call_streams, emulator_with_streams, error_code, REGION};

/// Every value the model's `StreamViewType` enum declares, read from the list the
/// streams crate derives from that model, so this suite cannot drift from the enum.
const VIEW_TYPES: &[&str] = STREAM_VIEW_TYPE_VALUES;

/// How long a poll for the documented `CREATING` -> `ACTIVE` transition may take
/// before this suite calls it a hang. Generous slack over the emulator's own
/// transition span, because a loaded machine is not a defect; the point of the bound
/// is that the wait TERMINATES, which is what a client's `table_exists` waiter needs.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(10);

/// How long one poll sleeps between `DescribeTable` calls.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A `CreateTable` body, optionally with a sort key and a `StreamSpecification`.
fn create_body(name: &str, with_sort_key: bool, stream: Option<Value>, billing: &str) -> Value {
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
    });
    if billing == "PROVISIONED" {
        body["ProvisionedThroughput"] = json!({"ReadCapacityUnits": 2, "WriteCapacityUnits": 2});
    }
    if let Some(specification) = stream {
        body["StreamSpecification"] = specification;
    }
    body
}

/// The item one of these tables holds, by key, with `value` as its payload.
fn item(with_sort_key: bool, pk: &str, value: &str) -> Value {
    let mut stored = json!({"Pk": {"S": pk}, "Payload": {"S": value}});
    if with_sort_key {
        stored["Sk"] = json!({"N": "1"});
    }
    stored
}

/// The `Key` map addressing that item.
fn key_of(with_sort_key: bool, pk: &str) -> Value {
    let mut key = json!({"Pk": {"S": pk}});
    if with_sort_key {
        key["Sk"] = json!({"N": "1"});
    }
    key
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

/// Poll `DescribeTable` until the table reports `ACTIVE`, answering every status it
/// reported on the way. Every poll must answer 200: a table the create accepted is
/// never missing.
async fn statuses_until_active(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
) -> Vec<String> {
    let started = std::time::Instant::now();
    let mut seen: Vec<String> = Vec::new();
    while started.elapsed() < ACTIVE_DEADLINE {
        let (status, body) = call(router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTable must resolve a table the create accepted, got {status} \
             with {body}"
        );
        let reported = body["Table"]["TableStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if seen.last() != Some(&reported) {
            seen.push(reported.clone());
        }
        if reported == "ACTIVE" {
            return seen;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the table never reached ACTIVE within {ACTIVE_DEADLINE:?}, saw {seen:?}")
}

/// Every record the stream holds, read from its trim horizon through the STREAMS
/// surface — the loop a consumer runs.
async fn drain_stream(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    stream_arn: &str,
) -> Vec<Value> {
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
    let mut records: Vec<Value> = Vec::new();
    for shard in &shards {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default();
        let (status, minted) = call_streams(
            router,
            "GetShardIterator",
            json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetShardIterator answered {status} with {minted}"
        );
        let mut iterator = minted["ShardIterator"].as_str().unwrap_or_default().to_string();
        // Bounded so a handler that never advances fails rather than hanging.
        for _ in 0..16 {
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
    }
    records
}

/// The `eventName` of every record, in order.
fn event_names(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| record["eventName"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_create_is_describable_at_once_and_reaches_active_whatever_stream_it_asks_for() {
    // The rows vary everything the rule is indifferent to: whether a stream is asked
    // for at all, which view type it captures, whether the table has a sort key, and
    // which billing mode it is created in. None of them may change that the table is
    // describable from the instant the create returned, or that it reaches ACTIVE.
    let mut rows: Vec<(String, Option<Value>, bool, &str)> = vec![
        ("no StreamSpecification".to_string(), None, false, "PAY_PER_REQUEST"),
        (
            "StreamEnabled false".to_string(),
            Some(json!({"StreamEnabled": false})),
            true,
            "PROVISIONED",
        ),
    ];
    for (position, view_type) in VIEW_TYPES.iter().enumerate() {
        rows.push((
            format!("StreamViewType {view_type}"),
            Some(json!({"StreamEnabled": true, "StreamViewType": view_type})),
            position % 2 == 1,
            if position % 2 == 0 {
                "PAY_PER_REQUEST"
            } else {
                "PROVISIONED"
            },
        ));
    }

    for (case, specification, with_sort_key, billing) in rows {
        let router = emulator_with_streams();
        let name = "waiter-subject";
        let created = ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body(name, with_sort_key, specification.clone(), billing),
        )
        .await;
        assert_eq!(
            created["TableDescription"]["TableStatus"],
            json!("CREATING"),
            "case {case:?}: the create reports the table CREATING, got {created}"
        );

        // The FIRST poll a client's waiter makes, with no wait before it: a table the
        // create accepted is addressable at once, never ResourceNotFoundException.
        let (status, first) = call(&router, "DescribeTable", json!({"TableName": name})).await;
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
        assert_eq!(
            first["Table"]["TableName"], json!(name),
            "case {case:?}: DescribeTable must report the table it was asked for, got {first}"
        );

        // And the waiter terminates: the status reaches ACTIVE, having been CREATING.
        let seen = statuses_until_active(&router, &case, name).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some("ACTIVE"),
            "case {case:?}: the table must reach ACTIVE, saw {seen:?}"
        );
        assert!(
            seen.first().map(String::as_str) == Some("CREATING") || seen.len() == 1,
            "case {case:?}: the statuses observed must start at CREATING, saw {seen:?}"
        );

        // Once ACTIVE the data plane serves the table, which is what the waiter is
        // waited on for.
        ddb_ok(
            &router,
            &case,
            "PutItem",
            json!({"TableName": name, "Item": item(with_sort_key, "a", "first")}),
        )
        .await;
        let read = ddb_ok(
            &router,
            &case,
            "GetItem",
            json!({"TableName": name, "Key": key_of(with_sort_key, "a")}),
        )
        .await;
        assert_eq!(
            read["Item"]["Payload"], json!({"S": "first"}),
            "case {case:?}: the item written to the active table must read back, got {read}"
        );
    }
}

#[tokio::test]
async fn every_item_change_reaches_the_tables_stream_with_the_images_it_captures() {
    // One row per `StreamViewType`: the sequence of changes is the same, and what
    // differs is only which images the stream captures.
    for (position, view_type) in VIEW_TYPES.iter().enumerate() {
        let with_sort_key = position % 2 == 1;
        let case = format!("{view_type} (sort key: {with_sort_key})");
        let router = emulator_with_streams();
        let name = "changes";
        let created = ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body(
                name,
                with_sort_key,
                Some(json!({"StreamEnabled": true, "StreamViewType": view_type})),
                "PAY_PER_REQUEST",
            ),
        )
        .await;
        let stream_arn = created["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stream_arn.is_empty(),
            "case {case:?}: an enabled stream must be reported, got {created}"
        );
        statuses_until_active(&router, &case, name).await;

        // A key that held no item, then the same key again, then an update of it, then
        // a delete of it, then a delete of a key that holds nothing.
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
            "PutItem",
            json!({"TableName": name, "Item": item(with_sort_key, "a", "second")}),
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
                "ExpressionAttributeValues": {":value": {"S": "third"}},
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
        ddb_ok(
            &router,
            &case,
            "DeleteItem",
            json!({"TableName": name, "Key": key_of(with_sort_key, "absent")}),
        )
        .await;

        let records = drain_stream(&router, &case, &stream_arn).await;
        assert_eq!(
            event_names(&records),
            vec!["INSERT", "MODIFY", "MODIFY", "REMOVE"],
            "case {case:?}: the stream must carry one record per CHANGE, in order; a delete of \
             an absent key changes nothing, got {records:?}"
        );

        for record in &records {
            let event_name = record["eventName"].as_str().unwrap_or_default();
            let change = &record["dynamodb"];
            // "The primary key attribute(s) for the DynamoDB item that was modified":
            // the key attributes, and only those.
            assert_eq!(
                change["Keys"], key_of(with_sort_key, "a"),
                "case {case:?}: the {event_name} record must carry the item's key alone, got \
                 {record}"
            );
            // An image is reported only where the stream captures it AND the change has
            // one: an INSERT has no before, and a REMOVE has no after.
            let expect_new = matches!(*view_type, "NEW_IMAGE" | "NEW_AND_OLD_IMAGES")
                && event_name != "REMOVE";
            let expect_old = matches!(*view_type, "OLD_IMAGE" | "NEW_AND_OLD_IMAGES")
                && event_name != "INSERT";
            assert_eq!(
                change.get("NewImage").is_some(),
                expect_new,
                "case {case:?}: the {event_name} record must report NewImage only when the view \
                 type captures it and the change has one, got {record}"
            );
            assert_eq!(
                change.get("OldImage").is_some(),
                expect_old,
                "case {case:?}: the {event_name} record must report OldImage only when the view \
                 type captures it and the change has one, got {record}"
            );
            assert_eq!(
                record["awsRegion"], json!(REGION),
                "case {case:?}: the record must name the region the change was made in, got \
                 {record}"
            );
        }

        // The images, where captured, are the item as STORED either side of the change:
        // the second put replaced "first" with "second".
        if matches!(*view_type, "NEW_IMAGE" | "NEW_AND_OLD_IMAGES") {
            assert_eq!(
                records[1]["dynamodb"]["NewImage"],
                item(with_sort_key, "a", "second"),
                "case {case:?}: the MODIFY record's NewImage must be the item the write stored"
            );
        }
        if matches!(*view_type, "OLD_IMAGE" | "NEW_AND_OLD_IMAGES") {
            assert_eq!(
                records[1]["dynamodb"]["OldImage"],
                item(with_sort_key, "a", "first"),
                "case {case:?}: the MODIFY record's OldImage must be the item it replaced"
            );
        }
    }
}

/// One row of the surface table: the operation an item change arrives through, the
/// request that makes it, and the records it must leave on the stream.
struct Surface {
    case: &'static str,
    operation: &'static str,
    /// Built for the table named `changes`, keyed on `Pk` alone.
    request: fn() -> Value,
    events: &'static [&'static str],
}

/// Every surface an item-level change can arrive through in this slice. A change is a
/// change whatever operation made it, so each row must leave the same record the
/// single-item write does.
const SURFACES: &[Surface] = &[
    Surface {
        case: "PutItem",
        operation: "PutItem",
        request: || json!({"TableName": "changes", "Item": {"Pk": {"S": "a"}, "Payload": {"S": "x"}}}),
        events: &["INSERT"],
    },
    Surface {
        case: "UpdateItem on an absent key",
        operation: "UpdateItem",
        request: || {
            json!({
                "TableName": "changes",
                "Key": {"Pk": {"S": "a"}},
                "UpdateExpression": "SET Payload = :value",
                "ExpressionAttributeValues": {":value": {"S": "x"}},
            })
        },
        events: &["INSERT"],
    },
    Surface {
        case: "BatchWriteItem",
        operation: "BatchWriteItem",
        request: || {
            json!({
                "RequestItems": {
                    "changes": [
                        {"PutRequest": {"Item": {"Pk": {"S": "a"}, "Payload": {"S": "x"}}}},
                        {"PutRequest": {"Item": {"Pk": {"S": "b"}, "Payload": {"S": "y"}}}},
                    ]
                }
            })
        },
        events: &["INSERT", "INSERT"],
    },
    Surface {
        case: "TransactWriteItems",
        operation: "TransactWriteItems",
        request: || {
            json!({
                "TransactItems": [
                    {"Put": {"TableName": "changes", "Item": {"Pk": {"S": "a"}, "Payload": {"S": "x"}}}},
                    {"Put": {"TableName": "changes", "Item": {"Pk": {"S": "b"}, "Payload": {"S": "y"}}}},
                ]
            })
        },
        events: &["INSERT", "INSERT"],
    },
    Surface {
        case: "ExecuteStatement INSERT",
        operation: "ExecuteStatement",
        request: || {
            json!({
                "Statement": "INSERT INTO \"changes\" VALUE {'Pk': 'a', 'Payload': 'x'}",
            })
        },
        events: &["INSERT"],
    },
    Surface {
        case: "BatchExecuteStatement",
        operation: "BatchExecuteStatement",
        request: || {
            json!({
                "Statements": [
                    {"Statement": "INSERT INTO \"changes\" VALUE {'Pk': 'a', 'Payload': 'x'}"},
                    {"Statement": "INSERT INTO \"changes\" VALUE {'Pk': 'b', 'Payload': 'y'}"},
                ]
            })
        },
        events: &["INSERT", "INSERT"],
    },
    Surface {
        case: "ExecuteTransaction",
        operation: "ExecuteTransaction",
        request: || {
            json!({
                "TransactStatements": [
                    {"Statement": "INSERT INTO \"changes\" VALUE {'Pk': 'a', 'Payload': 'x'}"},
                ]
            })
        },
        events: &["INSERT"],
    },
];

#[tokio::test]
async fn the_surface_a_change_arrives_through_does_not_change_what_the_stream_carries() {
    for surface in SURFACES {
        let case = surface.case;
        let router = emulator_with_streams();
        let created = ddb_ok(
            &router,
            case,
            "CreateTable",
            create_body(
                "changes",
                false,
                Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"})),
                "PAY_PER_REQUEST",
            ),
        )
        .await;
        let stream_arn = created["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        statuses_until_active(&router, case, "changes").await;

        ddb_ok(&router, case, surface.operation, (surface.request)()).await;

        let records = drain_stream(&router, case, &stream_arn).await;
        assert_eq!(
            event_names(&records),
            surface.events,
            "case {case:?}: expected {:?} on the stream, got {records:?}",
            surface.events
        );
        for record in &records {
            assert!(
                record["dynamodb"]["NewImage"].get("Payload").is_some(),
                "case {case:?}: the record must carry the item the write stored, got {record}"
            );
        }
    }
}

#[tokio::test]
async fn a_change_no_enabled_stream_captures_is_published_nowhere() {
    // A table with no stream, and a table whose stream has been turned off, both
    // accept writes and publish nothing: a DISABLED stream "takes no further records"
    // and keeps exactly what it captured while it was on.
    let case = "no stream";
    let router = emulator_with_streams();
    ddb_ok(
        &router,
        case,
        "CreateTable",
        create_body("quiet", false, None, "PAY_PER_REQUEST"),
    )
    .await;
    statuses_until_active(&router, case, "quiet").await;
    ddb_ok(
        &router,
        case,
        "PutItem",
        json!({"TableName": "quiet", "Item": item(false, "a", "x")}),
    )
    .await;
    let (status, listed) = call_streams(&router, "ListStreams", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListStreams answered {status} with {listed}"
    );
    assert_eq!(
        listed["Streams"], json!([]),
        "case {case:?}: a table with no stream publishes nowhere, got {listed}"
    );

    let case = "a stream turned off";
    let created = ddb_ok(
        &router,
        case,
        "CreateTable",
        create_body(
            "loud",
            false,
            Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"})),
            "PAY_PER_REQUEST",
        ),
    )
    .await;
    let stream_arn = created["TableDescription"]["LatestStreamArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    statuses_until_active(&router, case, "loud").await;
    ddb_ok(
        &router,
        case,
        "PutItem",
        json!({"TableName": "loud", "Item": item(false, "a", "captured")}),
    )
    .await;
    ddb_ok(
        &router,
        case,
        "UpdateTable",
        json!({"TableName": "loud", "StreamSpecification": {"StreamEnabled": false}}),
    )
    .await;
    ddb_ok(
        &router,
        case,
        "PutItem",
        json!({"TableName": "loud", "Item": item(false, "b", "after the disable")}),
    )
    .await;

    let records = drain_stream(&router, case, &stream_arn).await;
    assert_eq!(
        event_names(&records),
        vec!["INSERT"],
        "case {case:?}: a disabled stream keeps what it captured and takes no more, got \
         {records:?}"
    );
    assert_eq!(
        records[0]["dynamodb"]["Keys"], json!({"Pk": {"S": "a"}}),
        "case {case:?}: the one record is the change made while the stream was on, got {records:?}"
    );
}

#[tokio::test]
async fn a_cancelled_transaction_publishes_nothing() {
    // A transactional write is all-or-nothing, so a transaction that is cancelled made
    // no item-level modification and its tables' streams carry no record of one.
    let case = "a transaction cancelled by a condition";
    let router = emulator_with_streams();
    let created = ddb_ok(
        &router,
        case,
        "CreateTable",
        create_body(
            "ledger",
            false,
            Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES"})),
            "PAY_PER_REQUEST",
        ),
    )
    .await;
    let stream_arn = created["TableDescription"]["LatestStreamArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    statuses_until_active(&router, case, "ledger").await;

    let (status, refused) = call(
        &router,
        "TransactWriteItems",
        json!({
            "TransactItems": [
                {"Put": {"TableName": "ledger", "Item": {"Pk": {"S": "a"}, "Payload": {"S": "x"}}}},
                {"Put": {
                    "TableName": "ledger",
                    "Item": {"Pk": {"S": "b"}, "Payload": {"S": "y"}},
                    "ConditionExpression": "attribute_exists(Payload)",
                }},
            ]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the transaction must be cancelled, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "TransactionCanceledException",
        "case {case:?}: expected TransactionCanceledException, got {refused}"
    );
    assert_eq!(
        drain_stream(&router, case, &stream_arn).await,
        Vec::<Value>::new(),
        "case {case:?}: a cancelled transaction publishes nothing"
    );
}

#[tokio::test]
async fn a_table_is_describable_and_publishes_in_the_region_it_was_created_in() {
    // A table name resolves inside ONE region, and so does its stream: the same name
    // created in two regions is two tables, each describable in its own region and
    // publishing to its own stream.
    let case = "two regions, one table name";
    let router = emulator_with_streams();
    let other = "eu-west-1";
    let mut arns: Vec<String> = Vec::new();
    for region in [REGION, other] {
        let (status, created) = call_in(
            &router,
            region,
            "CreateTable",
            create_body(
                "regional",
                false,
                Some(json!({"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"})),
                "PAY_PER_REQUEST",
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTable in {region} answered {status} with {created}"
        );
        let arn = created["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.contains(region),
            "case {case:?}: the stream of the table in {region} must be that region's, got {arn}"
        );
        arns.push(arn);
    }
    assert_ne!(
        arns[0], arns[1],
        "case {case:?}: the two regions' tables must have streams of their own, got {arns:?}"
    );

    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
    for region in [REGION, other] {
        let (status, described) =
            call_in(&router, region, "DescribeTable", json!({"TableName": "regional"})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTable in {region} answered {status} with {described}"
        );
        assert_eq!(
            described["Table"]["TableStatus"], json!("ACTIVE"),
            "case {case:?}: the table in {region} must be ACTIVE, got {described}"
        );
    }
}
