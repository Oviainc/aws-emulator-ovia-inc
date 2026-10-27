// SPDX-License-Identifier: CC0-1.0

//! How a Kinesis data stream DISTRIBUTES the records written to it when the SERVICE places
//! them, end to end over the wire: `CreateStream` (on-demand, `RecordDistributionStrategy`
//! `AUTO`) -> `PutRecord` / `PutRecords` -> `GetShardIterator` -> `GetRecords`.
//!
//! RULE: for a stream whose record distribution strategy is `AUTO` the service places every
//! record itself — "the partition key is optional and any value you provide is ignored,
//! along with any ExplicitHashKey you provide. In that case, Amazon Kinesis Data Streams
//! distributes the record across shards using service-managed algorithms"
//! (`PutRecordInput$PartitionKey`) — so the records a producer writes are spread over the
//! stream's open shards whether they arrive one at a time or as one batch, and a partition
//! key the producer did supply is still reported back on the record a reader is served
//! ("the original value is returned even though it was not used to determine shard
//! placement", `Record$PartitionKey`).
//!
//! The rows differ along what the rule generalises over: whether the records arrive as
//! single writes or as one batch, and whether the producer supplies a partition key at all.
//! The `USER_PARTITION_KEY` default — the strategy under which the producer's key places the
//! record — is the subject of `data_records.rs`, which this suite deliberately does not
//! repeat.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses
//! and bodies asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    RECORD_DISTRIBUTION_AUTO, SHARD_ITERATOR_TYPE_TRIM_HORIZON, STREAM_MODE_ON_DEMAND,
};

// The one wire harness this crate's suites share — a router carrying exactly this service
// over one shared engine, with the request-signing and error-body helpers around it.
use crate::stream_tags::{past_activation, Emulator};

/// The `Data` member as it travels on the wire: a blob is base64-encoded when it is
/// serialized.
fn data(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// An on-demand stream whose records the SERVICE places, past the transition the create
/// armed, answering the ids of the shards the create allocated.
///
/// "The record distribution strategy is only supported for streams that use the on-demand
/// capacity mode" (`CreateStreamInput$RecordDistributionStrategy`), and an on-demand
/// stream's shard count is the service's to manage, so the create names neither.
async fn managed_stream(emulator: &Emulator, case: &str, name: &str) -> Vec<String> {
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": name,
                "StreamModeDetails": {"StreamMode": STREAM_MODE_ON_DEMAND},
                "RecordDistributionStrategy": RECORD_DISTRIBUTION_AUTO,
            }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let listed = emulator
        .ok(case, "ListShards", &json!({"StreamName": name}))
        .await;
    let shards: Vec<String> = listed["Shards"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListShards reported no Shards"))
        .iter()
        .map(|shard| {
            shard["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a shard carries no ShardId"))
                .to_string()
        })
        .collect();
    assert!(
        shards.len() > 1,
        "case {case}: an on-demand stream is created with more than one shard, got {shards:?}"
    );
    shards
}

/// Write one record, answering the shard id and sequence number `PutRecord` reports.
async fn write_one(emulator: &Emulator, case: &str, body: &Value) -> (String, String) {
    let answer = emulator.ok(case, "PutRecord", body).await;
    let shard = answer["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: PutRecord answered no ShardId, got {answer}"))
        .to_string();
    let sequence_number = answer["SequenceNumber"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: PutRecord answered no SequenceNumber, got {answer}")
        })
        .to_string();
    (shard, sequence_number)
}

/// Write `entries` as one batch, answering the shard id of each stored entry in the input's
/// order.
async fn write_batch(emulator: &Emulator, case: &str, stream: &str, entries: Vec<Value>) -> Vec<String> {
    let answer = emulator
        .ok(
            case,
            "PutRecords",
            &json!({"StreamName": stream, "Records": entries}),
        )
        .await;
    assert_eq!(
        answer["FailedRecordCount"].as_i64(),
        Some(0),
        "case {case}: a batch every record of which was stored reports a failed count of 0, got \
         {answer}"
    );
    answer["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: PutRecords answered no Records, got {answer}"))
        .iter()
        .map(|result| {
            result["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a stored entry names its shard"))
                .to_string()
        })
        .collect()
}

/// The records `shard` serves from its oldest retained one, as (sequence number, partition
/// key, data) triples.
async fn records_of(
    emulator: &Emulator,
    case: &str,
    stream: &str,
    shard: &str,
) -> Vec<(String, Option<String>, Vec<u8>)> {
    let iterator = emulator
        .ok(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator"))
        .to_string();
    let answer = emulator
        .ok(case, "GetRecords", &json!({"ShardIterator": iterator}))
        .await;
    answer["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: GetRecords answered no Records, got {answer}"))
        .iter()
        .map(|record| {
            let sequence_number = record["SequenceNumber"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let partition_key = record["PartitionKey"].as_str().map(str::to_string);
            let data = aws_smithy_types::base64::decode(
                record["Data"].as_str().unwrap_or_default(),
            )
            .unwrap_or_else(|error| {
                panic!("case {case}: a record's Data must be base64: {error}")
            });
            (sequence_number, partition_key, data)
        })
        .collect()
}

/// The records a stream the service distributes itself takes are spread over its open
/// shards, whether they arrive one at a time or as one batch.
#[tokio::test]
async fn a_stream_the_service_distributes_spreads_its_records_over_its_shards() {
    // (case, how the records are written)
    for case in ["one write at a time", "one batch"] {
        let emulator = Emulator::new();
        let stream = "records-managed-distribution";
        let shards = managed_stream(&emulator, case, stream).await;
        // Twice as many records as there are shards, so an even spread uses every one of
        // them whichever way the records are written.
        let count = shards.len() * 2;

        let written: Vec<String> = if case == "one batch" {
            // No PartitionKey at all, which only a stream the service distributes itself
            // admits.
            let entries: Vec<Value> = (0..count)
                .map(|_| json!({"Data": data(b"managed")}))
                .collect();
            write_batch(&emulator, case, stream, entries).await
        } else {
            let mut placed = Vec::new();
            for _ in 0..count {
                let (shard, _) = write_one(
                    &emulator,
                    case,
                    &json!({"StreamName": stream, "Data": data(b"managed")}),
                )
                .await;
                placed.push(shard);
            }
            placed
        };

        let mut distinct = written.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct, shards,
            "case {case}: the records are spread over every open shard the create \
             allocated, got {written:?}"
        );
    }
}

/// A partition key a producer supplies to such a stream places nothing, and is still
/// reported back on the record a reader is served.
#[tokio::test]
async fn a_partition_key_a_managed_stream_ignores_is_still_reported_back() {
    let emulator = Emulator::new();
    let case = "reported back";
    let stream = "records-managed-partition-key";
    let shards = managed_stream(&emulator, case, stream).await;

    // (case, the members the write carries beside its data)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        ("no partition key at all", json!({}), None),
        (
            "a partition key the strategy ignores",
            json!({"PartitionKey": "pk-reported-but-unused"}),
            Some("pk-reported-but-unused"),
        ),
        (
            "a partition key and an explicit hash key, both ignored",
            json!({"PartitionKey": "pk-with-hash-key", "ExplicitHashKey": "0"}),
            Some("pk-with-hash-key"),
        ),
    ];
    for (row, members, expected_key) in rows {
        let mut body = members;
        body["StreamName"] = json!(stream);
        body["Data"] = data(row.as_bytes());
        let (shard, sequence_number) = write_one(&emulator, row, &body).await;
        assert!(
            shards.contains(&shard),
            "case {row}: the record is placed on a shard the create allocated, got {shard}"
        );

        let served = records_of(&emulator, row, stream, &shard).await;
        let record = served
            .iter()
            .find(|(number, _, _)| number == &sequence_number)
            .unwrap_or_else(|| {
                panic!("case {row}: the record written is served from the shard it was placed on")
            });
        assert_eq!(
            record.1.as_deref(),
            expected_key,
            "case {row}: expected the partition key the producer supplied, got {record:?}"
        );
        assert_eq!(
            record.2,
            row.as_bytes().to_vec(),
            "case {row}: the data is served back byte for byte, got {record:?}"
        );
    }
}
