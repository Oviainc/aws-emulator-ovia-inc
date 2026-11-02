// SPDX-License-Identifier: CC0-1.0

//! What a BATCH write reports about its own failures: `PutRecords` ->
//! `FailedRecordCount`.
//!
//! RULE: a batch write answers how many of ITS entries failed on every accepted request —
//! the count is always stated, and it is exactly the number of result entries carrying an
//! `ErrorCode`, so a batch every entry of which was stored reports zero rather than
//! reporting nothing. The count describes one request only: it never accumulates over the
//! writes a stream has taken, and it is reported whichever way the request names its
//! stream. A request that is not accepted carries no count: one naming no stream at all is
//! refused `InvalidArgumentException` in the single sentence the service words that refusal
//! in, whichever operation it addresses.
//!
//! The recorded flow this was derived from writes ONE small entry to a one-shard stream and
//! reads the count back, which is one row of the table below. The rows differ along what
//! the rule generalises over: how many entries the batch carries (one, a few, and the
//! modelled maximum of `Records`), the bytes an entry carries (empty, text, binary), how
//! many partition keys the entries spread over, how many shards the stream has, and which
//! member spells the stream (`StreamName` alone, `StreamARN` alone, both together).
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses
//! and bodies asserted here are the ones a client sees.

use serde_json::{json, Value};

// The one wire harness this crate's suites share — a router carrying exactly this service
// over one shared engine, with the signing and error-body helpers around it.
use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// The `@length` maximum of `PutRecordsInput$Records`
/// (`smithy_query(query="operation",operation="PutRecords")` -> `Records` len 1..500): the
/// largest batch the operation admits at all.
const MAX_BATCH_RECORDS: usize = 500;

/// How one table row spells the stream it writes to. The operation's doc publishes all
/// three: "you must use either the StreamARN or the StreamName parameter, or both".
#[derive(Debug, Clone, Copy)]
enum Spelling {
    Name,
    Arn,
    Both,
}

impl Spelling {
    /// Every spelling a row is driven under.
    fn all() -> [Spelling; 3] {
        [Spelling::Name, Spelling::Arn, Spelling::Both]
    }

    /// The stream-identifying members of a request body under this spelling.
    fn members(self, name: &str) -> Vec<(&'static str, String)> {
        let arn = stream_arn(REGION, name);
        match self {
            Spelling::Name => vec![("StreamName", name.to_string())],
            Spelling::Arn => vec![("StreamARN", arn)],
            Spelling::Both => vec![("StreamName", name.to_string()), ("StreamARN", arn)],
        }
    }
}

/// One row of the table: a batch of entries, and the stream it is written to.
#[derive(Debug)]
struct Row {
    label: &'static str,
    /// How many shards the stream the row writes to has.
    shards: i64,
    /// The entries of the batch, as (partition key, data) pairs.
    entries: Vec<(String, Vec<u8>)>,
}

/// The payload shapes an entry's `Data` admits ("The data in the blob is both opaque and
/// immutable to Kinesis Data Streams", `PutRecordsRequestEntry$Data`): an empty blob, text,
/// and bytes that are not valid UTF-8 — here a GZIP-framed payload, whose first two bytes
/// are the format's magic number.
fn payload_shapes() -> Vec<Vec<u8>> {
    vec![
        Vec::new(),
        b"a text event".to_vec(),
        vec![0x1f, 0x8b, 0x08, 0x00, 0xfe, 0xff, 0x00, 0x7f],
    ]
}

/// The rows the rule is asserted over, spanning batch size, payload shape, how many
/// partition keys the batch spreads over and how many shards the stream has.
fn rows() -> Vec<Row> {
    let shapes = payload_shapes();
    vec![
        Row {
            label: "one entry, one shard",
            shards: 1,
            entries: vec![("pk".to_string(), b"{\"id\":\"only\"}\n".to_vec())],
        },
        Row {
            label: "one empty blob",
            shards: 1,
            entries: vec![("pk".to_string(), Vec::new())],
        },
        Row {
            label: "every payload shape, one key",
            shards: 1,
            entries: shapes
                .iter()
                .map(|data| ("pk".to_string(), data.clone()))
                .collect(),
        },
        Row {
            label: "every payload shape, a key each, three shards",
            shards: 3,
            entries: shapes
                .iter()
                .enumerate()
                .map(|(index, data)| (format!("partition-{index}"), data.clone()))
                .collect(),
        },
        Row {
            label: "the modelled maximum batch, three shards",
            shards: 3,
            entries: (0..MAX_BATCH_RECORDS)
                .map(|index| {
                    (
                        format!("partition-{}", index % 7),
                        format!("entry {index}").into_bytes(),
                    )
                })
                .collect(),
        },
    ]
}

/// The request body a row's batch is written with, under one spelling.
fn body(row: &Row, name: &str, spelling: Spelling) -> Value {
    let mut request = serde_json::Map::new();
    for (member, value) in spelling.members(name) {
        request.insert(member.to_string(), Value::String(value));
    }
    request.insert(
        "Records".to_string(),
        Value::Array(
            row.entries
                .iter()
                .map(|(partition_key, data)| {
                    json!({
                        "PartitionKey": partition_key,
                        "Data": aws_smithy_types::base64::encode(data),
                    })
                })
                .collect(),
        ),
    );
    Value::Object(request)
}

/// Assert the rule over one answer: the count is stated, and it is the number of result
/// entries that carry an `ErrorCode`. Answers that count.
fn assert_count_matches_the_entries(case: &str, expected_entries: usize, answer: &Value) -> i64 {
    let results = answer["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: PutRecords answered no Records, got {answer}"));
    assert_eq!(
        results.len(),
        expected_entries,
        "case {case}: expected one result entry per request record ({expected_entries}), got \
         {} in {answer}",
        results.len()
    );
    let failed_entries = results
        .iter()
        .filter(|entry| !entry["ErrorCode"].is_null())
        .count() as i64;
    let reported = answer["FailedRecordCount"].as_i64().unwrap_or_else(|| {
        panic!(
            "case {case}: expected PutRecords to report a FailedRecordCount, got \
             {:?} in {answer}",
            answer["FailedRecordCount"]
        )
    });
    assert_eq!(
        reported, failed_entries,
        "case {case}: expected FailedRecordCount to be the number of result entries carrying an \
         ErrorCode ({failed_entries}), got {reported} in {answer}"
    );
    reported
}

/// Every accepted batch states its failure count, and a batch every entry of which was
/// stored states zero — under each spelling of the stream, each batch size and each payload
/// shape.
#[tokio::test]
async fn an_accepted_batch_always_reports_its_failure_count() {
    let emulator = Emulator::new();
    // The two shard counts the rows need, created together so the cases wait for the
    // CREATING -> ACTIVE transition once.
    let one_shard = "batch-count-one-shard";
    let three_shards = "batch-count-three-shards";
    for (name, shards) in [(one_shard, 1), (three_shards, 3)] {
        emulator
            .ok(
                "setup",
                "CreateStream",
                &json!({"StreamName": name, "ShardCount": shards}),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for row in rows() {
        let name = if row.shards == 1 { one_shard } else { three_shards };
        for spelling in Spelling::all() {
            let case = format!("{} / {spelling:?}", row.label);
            let answer = emulator
                .ok(&case, "PutRecords", &body(&row, name, spelling))
                .await;
            let reported = assert_count_matches_the_entries(&case, row.entries.len(), &answer);
            assert_eq!(
                reported, 0,
                "case {case}: expected a batch every entry of which was stored to report 0, got \
                 {reported} in {answer}"
            );
            assert!(
                answer.get("EncryptionType").is_none(),
                "case {case}: a stream nothing encrypted applies no encryption to a batch, so \
                 the answer names no encryption type, got {answer}"
            );
            for (index, entry) in answer["Records"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .enumerate()
            {
                assert!(
                    entry["ShardId"].is_string() && entry["SequenceNumber"].is_string(),
                    "case {case}: a stored entry names its shard and sequence number, entry \
                     {index} is {entry}"
                );
                assert!(
                    entry["ErrorCode"].is_null() && entry["ErrorMessage"].is_null(),
                    "case {case}: a stored entry carries no per-record error, entry {index} is \
                     {entry}"
                );
            }
        }
    }
}

/// The count describes ONE request: writing the same batch again reports zero again rather
/// than a count that grew with what the stream already holds.
#[tokio::test]
async fn the_failure_count_describes_one_request_only() {
    let emulator = Emulator::new();
    let name = "batch-count-repeated";
    emulator
        .ok(
            "repeated",
            "CreateStream",
            &json!({"StreamName": name, "ShardCount": 2}),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let row = Row {
        label: "repeated",
        shards: 2,
        entries: (0..4)
            .map(|index| (format!("partition-{index}"), format!("pass {index}").into_bytes()))
            .collect(),
    };
    for pass in 1..=3 {
        let case = format!("pass {pass}");
        let answer = emulator
            .ok(&case, "PutRecords", &body(&row, name, Spelling::Name))
            .await;
        let reported = assert_count_matches_the_entries(&case, row.entries.len(), &answer);
        assert_eq!(
            reported, 0,
            "case {case}: the count is this request's failures, not the stream's history, got \
             {reported} in {answer}"
        );
    }
}

/// A batch the stream cannot take at all is refused rather than answered with a count: the
/// count belongs to an accepted request, so a refusal carries an error body instead.
#[tokio::test]
async fn a_refused_batch_carries_an_error_rather_than_a_count() {
    let emulator = Emulator::new();
    let name = "batch-count-refused";
    emulator
        .ok(
            "refused",
            "CreateStream",
            &json!({"StreamName": name, "ShardCount": 1}),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let entries = vec![json!({
        "PartitionKey": "pk",
        "Data": aws_smithy_types::base64::encode(b"{\"id\":\"nowhere\"}"),
    })];
    for (case, request, expected) in [
        (
            "no stream named",
            json!({"Records": entries.clone()}),
            "InvalidArgumentException",
        ),
        (
            "a stream nothing created",
            json!({"StreamName": "batch-count-absent", "Records": entries.clone()}),
            "ResourceNotFoundException",
        ),
    ] {
        let (status, response) = emulator.call("PutRecords", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected PutRecords to refuse the batch with 400, got {status} with \
             body {response}"
        );
        assert_eq!(
            error_code(&response),
            expected,
            "case {case}: expected {expected}, got {response}"
        );
        assert!(
            response["FailedRecordCount"].is_null(),
            "case {case}: a refused batch reports no count, got {response}"
        );
    }
}

/// The sentence the service answers a request that carries neither stream identifier with:
/// fixed text, with nothing taken from the request.
const NEITHER_IDENTIFIER_MESSAGE: &str = "Stream arn and stream name can't be empty at the same time";

/// A request that names NO stream is refused the same way whichever operation it addresses:
/// `InvalidArgumentException` with the one sentence the service words that refusal in. The
/// rows span the data plane and the control plane, reads and writes.
#[tokio::test]
async fn a_request_naming_no_stream_is_refused_the_same_way_everywhere() {
    let emulator = Emulator::new();
    let data = aws_smithy_types::base64::encode(b"{\"id\":\"unaddressed\"}");
    for (operation, request) in [
        (
            "PutRecords",
            json!({"Records": [{"PartitionKey": "pk", "Data": data.clone()}]}),
        ),
        (
            "PutRecord",
            json!({"PartitionKey": "pk", "Data": data.clone()}),
        ),
        ("DescribeStreamSummary", json!({})),
        ("ListShards", json!({})),
        ("DeleteStream", json!({})),
        ("AddTagsToStream", json!({"Tags": {"owner": "batch"}})),
    ] {
        let (status, response) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected a request naming no stream to be refused with 400, got \
             {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {operation}: expected InvalidArgumentException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            NEITHER_IDENTIFIER_MESSAGE,
            "case {operation}: expected {NEITHER_IDENTIFIER_MESSAGE:?}, got {response}"
        );
    }
}
