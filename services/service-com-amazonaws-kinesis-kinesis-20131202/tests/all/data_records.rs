// SPDX-License-Identifier: CC0-1.0

//! The Kinesis DATA PLANE, end to end over the wire: `CreateStream` -> `PutRecord` /
//! `PutRecords` -> `GetShardIterator` -> `GetRecords`.
//!
//! RULE: a record written to a stream is stored in the one shard its partition key (or its
//! explicit hash key) maps to, under a sequence number of that shard, and `GetRecords`
//! answers the records stored at and after the position its iterator names — in the order
//! they were written, byte for byte, bounded by `Limit` — together with the iterator a
//! reader continues from and how far behind the newest record of the shard the page is.
//! Reading records changes nothing a later caller sees: the same iterator answers the same
//! page twice.
//!
//! The recorded flow this was derived from writes a small batch into a one-shard stream and
//! reads it back with ONE `TRIM_HORIZON` iterator, which is one row of the tables below.
//! The rows differ along what the rule generalises over: which iterator type names the
//! position, how many records are written and in what shape (one by one or as a batch), the
//! bytes a record carries (empty, text, binary), how many records one page may return, and
//! how many shards the partition keys are spread over.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    partition_key_hash, shard_id, ShardIteratorToken, ShardPosition,
    DEFAULT_MAX_RECORD_SIZE_KI_B, ENCRYPTION_TYPE_NONE,
    SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER, SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
    SHARD_ITERATOR_TYPE_AT_TIMESTAMP, SHARD_ITERATOR_TYPE_LATEST, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
    SHARD_ITERATOR_TTL,
};

// The one wire harness this crate's suites share — a router carrying exactly this service
// over one shared engine, with the request-signing and error-body helpers around it. The
// data-plane helpers this suite needs are added to it below rather than beside a second
// copy of it.
use crate::stream_tags::{error_code, error_message, past_activation, Emulator};

/// The payloads one table row writes, chosen along what `Record$Data` admits ("The data in
/// the blob is both opaque and immutable to Kinesis Data Streams, which does not inspect,
/// interpret, or change the data in the blob in any way", modelled `len 0..10485760`): an
/// empty blob, text, and bytes that are not valid UTF-8 — here a GZIP-framed payload, whose
/// first two bytes are the format's magic number.
fn payloads() -> Vec<Vec<u8>> {
    vec![
        Vec::new(),
        b"first event".to_vec(),
        vec![0x1f, 0x8b, 0x08, 0x00, 0xff, 0xfe, 0x00, 0x01],
        "a longer payload, written last".as_bytes().to_vec(),
    ]
}

/// The data-plane steps this suite drives, on the shared harness.
impl Emulator {
    /// Call `operation` expecting a client refusal, answering its error code and message.
    async fn refused(&self, case: &str, operation: &str, body: &Value) -> (String, String) {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected {operation} to answer 400, got {status} with body {response}"
        );
        (error_code(&response), error_message(&response))
    }

    /// A provisioned stream of `shard_count` shards, past the transition the create armed.
    async fn active_stream(&self, case: &str, name: &str, shard_count: i64) -> String {
        self.ok(
            case,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": shard_count }),
        )
        .await;
        tokio::time::sleep(past_activation()).await;
        let summary = self
            .ok(case, "DescribeStreamSummary", &json!({"StreamName": name}))
            .await;
        summary["StreamDescriptionSummary"]["StreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
            .to_string()
    }

    /// Write one record, answering the shard id and sequence number `PutRecord` reports.
    async fn put_record(
        &self,
        case: &str,
        stream: &str,
        partition_key: &str,
        data: &[u8],
    ) -> (String, String) {
        let answer = self
            .ok(
                case,
                "PutRecord",
                &json!({
                    "StreamName": stream,
                    "PartitionKey": partition_key,
                    "Data": aws_smithy_types::base64::encode(data),
                }),
            )
            .await;
        assert!(
            answer.get("EncryptionType").is_none(),
            "case {case}: a stream nothing encrypted applies no encryption to a write, so \
             the answer names no encryption type, got {answer}"
        );
        (
            answer["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: PutRecord reported no ShardId: {answer}"))
                .to_string(),
            answer["SequenceNumber"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: PutRecord reported no SequenceNumber: {answer}")
                })
                .to_string(),
        )
    }

    /// An iterator of `shard` in `stream`, of the type `iterator_type`.
    async fn iterator(&self, case: &str, body: &Value) -> String {
        let answer = self.ok(case, "GetShardIterator", body).await;
        answer["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator, got {answer}"))
            .to_string()
    }

    /// A `TRIM_HORIZON` iterator of `shard`, the position a reader starts a shard from.
    async fn horizon_iterator(&self, case: &str, stream: &str, shard: &str) -> String {
        self.iterator(
            case,
            &json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await
    }

    /// A `LATEST` iterator of `shard`: the position pinned just after the most recent
    /// record the shard holds when the iterator is handed out.
    async fn latest_iterator(&self, case: &str, stream: &str, shard: &str) -> String {
        self.iterator(
            case,
            &json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_LATEST,
            }),
        )
        .await
    }

    /// One `GetRecords` page: the records it answered, the iterator to continue from and
    /// the lag it reported.
    async fn get_records(&self, case: &str, body: &Value) -> Page {
        let answer = self.ok(case, "GetRecords", body).await;
        let records = answer["Records"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: GetRecords always answers a Records list, got {answer}")
            })
            .clone();
        let millis_behind_latest = answer["MillisBehindLatest"].as_i64().unwrap_or_else(|| {
            panic!("case {case}: GetRecords reports MillisBehindLatest, got {answer}")
        });
        assert!(
            millis_behind_latest >= 0,
            "case {case}: MillisBehindLatest is modelled range 0.., got {millis_behind_latest}"
        );
        Page {
            records,
            next_shard_iterator: answer["NextShardIterator"].as_str().map(str::to_string),
            millis_behind_latest,
        }
    }
}

/// One `GetRecords` answer, reduced to what the rule is asserted over.
struct Page {
    records: Vec<Value>,
    next_shard_iterator: Option<String>,
    millis_behind_latest: i64,
}

impl Page {
    /// The `Data` of every record of the page, decoded from the base64 the wire carries a
    /// blob as.
    fn payloads(&self, case: &str) -> Vec<Vec<u8>> {
        self.records
            .iter()
            .map(|record| {
                let encoded = record["Data"].as_str().unwrap_or_else(|| {
                    panic!("case {case}: every record carries Data, got {record}")
                });
                aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                    panic!("case {case}: a record's Data is base64, got {error} for {encoded:?}")
                })
            })
            .collect()
    }

    /// The `SequenceNumber` of every record of the page.
    fn sequence_numbers(&self, case: &str) -> Vec<String> {
        self.records
            .iter()
            .map(|record| {
                record["SequenceNumber"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: every record carries a SequenceNumber, got {record}")
                    })
                    .to_string()
            })
            .collect()
    }
}

/// Epoch milliseconds now, the unit `GetShardIteratorInput$Timestamp` carries.
fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_millis()
}

/// The records a one-shard stream holds are read back, in write order and byte for byte,
/// from every iterator type the model declares — each type reading the portion of the shard
/// its position names.
#[tokio::test]
async fn every_iterator_type_reads_the_records_its_position_names() {
    let emulator = Emulator::new();
    let stream = "records-every-type";
    emulator.active_stream("every type", stream, 1).await;
    let shard = shard_id(0);

    // The write side of the table: the first two payloads go in one by one, the rest as a
    // batch, so both write paths feed the SAME shard and the reader cannot tell them apart.
    let written = payloads();
    let mut sequence_numbers: Vec<String> = Vec::new();
    for payload in written.iter().take(2) {
        let (shard_written, sequence_number) = emulator
            .put_record("every type", stream, "partition-a", payload)
            .await;
        assert_eq!(
            shard_written, shard,
            "case every type: a one-shard stream places every record in {shard}, got \
             {shard_written}"
        );
        sequence_numbers.push(sequence_number);
    }
    let batch = emulator
        .ok(
            "every type",
            "PutRecords",
            &json!({
                "StreamName": stream,
                "Records": written.iter().skip(2).map(|payload| json!({
                    "PartitionKey": "partition-a",
                    "Data": aws_smithy_types::base64::encode(payload),
                })).collect::<Vec<Value>>(),
            }),
        )
        .await;
    assert_eq!(
        batch["FailedRecordCount"].as_i64(),
        Some(0),
        "case every type: a batch nothing failed in reports a FailedRecordCount of 0, got {batch}"
    );
    for entry in batch["Records"].as_array().unwrap_or(&Vec::new()) {
        assert_eq!(
            entry["ShardId"].as_str(),
            Some(shard.as_str()),
            "case every type: every result entry names the shard it was stored in, got {entry}"
        );
        sequence_numbers.push(
            entry["SequenceNumber"]
                .as_str()
                .unwrap_or_else(|| panic!("case every type: a result entry carries a \
                                           SequenceNumber, got {entry}"))
                .to_string(),
        );
    }
    assert_eq!(
        sequence_numbers.len(),
        written.len(),
        "case every type: every written record got one sequence number, got \
         {sequence_numbers:?}"
    );

    // (case, the iterator request, the records the position names)
    let cases: Vec<(String, Value, Vec<Vec<u8>>)> = vec![
        (
            format!("{SHARD_ITERATOR_TYPE_TRIM_HORIZON} reads the whole shard"),
            json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
            written.clone(),
        ),
        (
            format!("{SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER} includes its own record"),
            json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
                "StartingSequenceNumber": sequence_numbers[1],
            }),
            written[1..].to_vec(),
        ),
        (
            format!("{SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER} starts past its own record"),
            json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER,
                "StartingSequenceNumber": sequence_numbers[1],
            }),
            written[2..].to_vec(),
        ),
        (
            format!("{SHARD_ITERATOR_TYPE_AT_TIMESTAMP} inside the window reads the shard"),
            json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_TIMESTAMP,
                // A timestamp travels as epoch SECONDS for a `Timestamp` member of an
                // awsJson protocol; a whole second before now is before every write.
                "Timestamp": (now_millis() / 1_000) as f64 - 2.0,
            }),
            written.clone(),
        ),
        (
            format!("{SHARD_ITERATOR_TYPE_LATEST} reads nothing already written"),
            json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_LATEST,
            }),
            Vec::new(),
        ),
    ];
    for (case, request, expected) in cases {
        let iterator = emulator.iterator(&case, &request).await;
        let page = emulator
            .get_records(&case, &json!({"ShardIterator": iterator}))
            .await;
        assert_eq!(
            page.payloads(&case),
            expected,
            "case {case}: expected the records the position names, in write order, got {} \
             records",
            page.records.len()
        );
        assert!(
            page.next_shard_iterator.is_some(),
            "case {case}: an open shard always answers a NextShardIterator, got none"
        );
        assert_eq!(
            page.millis_behind_latest, 0,
            "case {case}: a page that read to the tip of the shard is caught up, got {}",
            page.millis_behind_latest
        );
    }

    // A record reads back under the sequence number its write answered, in that order.
    let case = "sequence numbers";
    let iterator = emulator.horizon_iterator(case, stream, &shard).await;
    let page = emulator
        .get_records(case, &json!({"ShardIterator": iterator}))
        .await;
    assert_eq!(
        page.sequence_numbers(case),
        sequence_numbers,
        "case {case}: every record reads back under the sequence number its write answered"
    );
    for record in &page.records {
        assert_eq!(
            record["PartitionKey"].as_str(),
            Some("partition-a"),
            "case {case}: a record reads back the partition key its producer sent, got {record}"
        );
        assert_eq!(
            record["EncryptionType"].as_str(),
            Some(ENCRYPTION_TYPE_NONE),
            "case {case}: a record of an unencrypted stream reads back NONE, got {record}"
        );
        assert!(
            record["ApproximateArrivalTimestamp"].is_number(),
            "case {case}: a record carries the approximate instant it arrived, got {record}"
        );
    }
}

/// `LATEST` is PINNED when the iterator is handed out, not resolved when the records are
/// read: a consumer that takes an iterator and only then starts producing reads everything
/// written after it took that iterator — and nothing written before.
#[tokio::test]
async fn latest_is_pinned_to_the_tip_the_iterator_was_issued_at() {
    let emulator = Emulator::new();
    let stream = "records-latest";
    let shard = shard_id(0);

    // (case, how many records the shard already holds when the iterator is taken)
    for already_written in [0usize, 1, 3] {
        let case = format!("{already_written} records written before the iterator");
        let stream_name = format!("{stream}-{already_written}");
        emulator.active_stream(&case, &stream_name, 1).await;
        for index in 0..already_written {
            emulator
                .put_record(
                    &case,
                    &stream_name,
                    "partition-a",
                    format!("before {index}").as_bytes(),
                )
                .await;
        }
        // The iterator is taken HERE: everything above is in the past, everything below is
        // in the future.
        let iterator = emulator.latest_iterator(&case, &stream_name, &shard).await;
        let after: Vec<Vec<u8>> = (0..3)
            .map(|index| format!("after {index}").into_bytes())
            .collect();
        for payload in &after {
            emulator
                .put_record(&case, &stream_name, "partition-a", payload)
                .await;
        }
        let page = emulator
            .get_records(&case, &json!({"ShardIterator": iterator}))
            .await;
        assert_eq!(
            page.payloads(&case),
            after,
            "case {case}: a LATEST iterator reads the records written after it was issued, \
             and no earlier one, got {} records",
            page.records.len()
        );

        // Continuing on the iterator it answered reads what was written after THAT page,
        // and nothing twice.
        let next = page
            .next_shard_iterator
            .unwrap_or_else(|| panic!("case {case}: an open shard answers a NextShardIterator"));
        emulator
            .put_record(&case, &stream_name, "partition-a", b"after the page")
            .await;
        let page = emulator
            .get_records(&case, &json!({"ShardIterator": next}))
            .await;
        assert_eq!(
            page.payloads(&case),
            vec![b"after the page".to_vec()],
            "case {case}: the continuation iterator reads only what was written after the \
             page it came from, got {} records",
            page.records.len()
        );
    }
}

/// A page is bounded by `Limit`, and the iterator it answers continues exactly where it
/// stopped — so a reader that loops sees every record once, in write order.
#[tokio::test]
async fn a_page_is_bounded_by_limit_and_its_next_iterator_resumes_where_it_stopped() {
    let emulator = Emulator::new();
    let stream = "records-paging";
    emulator.active_stream("paging", stream, 1).await;
    let shard = shard_id(0);
    let total = 6usize;
    for index in 0..total {
        emulator
            .put_record(
                "paging",
                stream,
                "partition-a",
                format!("event {index}").as_bytes(),
            )
            .await;
    }

    // (case, the page size a reader loops with)
    for limit in [1usize, 2, 4, total, total + 3] {
        let case = format!("limit {limit}");
        let mut iterator = emulator.horizon_iterator(&case, stream, &shard).await;
        let mut read: Vec<Vec<u8>> = Vec::new();
        // Bounded by the records written plus one empty page, so a loop that stops making
        // progress ends rather than running forever.
        for _ in 0..=total {
            let page = emulator
                .get_records(
                    &case,
                    &json!({"ShardIterator": iterator, "Limit": limit as i64}),
                )
                .await;
            assert!(
                page.records.len() <= limit,
                "case {case}: a page returns at most Limit records, got {}",
                page.records.len()
            );
            let empty = page.records.is_empty();
            read.extend(page.payloads(&case));
            iterator = page
                .next_shard_iterator
                .unwrap_or_else(|| panic!("case {case}: an open shard answers a NextShardIterator"));
            if empty {
                break;
            }
        }
        let expected: Vec<Vec<u8>> = (0..total)
            .map(|index| format!("event {index}").into_bytes())
            .collect();
        assert_eq!(
            read, expected,
            "case {case}: looping on NextShardIterator reads every record once, in write \
             order, got {} records",
            read.len()
        );
    }
}

/// Reading records is a read: the same iterator answers the same page twice, and a reader
/// that already read the shard does not keep another one from reading it.
#[tokio::test]
async fn reading_records_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let stream = "records-repeatable";
    emulator.active_stream("repeatable", stream, 1).await;
    let shard = shard_id(0);
    for payload in payloads() {
        emulator
            .put_record("repeatable", stream, "partition-a", &payload)
            .await;
    }

    let case = "the same iterator twice";
    let iterator = emulator.horizon_iterator(case, stream, &shard).await;
    let first = emulator
        .get_records(case, &json!({"ShardIterator": iterator.clone()}))
        .await;
    let second = emulator
        .get_records(case, &json!({"ShardIterator": iterator}))
        .await;
    assert_eq!(
        first.payloads(case),
        second.payloads(case),
        "case {case}: the same position answers the same records, got {} then {} records",
        first.records.len(),
        second.records.len()
    );

    let case = "a second reader from the horizon";
    let fresh = emulator.horizon_iterator(case, stream, &shard).await;
    let page = emulator
        .get_records(case, &json!({"ShardIterator": fresh}))
        .await;
    assert_eq!(
        page.payloads(case),
        payloads(),
        "case {case}: a later reader still sees every record, got {} records",
        page.records.len()
    );
}

/// A partition key maps a record to ONE shard — the shard whose published hash key range
/// holds the key's MD5 hash — and an explicit hash key names that shard directly. Every
/// record written with one key reads back from that shard and from no other.
#[tokio::test]
async fn a_partition_key_maps_every_record_to_one_shard() {
    let emulator = Emulator::new();
    let stream = "records-shards";
    let shard_count = 3usize;
    emulator.active_stream("shards", stream, shard_count as i64).await;

    // The shard map as the service publishes it, which is where the expected placement is
    // re-derived from: "an MD5 hash function is used to map partition keys to 128-bit
    // integer values and to map associated data records to shards using the hash key ranges
    // of the shards" (`PutRecord`).
    let listed = emulator
        .ok("shards", "ListShards", &json!({"StreamName": stream}))
        .await;
    let ranges: Vec<(String, u128, u128)> = listed["Shards"]
        .as_array()
        .expect("ListShards reports the shard map")
        .iter()
        .map(|shard| {
            let bound = |member: &str| -> u128 {
                shard["HashKeyRange"][member]
                    .as_str()
                    .expect("a shard publishes its HashKeyRange")
                    .parse()
                    .expect("a hash key is a decimal integer")
            };
            (
                shard["ShardId"].as_str().expect("a shard has an id").to_string(),
                bound("StartingHashKey"),
                bound("EndingHashKey"),
            )
        })
        .collect();
    assert_eq!(
        ranges.len(),
        shard_count,
        "case shards: a {shard_count}-shard stream publishes {shard_count} shards, got \
         {ranges:?}"
    );

    // (case, the partition key the records are written with, how many are written)
    let keys = ["alpha", "beta", "gamma", "delta", "a key with spaces", ""];
    // The mapping itself, checked against PUBLISHED MD5 test vectors (RFC 1321, appendix
    // A.5) read as one big-endian 128-bit integer — so a different digest, or the same
    // digest read the other way round, fails here rather than only being self-consistent.
    let vectors: [(&str, u128); 3] = [
        ("abc", 0x900150983cd24fb0_d6963f7d28e17f72),
        ("message digest", 0xf96b697d7cb7938d_525a2f31aaf161d0),
        ("abcdefghijklmnopqrstuvwxyz", 0xc3fcd3d76192e400_7dfb496cca67e13b),
    ];
    for (key, expected_hash) in vectors {
        assert_eq!(
            partition_key_hash(key),
            expected_hash,
            "case MD5 vector {key:?}: a partition key maps to the big-endian MD5 of its \
             bytes, got {:#034x}",
            partition_key_hash(key)
        );
        let case = format!("MD5 vector {key:?}");
        let expected = ranges
            .iter()
            .find(|(_, starting, ending)| (*starting..=*ending).contains(&expected_hash))
            .map(|(id, _, _)| id.clone())
            .unwrap_or_else(|| panic!("case {case}: the shard map covers every hash key"));
        let (placed, _) = emulator
            .put_record(&case, stream, key, b"a published vector")
            .await;
        assert_eq!(
            placed, expected,
            "case {case}: the record goes to the shard whose published HashKeyRange holds \
             {expected_hash:#034x}, got {placed}"
        );
    }
    for key in keys {
        let case = format!("partition key {key:?}");
        // An empty partition key is outside `PartitionKey`'s modelled `len 1..256`, so it
        // is refused by the request walk before any handler runs.
        if key.is_empty() {
            let (status, response) = emulator
                .call(
                    "PutRecord",
                    &json!({
                        "StreamName": stream,
                        "PartitionKey": key,
                        "Data": aws_smithy_types::base64::encode("x"),
                    }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case}: a partition key shorter than the modelled minimum is refused, \
                 got {status} with {response}"
            );
            continue;
        }
        let hash = partition_key_hash(key);
        let expected = ranges
            .iter()
            .find(|(_, starting, ending)| (*starting..=*ending).contains(&hash))
            .map(|(id, _, _)| id.clone())
            .unwrap_or_else(|| panic!("case {case}: the shard map covers every hash key"));
        let mut placed: Vec<String> = Vec::new();
        for index in 0..3 {
            let (shard, _) = emulator
                .put_record(&case, stream, key, format!("{key} {index}").as_bytes())
                .await;
            placed.push(shard);
        }
        assert!(
            placed.iter().all(|shard| *shard == expected),
            "case {case}: every record with one partition key goes to the shard its hash \
             falls in ({expected}), got {placed:?}"
        );
        let iterator = emulator.horizon_iterator(&case, stream, &expected).await;
        let page = emulator
            .get_records(&case, &json!({"ShardIterator": iterator}))
            .await;
        let expected_payloads: Vec<Vec<u8>> = (0..3)
            .map(|index| format!("{key} {index}").into_bytes())
            .collect();
        for payload in &expected_payloads {
            assert!(
                page.payloads(&case).contains(payload),
                "case {case}: the shard the records were placed in reads them back, missing \
                 {payload:?}"
            );
        }
    }

    // An explicit hash key overrides the partition key hash: "The hash value used to
    // explicitly determine the shard the data record is assigned to by overriding the
    // partition key hash" (`PutRecordInput$ExplicitHashKey`).
    for (expected, starting, _) in &ranges {
        let case = format!("explicit hash key of {expected}");
        let answer = emulator
            .ok(
                &case,
                "PutRecord",
                &json!({
                    "StreamName": stream,
                    "PartitionKey": "ignored-by-the-explicit-hash-key",
                    "ExplicitHashKey": starting.to_string(),
                    "Data": aws_smithy_types::base64::encode("explicit"),
                }),
            )
            .await;
        assert_eq!(
            answer["ShardId"].as_str(),
            Some(expected.as_str()),
            "case {case}: an explicit hash key places the record in the shard whose range \
             holds it, got {answer}"
        );
    }
}

/// Every refusal the rule names, each with the modelled error code it is refused under.
#[tokio::test]
async fn a_read_or_write_the_rule_does_not_admit_is_refused() {
    let emulator = Emulator::new();
    let stream = "records-refusals";
    let arn = emulator.active_stream("refusals", stream, 1).await;
    let other = "records-refusals-other";
    let other_arn = emulator.active_stream("refusals", other, 1).await;
    let shard = shard_id(0);
    let iterator = emulator.horizon_iterator("refusals", stream, &shard).await;

    // An iterator this service issued, aged past its lifetime: "A shard iterator expires 5
    // minutes after it is returned to the requester" (`GetShardIterator`).
    let mut aged = ShardIteratorToken::decode(&iterator).expect("the issued iterator decodes");
    assert_eq!(
        aged.position,
        ShardPosition::TrimHorizon,
        "the iterator under test names the position it was asked for, got {aged:?}"
    );
    aged.issued_at_millis = now_millis()
        .saturating_sub(u128::from(SHARD_ITERATOR_TTL.as_millis() as u64) + 1_000);
    let expired = aged.encode();

    // (case, the operation, the request, the modelled error code it is refused under)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a shard iterator this service never issued",
            "GetRecords",
            json!({"ShardIterator": "not-an-iterator"}),
            "InvalidArgumentException",
        ),
        (
            "an iterator past its lifetime",
            "GetRecords",
            json!({"ShardIterator": expired}),
            "ExpiredIteratorException",
        ),
        (
            "a StreamARN naming another stream than the iterator does",
            "GetRecords",
            json!({"ShardIterator": iterator.clone(), "StreamARN": other_arn}),
            "InvalidArgumentException",
        ),
        (
            "a dry run of an otherwise valid read",
            "GetRecords",
            json!({"ShardIterator": iterator.clone(), "DryRun": true}),
            "DryRunOperationException",
        ),
        (
            "a write naming neither StreamName nor StreamARN",
            "PutRecord",
            json!({
                "PartitionKey": "partition-a",
                "Data": aws_smithy_types::base64::encode("x"),
            }),
            "InvalidArgumentException",
        ),
        (
            "a write to a stream this account and region do not hold",
            "PutRecord",
            json!({
                "StreamName": "records-no-such-stream",
                "PartitionKey": "partition-a",
                "Data": aws_smithy_types::base64::encode("x"),
            }),
            "ResourceNotFoundException",
        ),
        (
            "a write naming no partition key",
            "PutRecord",
            json!({
                "StreamName": stream,
                "Data": aws_smithy_types::base64::encode("x"),
            }),
            "InvalidArgumentException",
        ),
        (
            "a write whose explicit hash key is in no shard of the map",
            "PutRecord",
            json!({
                "StreamName": stream,
                "PartitionKey": "partition-a",
                "ExplicitHashKey": "340282366920938463463374607431768211456",
                "Data": aws_smithy_types::base64::encode("x"),
            }),
            "InvalidArgumentException",
        ),
        (
            // The blob alone is longer than the stream's maximum record size, which is a
            // bound on that member's LENGTH: refused as a validation failure naming it,
            // the way a modelled `@length` is. The total of the blob AND the partition key
            // is a bound on two members together, which is the modelled
            // `InvalidArgumentException` instead (asserted in `max_record_size`).
            "a record whose data is longer than the stream's maximum record size",
            "PutRecord",
            json!({
                "StreamName": stream,
                "PartitionKey": "partition-a",
                "Data": aws_smithy_types::base64::encode(
                    vec![b'x'; (DEFAULT_MAX_RECORD_SIZE_KI_B as usize) * 1024 + 1],
                ),
            }),
            "ValidationException",
        ),
        (
            "a dry run of an otherwise valid write",
            "PutRecord",
            json!({
                "StreamName": stream,
                "PartitionKey": "partition-a",
                "Data": aws_smithy_types::base64::encode("x"),
                "DryRun": true,
            }),
            "DryRunOperationException",
        ),
        (
            "a batch with one entry whose data is longer than the maximum record size",
            "PutRecords",
            json!({
                "StreamName": stream,
                "Records": [
                    {"PartitionKey": "partition-a", "Data": aws_smithy_types::base64::encode("x")},
                    {"PartitionKey": "partition-a", "Data": aws_smithy_types::base64::encode(
                        vec![b'y'; (DEFAULT_MAX_RECORD_SIZE_KI_B as usize) * 1024 + 1],
                    )},
                ],
            }),
            "ValidationException",
        ),
        (
            // Each entry is inside the per-record maximum, but together they are past the
            // documented aggregate: "Each record in the request can be as large as 1 MiB,
            // up to a limit of 5 MiB for the entire request" (`PutRecords`).
            "a batch whose entries together pass the request's byte ceiling",
            "PutRecords",
            json!({
                "StreamName": stream,
                "Records": (0..6).map(|_| json!({
                    "PartitionKey": "partition-a",
                    "Data": aws_smithy_types::base64::encode(
                        vec![b'z'; (DEFAULT_MAX_RECORD_SIZE_KI_B as usize) * 1024 - 16],
                    ),
                })).collect::<Vec<Value>>(),
            }),
            "InvalidArgumentException",
        ),
    ];
    for (case, operation, body, expected) in cases {
        let (code, message) = emulator.refused(case, operation, &body).await;
        assert_eq!(
            code, expected,
            "case {case}: expected {expected}, got {code} with message {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case}: a refusal carries a message, got none"
        );
    }

    // The refusals that have one sentence of their own say exactly what was wrong.
    let (_, message) = emulator
        .refused(
            "the invalid iterator is explained",
            "GetRecords",
            &json!({"ShardIterator": "not-an-iterator"}),
        )
        .await;
    // The sentence Kinesis answers for an unusable iterator, pinned as a LITERAL so the
    // wording itself is asserted rather than whatever the implementation's constant says.
    assert_eq!(
        message, "Invalid ShardIterator.",
        "an iterator this service never issued is refused with the sentence Kinesis uses"
    );
    let (_, message) = emulator
        .refused(
            "the mismatch is explained",
            "GetRecords",
            &json!({"ShardIterator": iterator, "StreamARN": format!("{arn}-not-this-one")}),
        )
        .await;
    // Each sentence is pinned as a LITERAL, so changing the wording fails this test rather
    // than moving the expectation with it.
    assert_eq!(
        message,
        "StreamARN names a different stream than the ShardIterator does; the iterator \
         already identifies the stream its position is in",
        "a StreamARN naming another stream is refused with the sentence that states the rule"
    );
    let (_, message) = emulator
        .refused(
            "the missing partition key is explained",
            "PutRecord",
            &json!({
                "StreamName": stream,
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        )
        .await;
    assert_eq!(
        message,
        "PartitionKey is required for a stream that uses the USER_PARTITION_KEY record \
         distribution strategy",
        "a write naming no partition key is refused with the sentence that states the rule"
    );

    // Nothing a refusal touched was stored: the shard still reads back empty.
    let case = "a refused write stores nothing";
    let fresh = emulator.horizon_iterator(case, stream, &shard).await;
    let page = emulator
        .get_records(case, &json!({"ShardIterator": fresh}))
        .await;
    assert!(
        page.records.is_empty(),
        "case {case}: expected an empty shard, got {} records",
        page.records.len()
    );
}

/// A write to a stream whose creation has not fallen due yet stores nothing: the shard the
/// record would land in is in no stream this account holds yet.
#[tokio::test]
async fn a_stream_that_is_not_active_yet_takes_no_record() {
    let emulator = Emulator::new();
    let stream = "records-creating";
    emulator
        .ok(
            "creating",
            "CreateStream",
            &json!({"StreamName": stream, "ShardCount": 1}),
        )
        .await;
    let (code, message) = emulator
        .refused(
            "creating",
            "PutRecord",
            &json!({
                "StreamName": stream,
                "PartitionKey": "partition-a",
                "Data": aws_smithy_types::base64::encode("too early"),
            }),
        )
        .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case creating: expected ResourceNotFoundException, got {code} with message {message:?}"
    );

    // Once the transition the create armed has fallen due, the same write is served and the
    // record reads back.
    tokio::time::sleep(past_activation()).await;
    emulator
        .put_record("active", stream, "partition-a", b"once active")
        .await;
    let iterator = emulator
        .horizon_iterator("active", stream, &shard_id(0))
        .await;
    let page = emulator
        .get_records("active", &json!({"ShardIterator": iterator}))
        .await;
    assert_eq!(
        page.payloads("active"),
        vec![b"once active".to_vec()],
        "case active: the record written to the active stream reads back, got {} records",
        page.records.len()
    );
}
