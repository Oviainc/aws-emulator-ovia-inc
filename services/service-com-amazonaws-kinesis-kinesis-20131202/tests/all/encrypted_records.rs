// SPDX-License-Identifier: CC0-1.0

//! What a Kinesis data stream reports about the ENCRYPTION OF ITS RECORDS, end to end over
//! the wire: `PutRecord` / `PutRecords` -> `StartStreamEncryption` / `StopStreamEncryption`
//! -> `GetShardIterator` -> `GetRecords`.
//!
//! RULE: a record is reported under the encryption the stream carried when the record was
//! WRITTEN — "Once the status of the stream is ACTIVE, encryption begins for records written
//! to the stream... you can verify that encryption is applied by inspecting the API response
//! from PutRecord or PutRecords" (`StartStreamEncryption`) — and `Record$EncryptionType` is
//! "the encryption type used on the record", so a reader served that record later is told the
//! type it was written under, whatever the stream's encryption is by then. A write to a
//! stream nothing has encrypted applied no encryption, so its answer names no type at all; a
//! write to one `StartStreamEncryption` has encrypted names `KMS`, and a write once
//! `StopStreamEncryption` has disabled it names none again — while the records written in
//! between keep theirs.
//!
//! `stream_encryption.rs` covers what the two encryption operations STORE and what a
//! description reports; this suite covers the other reader of that stored configuration, the
//! record plane, which is where the doc says encryption is verified. The rows differ along
//! what the rule generalises over: whether a record arrives as a single write or in a batch,
//! the encryption in force when it is written (none, KMS, none again), and whether it is read
//! back before or after the stream's encryption changed again.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses and
//! bodies asserted here are the ones a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    ENCRYPTION_TYPE_KMS, ENCRYPTION_TYPE_NONE, ENCRYPTION_UPDATE_DELAY, STREAM_ACTIVE,
    SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};

use crate::shard_scaling::{shard_id, shard_map, summary};
use crate::stream_tags::{past_activation, Emulator};

/// A `KeyId` in one of the forms `StartStreamEncryptionInput$KeyId` documents: "You can also
/// use a master key owned by Kinesis Data Streams by specifying the alias aws/kinesis".
const KEY_ID: &str = "alias/aws/kinesis";

/// How long a case waits before treating a stream whose encryption changed as ready: past the
/// instant the change armed the `UPDATING` -> `ACTIVE` transition for.
fn past_encryption_update() -> Duration {
    ENCRYPTION_UPDATE_DELAY + Duration::from_millis(400)
}

/// The `Data` member as it travels on the wire: a blob is base64-encoded when it is
/// serialized.
fn data(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// Write one record to `name` and answer the `EncryptionType` `PutRecord` reports for it,
/// `None` when the answer names none.
async fn write_one(
    emulator: &Emulator,
    case: &str,
    name: &str,
    payload: &str,
) -> Option<String> {
    let answer = emulator
        .ok(
            case,
            "PutRecord",
            &json!({
                "StreamName": name,
                "PartitionKey": payload,
                "Data": data(payload.as_bytes()),
            }),
        )
        .await;
    reported_type(&answer)
}

/// Write `payloads` as ONE batch to `name` and answer the `EncryptionType` `PutRecords`
/// reports for the batch, `None` when the answer names none.
async fn write_batch(
    emulator: &Emulator,
    case: &str,
    name: &str,
    payloads: &[&str],
) -> Option<String> {
    let entries: Vec<Value> = payloads
        .iter()
        .map(|payload| {
            json!({ "PartitionKey": payload, "Data": data(payload.as_bytes()) })
        })
        .collect();
    let answer = emulator
        .ok(
            case,
            "PutRecords",
            &json!({ "StreamName": name, "Records": entries }),
        )
        .await;
    assert_eq!(
        answer["FailedRecordCount"].as_i64(),
        Some(0),
        "case {case}: expected every entry of the batch to be stored, got {answer}"
    );
    reported_type(&answer)
}

/// The `EncryptionType` a write's answer carries, `None` when it carries none: the member is
/// optional and reports the encryption the write APPLIED, so a write that encrypted nothing
/// names no type.
fn reported_type(answer: &Value) -> Option<String> {
    answer
        .get("EncryptionType")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Turn encryption on for `name` and wait for the change to fall due.
async fn encrypt(emulator: &Emulator, case: &str, name: &str) {
    emulator
        .ok(
            case,
            "StartStreamEncryption",
            &json!({
                "StreamName": name,
                "EncryptionType": ENCRYPTION_TYPE_KMS,
                "KeyId": KEY_ID,
            }),
        )
        .await;
    tokio::time::sleep(past_encryption_update()).await;
    assert_ready(emulator, case, name).await;
}

/// Turn encryption off for `name` and wait for the change to fall due.
async fn decrypt(emulator: &Emulator, case: &str, name: &str) {
    emulator
        .ok(
            case,
            "StopStreamEncryption",
            &json!({
                "StreamName": name,
                "EncryptionType": ENCRYPTION_TYPE_KMS,
                "KeyId": KEY_ID,
            }),
        )
        .await;
    tokio::time::sleep(past_encryption_update()).await;
    assert_ready(emulator, case, name).await;
}

/// Assert the stream is back to `ACTIVE`, which is the status the doc ties the record plane's
/// encryption to.
async fn assert_ready(emulator: &Emulator, case: &str, name: &str) {
    let (status, _) = summary(emulator, case, name).await;
    assert_eq!(
        status, STREAM_ACTIVE,
        "case {case}: expected the stream to be {STREAM_ACTIVE} once the change fell due, got \
         {status}"
    );
}

/// The (partition key, `EncryptionType`) of every record the stream's one shard serves, from
/// its oldest retained record, in write order.
async fn served_records(emulator: &Emulator, case: &str, name: &str) -> Vec<(String, String)> {
    let shards = shard_map(emulator, case, name).await;
    let shard = shards
        .first()
        .unwrap_or_else(|| panic!("case {case}: the stream must hold a shard"));
    let iterator = emulator
        .ok(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": name,
                "ShardId": shard_id(case, shard),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetShardIterator must answer a ShardIterator"))
        .to_string();
    let answer = emulator
        .ok(case, "GetRecords", &json!({ "ShardIterator": iterator }))
        .await;
    answer["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: GetRecords must answer Records, got {answer}"))
        .iter()
        .map(|record| {
            let partition_key = record["PartitionKey"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a record names its PartitionKey"))
                .to_string();
            let encryption_type = record
                .get("EncryptionType")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: a record must carry an EncryptionType, got {record}")
                })
                .to_string();
            (partition_key, encryption_type)
        })
        .collect()
}

/// A write is reported under the encryption the stream carries when it arrives, whether it
/// arrives alone or in a batch — and the record keeps that type when a reader is served it
/// later, after the stream's encryption has changed again.
#[tokio::test]
async fn a_record_is_reported_under_the_encryption_in_force_when_it_was_written() {
    let emulator = Emulator::new();
    let case = "encrypted-records";
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": case, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    // Before anything encrypts the stream: neither a single write nor a batch names a type.
    let single_before = write_one(&emulator, case, case, "plain-single").await;
    assert_eq!(
        single_before, None,
        "case {case}: expected a write to an unencrypted stream to name no encryption type, \
         got {single_before:?}"
    );
    let batch_before = write_batch(&emulator, case, case, &["plain-batch-1", "plain-batch-2"]).await;
    assert_eq!(
        batch_before, None,
        "case {case}: expected a batch written to an unencrypted stream to name no \
         encryption type, got {batch_before:?}"
    );

    // "Once the status of the stream is ACTIVE, encryption begins for records written to the
    // stream... you can verify that encryption is applied by inspecting the API response from
    // PutRecord or PutRecords."
    encrypt(&emulator, case, case).await;
    let single_encrypted = write_one(&emulator, case, case, "kms-single").await;
    assert_eq!(
        single_encrypted.as_deref(),
        Some(ENCRYPTION_TYPE_KMS),
        "case {case}: expected a record written to the encrypted stream to be reported under \
         {ENCRYPTION_TYPE_KMS}, got {single_encrypted:?}"
    );
    let batch_encrypted = write_batch(&emulator, case, case, &["kms-batch-1", "kms-batch-2"]).await;
    assert_eq!(
        batch_encrypted.as_deref(),
        Some(ENCRYPTION_TYPE_KMS),
        "case {case}: expected a batch written to the encrypted stream to be reported under \
         {ENCRYPTION_TYPE_KMS}, got {batch_encrypted:?}"
    );

    // Encryption disabled again: what is written from then on is not encrypted, so the
    // answer names no type.
    decrypt(&emulator, case, case).await;
    let single_after = write_one(&emulator, case, case, "plain-again").await;
    assert_eq!(
        single_after, None,
        "case {case}: expected a write once encryption was disabled to name no encryption \
         type, got {single_after:?}"
    );

    // Every record is served under the type it was WRITTEN under, so the two changes of the
    // stream's encryption did not re-report what was already in the shard.
    let served = served_records(&emulator, case, case).await;
    let expected: Vec<(String, String)> = [
        ("plain-single", ENCRYPTION_TYPE_NONE),
        ("plain-batch-1", ENCRYPTION_TYPE_NONE),
        ("plain-batch-2", ENCRYPTION_TYPE_NONE),
        ("kms-single", ENCRYPTION_TYPE_KMS),
        ("kms-batch-1", ENCRYPTION_TYPE_KMS),
        ("kms-batch-2", ENCRYPTION_TYPE_KMS),
        ("plain-again", ENCRYPTION_TYPE_NONE),
    ]
    .iter()
    .map(|(key, encryption_type)| ((*key).to_string(), (*encryption_type).to_string()))
    .collect();
    assert_eq!(
        served, expected,
        "case {case}: expected every record to be served under the encryption in force when \
         it was written, got {served:?}"
    );
}
