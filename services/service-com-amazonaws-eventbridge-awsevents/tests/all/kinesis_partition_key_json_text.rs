// SPDX-License-Identifier: CC0-1.0

//! What a Kinesis data stream target's record is KEYED on, read back where a
//! consumer reads it: over the wire through the real `MultiServiceRouter`, with
//! Kinesis mounted beside EventBridge on one engine, through `GetShardIterator`
//! and `GetRecords`.
//!
//! The RULE asserted here: the partition key of the record a Kinesis data stream
//! target is delivered is the JSON TEXT of the ONE value that target's
//! `KinesisParameters$PartitionKeyPath` selects out of the delivered envelope —
//! whatever JSON type that value has, rendered compactly — and a path that selects
//! no single value does not fail the delivery: the record is written all the same,
//! keyed on the empty JSON object. "The JSON path to be extracted from the event
//! and used as the partition key for the Amazon Kinesis data stream, so that you
//! can control the shard to which the event goes. If you do not include this
//! parameter, the default is to use the eventId as the partition key"
//! (`KinesisParameters$PartitionKeyPath`).
//!
//! The rows span the rule along the TYPE and FORM of the selected value — a
//! number (integer, negative fractional, zero, an integer past what a double
//! holds exactly), a boolean, a null, a string (plain, one carrying characters
//! JSON escapes, one carrying non-ASCII text), an empty object, an empty array,
//! a populated object, a nested array, a value below an array element — beside
//! the two rows that select no single value (a member the event does not carry,
//! and a target carrying no `KinesisParameters` at all). `$.detail.n` and
//! `$.detail.o` are the two cases the recorded instance covers; every other row
//! is the same rule on a value form it did not.
//!
//! Because the key is what "you control the shard to which the event goes" with,
//! each record is also asserted to sit in the shard whose published
//! `HashKeyRange` covers that key's hash ("an MD5 hash function is used to map
//! partition keys to 128-bit integer values and to map associated data records to
//! shards", `PutRecordInput$PartitionKey`) — which is what makes the rendering
//! observable as shard placement rather than as an opaque string.
//!
//! `kinesis_target_delivery` owns the path-FORM table and the role half of the
//! same rule; `kinesis_target_stream_consumption` owns the consumer's
//! read-exactly-once half; `kinesis_target_shard_affinity` owns the placement half
//! over shard maps of other sizes and over events whose selected values differ.
//! The harness is the one `target_dispatch` owns.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{MAX_TARGETS_PER_RULE, SERVICE_PRINCIPAL};

use crate::target_dispatch::{
    covering_shard, drain, emulator, given_queue, given_role, given_rule, given_stream,
    publish_details, read_shards_until, shard_map, Held, DETAIL_TYPE, SOURCE,
};

/// The stream every target below names, and the rules the targets are spread over.
const STREAM_NAME: &str = "keyed-by-json-text";
const RULE_NAME: &str = "to-the-stream";

/// The action a Kinesis data stream target's invocation performs, and so the one
/// the target's role has to be allowed to make.
const TARGET_ACTION: &str = "kinesis:PutRecord";

/// More than one shard, because the partition key is what chooses among them: on a
/// single-shard stream every record lands in the one shard whatever key it carries.
const SHARD_COUNT: i64 = 4;

/// The `Detail` the one published event carries: one member per JSON value form
/// the rows below select.
fn detail() -> String {
    json!({
        "n": 7,
        "big": 9_007_199_254_740_993_i64,
        "frac": -42.5,
        "zero": 0,
        "flag": true,
        "nothing": null,
        "s": "alpha",
        "quoted": "he said \"hi\"",
        "accented": "naïve",
        "hollow": {},
        "none": [],
        "o": { "a": 1, "b": [2] },
        "nested": [1, [2], { "c": 3 }],
        "items": [{ "sku": "A-1" }, { "sku": "B-2" }],
    })
    .to_string()
}

/// One row: the target's `Id`, the `PartitionKeyPath` it carries (`None` for a
/// target carrying no `KinesisParameters`) and the partition key its record is
/// expected under for the published event.
struct Row {
    case: &'static str,
    id: &'static str,
    path: Option<&'static str>,
    key: fn(&str) -> String,
}

/// The rows, each naming the JSON text its path's value renders as. The texts are
/// written out rather than derived, so a rendering that drifts — a number through
/// language-native formatting, an object with spaces in it, an escape dropped —
/// fails the row that names it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a path selecting an integer",
            id: "int",
            path: Some("$.detail.n"),
            key: |_event_id| "7".to_string(),
        },
        Row {
            case: "a path selecting an integer past what a double holds exactly",
            id: "big",
            path: Some("$.detail.big"),
            key: |_event_id| "9007199254740993".to_string(),
        },
        Row {
            case: "a path selecting a negative fractional number",
            id: "frac",
            path: Some("$.detail.frac"),
            key: |_event_id| "-42.5".to_string(),
        },
        Row {
            case: "a path selecting zero",
            id: "zero",
            path: Some("$.detail.zero"),
            key: |_event_id| "0".to_string(),
        },
        Row {
            case: "a path selecting a boolean",
            id: "flag",
            path: Some("$.detail.flag"),
            key: |_event_id| "true".to_string(),
        },
        Row {
            case: "a path selecting a null",
            id: "null",
            path: Some("$.detail.nothing"),
            key: |_event_id| "null".to_string(),
        },
        Row {
            case: "a path selecting a string",
            id: "str",
            path: Some("$.detail.s"),
            // A string's JSON text is quoted.
            key: |_event_id| "\"alpha\"".to_string(),
        },
        Row {
            case: "a path selecting a string carrying characters JSON escapes",
            id: "quoted",
            path: Some("$.detail.quoted"),
            key: |_event_id| r#""he said \"hi\"""#.to_string(),
        },
        Row {
            case: "a path selecting a string carrying non-ASCII text",
            id: "accented",
            path: Some("$.detail.accented"),
            key: |_event_id| "\"naïve\"".to_string(),
        },
        Row {
            case: "a path selecting an empty object",
            id: "hollow",
            path: Some("$.detail.hollow"),
            key: |_event_id| "{}".to_string(),
        },
        Row {
            case: "a path selecting an empty array",
            id: "none",
            path: Some("$.detail.none"),
            key: |_event_id| "[]".to_string(),
        },
        Row {
            case: "a path selecting an object",
            id: "obj",
            path: Some("$.detail.o"),
            // Compact JSON text: no spaces.
            key: |_event_id| r#"{"a":1,"b":[2]}"#.to_string(),
        },
        Row {
            case: "a path selecting an array of mixed values",
            id: "nested",
            path: Some("$.detail.nested"),
            key: |_event_id| r#"[1,[2],{"c":3}]"#.to_string(),
        },
        Row {
            case: "a path selecting a member below an array element",
            id: "elem",
            path: Some("$.detail.items[1].sku"),
            key: |_event_id| "\"B-2\"".to_string(),
        },
        Row {
            case: "a path selecting a member the event does not carry",
            id: "absent",
            path: Some("$.detail.missing"),
            // Not a failed delivery: the record is written under the empty JSON
            // object.
            key: |_event_id| "{}".to_string(),
        },
        Row {
            case: "no KinesisParameters at all",
            id: "dflt",
            path: None,
            key: |event_id| event_id.to_string(),
        },
    ]
}

/// A record's data is the envelope the target was delivered: the published source,
/// detail type and detail, under this event's id.
fn assert_delivered_envelope(case: &str, data: &str, event_id: &str) {
    let parsed: Value = serde_json::from_str(data)
        .unwrap_or_else(|error| panic!("case {case:?}: a record's data is not JSON ({error})"));
    for (member, expected) in [
        ("id", json!(event_id)),
        ("source", json!(SOURCE)),
        ("detail-type", json!(DETAIL_TYPE)),
        (
            "detail",
            serde_json::from_str::<Value>(&detail()).expect("the published detail is JSON"),
        ),
    ] {
        assert_eq!(
            parsed[member], expected,
            "case {case:?}: expected the record's {member} to be the published {expected}, got \
             {parsed}"
        );
    }
}

/// Every Kinesis target's record is keyed on the JSON text of the value its path
/// selects, whatever type that value has, carries the delivered envelope, and
/// lands in the shard that key's hash falls in — with nothing dead-lettered.
#[tokio::test]
async fn a_kinesis_targets_record_is_keyed_on_the_json_text_of_what_its_path_selects() {
    let emulator = emulator();
    let case = "a key per JSON value form";

    let stream_arn = given_stream(&emulator, case, STREAM_NAME, SHARD_COUNT).await;
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
                "RetryPolicy": { "MaximumRetryAttempts": 0, "MaximumEventAgeInSeconds": 60 },
            });
            if let Some(path) = row.path {
                target["KinesisParameters"] = json!({ "PartitionKeyPath": path });
            }
            target
        })
        .collect();
    // A rule holds at most `MAX_TARGETS_PER_RULE` targets, so the rows are spread
    // over as many rules as they need; every one of them matches the published
    // event, so each target is delivered to once.
    for (index, chunk) in targets.chunks(MAX_TARGETS_PER_RULE).enumerate() {
        given_rule(
            &emulator,
            &format!("{RULE_NAME}-{index}"),
            Value::Array(chunk.to_vec()),
        )
        .await;
    }

    let event_ids = publish_details(&emulator, &[detail()]).await;
    let event_id = event_ids
        .first()
        .unwrap_or_else(|| panic!("case {case}: PutEvents reported no EventId"));

    let shards = shard_map(&emulator, case, STREAM_NAME, SHARD_COUNT).await;
    let held = read_shards_until(&emulator, case, STREAM_NAME, &shards, rows.len()).await;

    // Grouped by partition key: two rows may key their records the same way (a
    // path selecting an empty object and one selecting nothing both do), so what a
    // key must hold is a COUNT rather than the presence of one record.
    let mut by_key: BTreeMap<String, Vec<Held>> = BTreeMap::new();
    for record in &held {
        by_key
            .entry(record.key.clone())
            .or_default()
            .push(record.clone());
    }
    for row in &rows {
        let expected = (row.key)(event_id);
        let wanted = rows
            .iter()
            .filter(|other| (other.key)(event_id) == expected)
            .count();
        let written = by_key.get(&expected).cloned().unwrap_or_default();
        assert_eq!(
            written.len(),
            wanted,
            "case {:?}: expected {wanted} record(s) under the partition key {expected:?}, got \
             {written:?} under it and {held:?} in all",
            row.case
        );
        for record in &written {
            assert_delivered_envelope(row.case, &record.data, event_id);
            // "All data records with the same partition key map to the same shard
            // within the stream": the shard that served the record is the one
            // whose published range covers that key's hash, so the key really is
            // what chose the shard.
            assert_eq!(
                covering_shard(&shards, &record.key).as_deref(),
                Some(record.shard.as_str()),
                "case {:?}: expected the record keyed {expected:?} in the shard covering its \
                 hash, got it in {:?}",
                row.case,
                record.shard
            );
        }
    }
    assert_eq!(
        held.len(),
        rows.len(),
        "case {case}: expected one record per target and no others, got {held:?}"
    );

    let dead_letters = drain(&emulator, &dead_letter_url).await;
    assert!(
        dead_letters.is_empty(),
        "case {case}: a delivery that succeeded dead-letters nothing, got {dead_letters:?}"
    );
}
