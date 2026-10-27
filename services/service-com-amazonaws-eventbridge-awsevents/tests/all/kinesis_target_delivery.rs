// SPDX-License-Identifier: CC0-1.0

//! A rule target whose `Arn` is a Kinesis data stream is WRITTEN to, end to end
//! over the wire through the real `MultiServiceRouter`, with Kinesis mounted on
//! the same engine so the delivery is observed where a consumer observes it: as
//! records a shard serves.
//!
//! The RULE asserted here: for every event an enabled rule matches, each of its
//! Kinesis data stream targets results in ONE `kinesis:PutRecord` onto the stream
//! the target names, whose data is the event the target is delivered and whose
//! partition key is resolved PER EVENT — `KinesisParameters` "enables you to
//! specify a JSON path to extract from the event and use as the partition key for
//! the Amazon Kinesis data stream, so that you can control the shard to which the
//! event goes. If you do not include this parameter, the default is to use the
//! `eventId` as the partition key". A partition key is a string while the selected
//! value is any JSON value, so the key carries that value's JSON TEXT, and a path
//! that selects no single value is not a failed delivery: the record is written
//! keyed on the empty JSON object.
//!
//! The first table spans the class rather than the one recorded case: its rows
//! differ along what the path selects — a string, a number, an object, an array
//! element, an envelope member, a field the event does not carry — and along a
//! target carrying no `KinesisParameters` at all. Every row is exercised against
//! TWO events whose selected field differs, which is what catches a key resolved
//! once per target instead of once per event.
//!
//! The second table is the other half of the same rule: a stream target is one AWS
//! invokes UNDER the target's `RoleArn` ("for EC2 instances, Kinesis data streams,
//! Step Functions state machines and API Gateway APIs, EventBridge relies on IAM
//! roles that you specify in the `RoleARN` argument in `PutTargets`"), so its rows
//! differ along what the role's records say about the write — and a delivery that
//! succeeds leaves the target's dead-letter queue empty.
//!
//! The harness (the router with EventBridge, Kinesis, SQS, IAM and CloudWatch on
//! one engine, and the rule/queue/role fixtures) is the one `target_dispatch`
//! owns.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    InvocationOutcome, MAX_TARGETS_PER_RULE, SERVICE_PRINCIPAL,
};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    ACTIVATION_DELAY, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};

use crate::target_dispatch::{
    drain, emulator, given_queue, given_role, given_rule, invocations, kinesis, publish_details,
    Emulator,
};

/// The rule every case below publishes through, and the stream its targets name.
const RULE_NAME: &str = "to-the-stream";
const STREAM_NAME: &str = "telemetry";

/// The action a Kinesis data stream target's invocation performs, and so the one
/// the target's role has to be allowed to make.
const TARGET_ACTION: &str = "kinesis:PutRecord";

/// How long a case waits before treating a freshly created stream as ready: past
/// the instant the create armed the stream's `CREATING` -> `ACTIVE` transition
/// for.
fn past_activation() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(400)
}

/// A provisioned stream of `shards` shards, past the transition its creation
/// armed; answered with the ARN a target names it by.
///
/// More than one shard, because the partition key is what chooses among them: a
/// single-shard stream would hold every record whatever key it was written under.
async fn given_stream(emulator: &Emulator, case: &str, shards: i64) -> String {
    kinesis(
        emulator,
        case,
        "CreateStream",
        &json!({ "StreamName": STREAM_NAME, "ShardCount": shards }),
    )
    .await;
    tokio::time::sleep(past_activation()).await;
    let summary = kinesis(
        emulator,
        case,
        "DescribeStreamSummary",
        &json!({ "StreamName": STREAM_NAME }),
    )
    .await;
    summary["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
        .to_string()
}

/// Every record the stream holds, as (partition key, data text) pairs, read from
/// each of its shards from that shard's oldest retained record.
async fn records(emulator: &Emulator, case: &str) -> Vec<(String, String)> {
    let listed = kinesis(
        emulator,
        case,
        "ListShards",
        &json!({ "StreamName": STREAM_NAME }),
    )
    .await;
    let shards: Vec<String> = listed["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|shard| {
            shard["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a shard carries no ShardId"))
                .to_string()
        })
        .collect();
    assert!(
        !shards.is_empty(),
        "case {case}: the stream has at least one shard to read, got {listed}"
    );
    let mut held: Vec<(String, String)> = Vec::new();
    for shard in shards {
        let iterator = kinesis(
            emulator,
            case,
            "GetShardIterator",
            &json!({
                "StreamName": STREAM_NAME,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator for {shard}"))
            .to_string();
        let answer = kinesis(
            emulator,
            case,
            "GetRecords",
            &json!({ "ShardIterator": iterator }),
        )
        .await;
        for record in answer["Records"].as_array().cloned().unwrap_or_default() {
            let encoded = record["Data"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a record carries no Data: {record}"));
            let data = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case}: a record's Data is not base64 ({error}): {record}")
            });
            held.push((
                record["PartitionKey"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a record carries no PartitionKey: {record}")
                    })
                    .to_string(),
                String::from_utf8_lossy(&data).to_string(),
            ));
        }
    }
    held
}

/// The `Detail` each published event carries: the members the rows below select
/// from, with `tenant` the one that differs between the two events.
fn detail(tenant: &str) -> String {
    json!({
        "tenant": tenant,
        "n": 7,
        "o": { "a": 1, "b": [2] },
        "tags": ["first", "second"],
    })
    .to_string()
}

/// The two tenants the published pair differs along, so a key resolved once per
/// target rather than once per event cannot pass.
const TENANTS: [&str; 2] = ["tenant-alpha", "tenant-omega"];

/// One row: a target of the rule, the `PartitionKeyPath` it carries (`None` for a
/// target carrying no `KinesisParameters`), and the partition key its record is
/// expected under for an event of the given id and tenant.
struct Row {
    case: &'static str,
    id: &'static str,
    path: Option<&'static str>,
    key: fn(&str, &str) -> String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a path selecting a string field of the detail",
            id: "sel",
            path: Some("$.detail.tenant"),
            // A string's JSON text is quoted.
            key: |_event_id, tenant| format!("\"{tenant}\""),
        },
        Row {
            case: "no KinesisParameters at all",
            id: "dflt",
            path: None,
            key: |event_id, _tenant| event_id.to_string(),
        },
        Row {
            case: "a path selecting a number",
            id: "num",
            path: Some("$.detail.n"),
            key: |_event_id, _tenant| "7".to_string(),
        },
        Row {
            case: "a path selecting an object",
            id: "obj",
            path: Some("$.detail.o"),
            // Compact JSON text, with no spaces.
            key: |_event_id, _tenant| "{\"a\":1,\"b\":[2]}".to_string(),
        },
        Row {
            case: "a path selecting an array element",
            id: "elem",
            path: Some("$.detail.tags[0]"),
            key: |_event_id, _tenant| "\"first\"".to_string(),
        },
        Row {
            case: "a path selecting a member of the envelope itself",
            id: "env",
            path: Some("$.id"),
            key: |event_id, _tenant| format!("\"{event_id}\""),
        },
        Row {
            case: "a path selecting a field the event does not carry",
            id: "absent",
            path: Some("$.detail.missing"),
            key: |_event_id, _tenant| "{}".to_string(),
        },
        Row {
            case: "a path in bracket notation",
            id: "bracketed",
            path: Some("$['detail']['tenant']"),
            key: |_event_id, tenant| format!("\"{tenant}\""),
        },
        Row {
            case: "a path selecting a hyphenated envelope member",
            id: "hyphenated",
            path: Some("$.detail-type"),
            key: |_event_id, _tenant| "\"Order Created\"".to_string(),
        },
        Row {
            // A descendant selects every matching member at every depth, so it
            // names no single value to key the record on — and is not evaluated
            // over the event at all.
            case: "a path whose descendant segment names no one place",
            id: "descendant",
            path: Some("$..tenant"),
            key: |_event_id, _tenant| "{}".to_string(),
        },
        Row {
            case: "a path whose wildcard names no one place",
            id: "wildcard",
            path: Some("$.detail.*"),
            key: |_event_id, _tenant| "{}".to_string(),
        },
        Row {
            case: "a path that is not a JSONPath expression at all",
            id: "unreadable",
            path: Some("detail.tenant"),
            key: |_event_id, _tenant| "{}".to_string(),
        },
    ]
}

/// Every Kinesis target of a matched rule is written one record per event, keyed
/// by what its `PartitionKeyPath` selects out of that event, with the event as the
/// record's data and nothing on the dead-letter queue.
#[tokio::test]
async fn a_kinesis_target_is_written_one_record_per_event_keyed_by_its_path() {
    let emulator = emulator();
    let case = "kinesis targets";
    let stream_arn = given_stream(&emulator, case, 2).await;
    let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "stream-dead-letters").await;
    let role_arn = given_role(
        &emulator,
        "stream-writer",
        SERVICE_PRINCIPAL,
        Some(TARGET_ACTION),
    )
    .await;

    let rows = rows();
    let targets: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut target = json!({
                "Id": row.id,
                "Arn": stream_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            });
            if let Some(path) = row.path {
                target["KinesisParameters"] = json!({ "PartitionKeyPath": path });
            }
            target
        })
        .collect();
    // A rule holds at most `MAX_TARGETS_PER_RULE` targets, so the rows are spread
    // over as many rules as they need; every one of them matches the same event,
    // so each target is still delivered to once per event.
    for (index, chunk) in targets.chunks(MAX_TARGETS_PER_RULE).enumerate() {
        given_rule(
            &emulator,
            &format!("{RULE_NAME}-{index}"),
            Value::Array(chunk.to_vec()),
        )
        .await;
    }

    let details: Vec<String> = TENANTS.iter().map(|tenant| detail(tenant)).collect();
    let event_ids = publish_details(&emulator, &details).await;
    assert_eq!(
        event_ids.len(),
        TENANTS.len(),
        "case {case}: expected one EventId per published entry, got {event_ids:?}"
    );

    // What the stream holds, grouped by partition key: every event delivered
    // under one key is one record, so the COUNT under a key is part of the
    // assertion rather than only the presence of the key.
    let held = records(&emulator, case).await;
    let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, data) in &held {
        by_key.entry(key.clone()).or_default().push(data.clone());
    }

    for (tenant, event_id) in TENANTS.iter().zip(&event_ids) {
        for row in &rows {
            let expected = (row.key)(event_id, tenant);
            // Two rows may key their records the same way — a path selecting
            // nothing and one that is no path at all both do — so what this
            // event must hold under that key is one record per such row.
            let wanted = rows
                .iter()
                .filter(|other| (other.key)(event_id, tenant) == expected)
                .count();
            let written = by_key.get(&expected).cloned().unwrap_or_default();
            let carrying: Vec<&String> = written
                .iter()
                .filter(|data| delivered_event_id(data) == *event_id)
                .collect();
            assert_eq!(
                carrying.len(),
                wanted,
                "case {:?} ({tenant}): expected {wanted} record(s) of event {event_id} under the \
                 partition key {expected:?}, got {written:?} under it and {held:?} in all",
                row.case
            );
            for data in carrying {
                assert_delivered_event(row.case, data, event_id, tenant);
            }
        }
    }
    assert_eq!(
        held.len(),
        rows.len() * TENANTS.len(),
        "case {case}: expected one record per target per event and no others, got {held:?}"
    );

    let dead_letters = drain(&emulator, &dead_letter_url).await;
    assert!(
        dead_letters.is_empty(),
        "case {case}: a delivery that succeeded dead-letters nothing, got {dead_letters:?}"
    );
}

/// The `id` of the event a record's data carries.
fn delivered_event_id(data: &str) -> String {
    serde_json::from_str::<Value>(data)
        .ok()
        .and_then(|parsed| parsed["id"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A record's data is the event the target was delivered: the whole envelope,
/// carrying this event's id and the detail it was published with.
fn assert_delivered_event(case: &str, data: &str, event_id: &str, tenant: &str) {
    let parsed: Value = serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("case {case:?}: a record's data is not JSON ({error})"));
    assert_eq!(
        parsed["id"],
        json!(event_id),
        "case {case:?}: expected the record to carry event {event_id}, got {parsed}"
    );
    assert_eq!(
        parsed["detail"],
        serde_json::from_str::<Value>(&detail(tenant)).expect("the detail is JSON"),
        "case {case:?}: expected the record's data to be the delivered event, got {parsed}"
    );
}

/// One row of the role table: what the target's role says about the write, and
/// whether the record is written or the event dead-lettered instead.
struct RoleRow {
    case: &'static str,
    /// The service principal the role's trust policy names, and the one action
    /// its inline policy allows.
    principal: &'static str,
    allowed: &'static str,
    /// The `ERROR_CODE` the dead-letter message carries, `None` for a delivery
    /// that goes through.
    dead_lettered: Option<&'static str>,
}

/// A Kinesis data stream target is invoked UNDER the target's role, so the role's
/// records decide whether the record is written at all — and a write the role
/// allows leaves the dead-letter queue empty.
#[tokio::test]
async fn a_stream_targets_role_decides_whether_the_record_is_written() {
    let rows = [
        RoleRow {
            case: "the role allows the stream write",
            principal: SERVICE_PRINCIPAL,
            allowed: TARGET_ACTION,
            dead_lettered: None,
        },
        RoleRow {
            case: "the role's policy names another action",
            principal: SERVICE_PRINCIPAL,
            allowed: "sqs:SendMessage",
            // The condition's own name, as the literal EventBridge stamps.
            dead_lettered: Some("NO_PERMISSIONS"),
        },
        RoleRow {
            case: "the role does not trust the EventBridge service principal",
            principal: "lambda.amazonaws.com",
            allowed: TARGET_ACTION,
            dead_lettered: Some("FAILED_TO_ASSUME_ROLE"),
        },
    ];

    for row in rows {
        let emulator = emulator();
        let stream_arn = given_stream(&emulator, row.case, 2).await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "role-dead-letters").await;
        let role_arn =
            given_role(&emulator, "stream-writer", row.principal, Some(row.allowed)).await;
        given_rule(
            &emulator,
            RULE_NAME,
            json!([{
                "Id": "sel",
                "Arn": stream_arn,
                "RoleArn": role_arn,
                "KinesisParameters": { "PartitionKeyPath": "$.detail.tenant" },
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;

        let details = vec![detail(TENANTS[0])];
        let event_ids = publish_details(&emulator, &details).await;
        let held = records(&emulator, row.case).await;
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        let recorded = invocations(&emulator);

        match row.dead_lettered {
            None => {
                assert_eq!(
                    held.len(),
                    1,
                    "case {:?}: expected the record to be written, got {held:?}",
                    row.case
                );
                assert_eq!(
                    held[0].0,
                    format!("\"{}\"", TENANTS[0]),
                    "case {:?}: expected the path-selected partition key, got {held:?}",
                    row.case
                );
                assert_delivered_event(row.case, &held[0].1, &event_ids[0], TENANTS[0]);
                assert!(
                    dead_letters.is_empty(),
                    "case {:?}: a delivery that succeeded dead-letters nothing, got \
                     {dead_letters:?}",
                    row.case
                );
                assert!(
                    recorded
                        .iter()
                        .all(|(_, _, outcome)| *outcome == InvocationOutcome::Invoked),
                    "case {:?}: expected the handover to be recorded as invoked, got {recorded:?}",
                    row.case
                );
            }
            Some(code) => {
                assert!(
                    held.is_empty(),
                    "case {:?}: expected no record for a write the role bars, got {held:?}",
                    row.case
                );
                assert_eq!(
                    dead_letters.len(),
                    1,
                    "case {:?}: an undeliverable event goes to the dead-letter queue, got \
                     {dead_letters:?}",
                    row.case
                );
                assert_eq!(
                    dead_letters[0]["MessageAttributes"]["ERROR_CODE"]["StringValue"],
                    json!(code),
                    "case {:?}: expected the dead-letter ERROR_CODE {code}, got {:?}",
                    row.case,
                    dead_letters[0]
                );
            }
        }
    }
}
