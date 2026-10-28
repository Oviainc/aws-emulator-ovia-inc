// SPDX-License-Identifier: CC0-1.0

//! What a CONSUMER of the stream sees when a rule's Kinesis data stream targets
//! are delivered to, end to end over the wire through the real
//! `MultiServiceRouter` with Kinesis mounted beside EventBridge on one engine.
//!
//! The RULE asserted here: for every event an enabled rule matches — whichever
//! event bus the rule lives on — each of its Kinesis data stream targets results
//! in exactly ONE record on the stream the target names, carrying the delivered
//! envelope as its data and keyed on the JSON TEXT of the value the target's
//! `PartitionKeyPath` selects out of that envelope (the empty JSON object when it
//! selects no single value, which is a delivery that still happens rather than a
//! dead-lettered failure), and a consumer reading a shard from its oldest retained
//! record and continuing through each answer's `NextShardIterator` is served every
//! one of those records EXACTLY ONCE: "GetRecords returns a new shard iterator in
//! NextShardIterator. Specify the shard iterator returned in NextShardIterator in
//! subsequent calls to GetRecords."
//!
//! `kinesis_target_delivery` asserts the key a path selects over the forms a path
//! can take; this suite is the consumer's half of the same rule, so its rows differ
//! along what a reader's position has to survive: the event bus the rule lives on
//! (a bus a client created, and the account's default bus), the SHARD COUNT the
//! records are spread over, and a SECOND publication arriving after the reader has
//! already drained the first — which is what a consumer polling a stream does, and
//! what catches records that are written once but served twice, or not at all.
//!
//! Every row delivers to five targets of one stream whose `PartitionKeyPath`s span
//! the value forms a key is rendered from — a number, an object, a string, a member
//! the event does not carry, and a target carrying no `KinesisParameters` at all —
//! against two events whose selected values all differ, so a key resolved once per
//! target instead of once per event cannot pass.
//!
//! The harness (the router with EventBridge, Kinesis, SQS, IAM and CloudWatch on
//! one engine, and the queue and role fixtures) is the one `target_dispatch` owns.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::SERVICE_PRINCIPAL;
use service_com_amazonaws_kinesis_kinesis_20131202::state::SHARD_ITERATOR_TYPE_TRIM_HORIZON;

use crate::target_dispatch::{
    drain, emulator, events, given_queue, given_role, given_stream, kinesis, Emulator, DETAIL_TYPE,
    SOURCE,
};

/// The stream every row below points its targets at, and the rule that matches.
const STREAM_NAME: &str = "keyed-telemetry";
const RULE_NAME: &str = "to-the-stream";

/// The action a Kinesis data stream target's invocation performs, and so the one
/// the target's role has to be allowed to make.
const TARGET_ACTION: &str = "kinesis:PutRecord";

/// How long the delivery of one publication is given to reach the stream before a
/// row calls it lost, and how long a poll sleeps between reads.
const DELIVERY_BUDGET: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One published event: the `Detail` it carries and the partition keys the paths
/// below select out of it, as the JSON text a key carries.
struct Event {
    detail: &'static str,
    /// The JSON text of `$.detail.n`, a number.
    number: &'static str,
    /// The COMPACT JSON text of `$.detail.o`, an object: no spaces.
    object: &'static str,
    /// The JSON text of `$.detail.m`, a string: quoted.
    marker: &'static str,
}

/// The two publications every row makes. Every selected value differs between
/// them, and each object's members are written in the order their names sort in,
/// so the expected text is the one compact rendering either way.
const EVENTS: [Event; 2] = [
    Event {
        detail: r#"{"m":"alpha","n":7,"o":{"a":1,"b":[2]}}"#,
        number: "7",
        object: r#"{"a":1,"b":[2]}"#,
        marker: r#""alpha""#,
    },
    Event {
        detail: r#"{"m":"omega","n":-42.5,"o":{"a":[3,{"c":4}],"b":"two"}}"#,
        number: "-42.5",
        object: r#"{"a":[3,{"c":4}],"b":"two"}"#,
        marker: r#""omega""#,
    },
];

/// One target of the rule: its `Id`, the `PartitionKeyPath` it carries (`None` for
/// a target carrying no `KinesisParameters`) and the partition key its record is
/// expected under for a given event.
struct Row {
    case: &'static str,
    id: &'static str,
    path: Option<&'static str>,
    key: fn(&Event, &str) -> String,
}

const ROWS: [Row; 5] = [
    Row {
        case: "a path selecting a number",
        id: "num",
        path: Some("$.detail.n"),
        key: |event, _event_id| event.number.to_string(),
    },
    Row {
        case: "a path selecting an object",
        id: "obj",
        path: Some("$.detail.o"),
        key: |event, _event_id| event.object.to_string(),
    },
    Row {
        case: "a path selecting a string",
        id: "marker",
        path: Some("$.detail.m"),
        key: |event, _event_id| event.marker.to_string(),
    },
    Row {
        case: "a path selecting a member the event does not carry",
        id: "absent",
        path: Some("$.detail.absent"),
        key: |_event, _event_id| "{}".to_string(),
    },
    Row {
        case: "no KinesisParameters at all",
        id: "dflt",
        path: None,
        key: |_event, event_id| event_id.to_string(),
    },
];

/// One row of the table: the bus the rule lives on (`None` for the account's
/// default bus) and how many shards the stream's records are spread over.
struct Placement {
    case: &'static str,
    bus: Option<&'static str>,
    shards: i64,
}

/// A reader of every shard of the stream, each positioned at that shard's oldest
/// retained record — the position a consumer starts a stream it has read nothing
/// of from.
async fn open_reader(emulator: &Emulator, case: &str) -> Vec<String> {
    let listed = kinesis(
        emulator,
        case,
        "ListShards",
        &json!({ "StreamName": STREAM_NAME }),
    )
    .await;
    let shards = listed["Shards"].as_array().cloned().unwrap_or_default();
    assert!(
        !shards.is_empty(),
        "case {case}: the stream has at least one shard to read, got {listed}"
    );
    let mut iterators = Vec::new();
    for shard in shards {
        let shard_id = shard["ShardId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a shard carries no ShardId: {shard}"));
        let answer = kinesis(
            emulator,
            case,
            "GetShardIterator",
            &json!({
                "StreamName": STREAM_NAME,
                "ShardId": shard_id,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await;
        iterators.push(
            answer["ShardIterator"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: no ShardIterator for {shard_id}"))
                .to_string(),
        );
    }
    iterators
}

/// Read each shard once from where the reader stands, advance the reader to the
/// `NextShardIterator` each read answered, and give back the records that read
/// served as (partition key, data text) pairs.
async fn read_once(emulator: &Emulator, case: &str, reader: &mut [String]) -> Vec<(String, String)> {
    let mut served = Vec::new();
    for iterator in reader.iter_mut() {
        let answer = kinesis(
            emulator,
            case,
            "GetRecords",
            &json!({ "ShardIterator": iterator.clone() }),
        )
        .await;
        for record in answer["Records"].as_array().cloned().unwrap_or_default() {
            let encoded = record["Data"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a record carries no Data: {record}"));
            let data = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case}: a record's Data is not base64 ({error}): {record}")
            });
            served.push((
                record["PartitionKey"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a record carries no PartitionKey: {record}")
                    })
                    .to_string(),
                String::from_utf8_lossy(&data).to_string(),
            ));
        }
        // An open shard always answers an iterator to continue from, which is the
        // only position the next read of this shard may start at.
        *iterator = answer["NextShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: an open shard answered no NextShardIterator"))
            .to_string();
    }
    served
}

/// Everything the reader is served until it has `want` records in hand, reading on
/// from where each read left it; gives up after [`DELIVERY_BUDGET`] so a delivery
/// that never arrives is a failed assertion rather than a hang.
async fn read_until(
    emulator: &Emulator,
    case: &str,
    reader: &mut [String],
    want: usize,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut served = Vec::new();
    loop {
        served.extend(read_once(emulator, case, reader).await);
        if served.len() >= want || Instant::now() >= deadline {
            return served;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Publish one event onto `bus` (the default bus when it names none) and answer
/// its `EventId`.
async fn publish(emulator: &Emulator, case: &str, bus: Option<&str>, detail: &str) -> String {
    let mut entry = json!({ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": detail });
    if let Some(bus) = bus {
        entry["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": [entry] })).await;
    assert_eq!(
        status, 200,
        "case {case}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case}: PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no EventId in {body}"))
        .to_string()
}

/// The rule of this row, on the bus it names, carrying one target per [`ROWS`]
/// entry — all of them the same stream, differing in the path they key on.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: Option<&str>,
    stream_arn: &str,
    role_arn: &str,
    dead_letter_arn: &str,
) {
    if let Some(bus) = bus {
        let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateEventBus answered {status} {body}"
        );
    }
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let mut rule = json!({ "Name": RULE_NAME, "EventPattern": pattern, "State": "ENABLED" });
    if let Some(bus) = bus {
        rule["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutRule", &rule).await;
    assert_eq!(status, 200, "case {case}: PutRule answered {status} {body}");

    let targets: Vec<Value> = ROWS
        .iter()
        .map(|row| {
            let mut target = json!({
                "Id": row.id,
                "Arn": stream_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
                "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
            });
            if let Some(path) = row.path {
                target["KinesisParameters"] = json!({ "PartitionKeyPath": path });
            }
            target
        })
        .collect();
    let mut put = json!({ "Rule": RULE_NAME, "Targets": targets });
    if let Some(bus) = bus {
        put["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutTargets", &put).await;
    assert_eq!(
        status, 200,
        "case {case}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case}: PutTargets reported a failed entry: {body}"
    );
}

/// The partition key each target's record is expected under, named by the row it
/// comes from so a mismatch says which target's rendering is wrong.
fn expectations(event: &Event, event_id: &str) -> Vec<(&'static str, String)> {
    ROWS.iter()
        .map(|row| (row.case, (row.key)(event, event_id)))
        .collect()
}

/// Those same keys as a count per key — two targets may key their records the
/// same way, so what a key must hold is a count rather than a presence.
fn expected_keys(expectations: &[(&'static str, String)]) -> BTreeMap<String, usize> {
    let mut wanted: BTreeMap<String, usize> = BTreeMap::new();
    for (_, key) in expectations {
        *wanted.entry(key.clone()).or_default() += 1;
    }
    wanted
}

/// What a read actually served, as a count per partition key.
fn served_keys(served: &[(String, String)]) -> BTreeMap<String, usize> {
    let mut held: BTreeMap<String, usize> = BTreeMap::new();
    for (key, _) in served {
        *held.entry(key.clone()).or_default() += 1;
    }
    held
}

/// A record's data is the envelope the target was delivered: this event's id, the
/// source and detail type it was published with, and its detail as published.
fn assert_delivered_envelope(case: &str, data: &str, event_id: &str, detail: &str) {
    let parsed: Value = serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("case {case}: a record's data is not JSON ({error})"));
    assert_eq!(
        parsed["id"],
        json!(event_id),
        "case {case}: expected the record to carry event {event_id}, got {parsed}"
    );
    assert_eq!(
        parsed["source"],
        json!(SOURCE),
        "case {case}: expected the published source on the record, got {parsed}"
    );
    assert_eq!(
        parsed["detail-type"],
        json!(DETAIL_TYPE),
        "case {case}: expected the published detail type on the record, got {parsed}"
    );
    assert_eq!(
        parsed["detail"],
        serde_json::from_str::<Value>(detail).expect("the published detail is JSON"),
        "case {case}: expected the published detail on the record, got {parsed}"
    );
}

/// Every Kinesis target of a matched rule is served to a consumer of the stream
/// exactly once per event, keyed by what that target's path selects out of the
/// event, whatever bus the rule lives on and however many shards the stream has.
#[tokio::test]
async fn a_consumer_reads_every_kinesis_targets_record_exactly_once() {
    let placements = [
        Placement {
            case: "a bus the client created, one shard",
            bus: Some("orders-bus"),
            shards: 1,
        },
        Placement {
            case: "a bus the client created, three shards",
            bus: Some("orders-bus"),
            shards: 3,
        },
        Placement {
            case: "the account's default bus, two shards",
            bus: None,
            shards: 2,
        },
    ];

    for placement in placements {
        let case = placement.case;
        let emulator = emulator();
        let stream_arn = given_stream(&emulator, case, STREAM_NAME, placement.shards).await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "stream-dead-letters").await;
        let role_arn = given_role(
            &emulator,
            "stream-writer",
            SERVICE_PRINCIPAL,
            Some(TARGET_ACTION),
        )
        .await;
        given_rule(
            &emulator,
            case,
            placement.bus,
            &stream_arn,
            &role_arn,
            &dead_letter_arn,
        )
        .await;

        // One reader, opened before anything is published and carried across both
        // publications: what it is served is what a consumer polling the stream
        // sees, and a record served twice shows up as a count that is too high.
        let mut reader = open_reader(&emulator, case).await;
        for event in &EVENTS {
            let event_id = publish(&emulator, case, placement.bus, event.detail).await;
            let served = read_until(&emulator, case, &mut reader, ROWS.len()).await;
            let expected = expectations(event, &event_id);
            assert_eq!(
                served_keys(&served),
                expected_keys(&expected),
                "case {case:?}: expected one record per target under the key its path selects \
                 ({expected:?}), got {served:?}"
            );
            for (key, data) in &served {
                assert_delivered_envelope(
                    &format!("{case} under the partition key {key:?}"),
                    data,
                    &event_id,
                    event.detail,
                );
            }
            // The reader stands past everything this publication wrote, so reading
            // on serves it nothing again until the next event is published.
            let again = read_once(&emulator, case, &mut reader).await;
            assert!(
                again.is_empty(),
                "case {case:?}: a record a reader was already served is not served again, got \
                 {again:?}"
            );
        }

        let dead_letters = drain(&emulator, &dead_letter_url).await;
        assert!(
            dead_letters.is_empty(),
            "case {case:?}: a delivery that succeeded dead-letters nothing, got {dead_letters:?}"
        );
    }
}
