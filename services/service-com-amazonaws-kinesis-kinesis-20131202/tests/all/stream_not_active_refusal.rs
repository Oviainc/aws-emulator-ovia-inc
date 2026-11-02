// SPDX-License-Identifier: CC0-1.0

//! The sentence this service refuses a stream that is NOT `ACTIVE` with, end to end over the
//! wire, for every operation that needs a ready stream.
//!
//! RULE: an operation that needs a stream in the `ACTIVE` state refuses one that is not with
//! ONE sentence, which names the stream by the `StreamName` it is stored under, the account
//! that holds it and the lifecycle state the stream is in INSTEAD of `ACTIVE` — the same
//! sentence whichever operation refuses, whichever identifier the request addressed the
//! stream by, and whichever state the stream is in.
//!
//! The recorded flow this was derived from issues a second `UpdateStreamMode` on a stream
//! whose previous mode change is still in flight (one operation, addressed by its ARN, in
//! `UPDATING`), which is one row of the tables below. The rows differ along what the rule
//! generalises over: the operation refused, the identifier the request names the stream by,
//! the lifecycle state the stream is in, and the modelled error the operation carries the
//! sentence in.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    STREAM_CREATING, STREAM_DELETING, STREAM_MODE_ON_DEMAND, STREAM_UPDATING,
};

use crate::stream_tags::{
    error_code, error_message, past_activation, stream_arn, Emulator, REGION,
};

/// The sentence the service states for a stream that is not `ACTIVE`, written out here rather
/// than borrowed from the crate so the wording itself is what the cases pin.
fn not_active_sentence(stream_name: &str, status: &str) -> String {
    format!(
        "Stream {stream_name} under account {} not ACTIVE, instead in state {status}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A shard id no stream holds: every operation below checks the stream's state BEFORE it
/// resolves a shard, so the cases that name one name this.
const ABSENT_SHARD_ID: &str = "shardId-000000000099";

/// A hash key inside the modelled `HashKey` pattern (`0|([1-9]\d{0,38})`), for the split
/// cases — the value is never reached, since the state check comes first.
const MIDDLE_HASH_KEY: &str = "170141183460469231731687303715884105728";

/// A request body for `operation` addressing the stream `name`. Each one is a well-formed
/// request for that operation, so what refuses it is the stream's state and nothing else.
fn body_for(operation: &str, name: &str) -> Value {
    let arn = stream_arn(REGION, name);
    match operation {
        // `StreamARN` is this operation's ONLY stream identifier, so the case that drives it
        // addresses the stream by its ARN and still expects the stored name in the sentence.
        "UpdateStreamMode" => json!({
            "StreamARN": arn,
            "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
        }),
        "UpdateMaxRecordSize" => json!({ "StreamARN": arn, "MaxRecordSizeInKiB": 2048 }),
        "RegisterStreamConsumer" => json!({ "StreamARN": arn, "ConsumerName": "a-consumer" }),
        "UpdateShardCount" => json!({
            "StreamName": name,
            "TargetShardCount": 4,
            "ScalingType": "UNIFORM_SCALING",
        }),
        "DeleteStream" => json!({ "StreamName": name }),
        "IncreaseStreamRetentionPeriod" => {
            json!({ "StreamName": name, "RetentionPeriodHours": 48 })
        }
        "DecreaseStreamRetentionPeriod" => {
            json!({ "StreamName": name, "RetentionPeriodHours": 24 })
        }
        "EnableEnhancedMonitoring" => {
            json!({ "StreamName": name, "ShardLevelMetrics": ["ALL"] })
        }
        "DisableEnhancedMonitoring" => {
            json!({ "StreamName": name, "ShardLevelMetrics": ["ALL"] })
        }
        "SplitShard" => json!({
            "StreamName": name,
            "ShardToSplit": ABSENT_SHARD_ID,
            "NewStartingHashKey": MIDDLE_HASH_KEY,
        }),
        "MergeShards" => json!({
            "StreamName": name,
            "ShardToMerge": ABSENT_SHARD_ID,
            "AdjacentShardToMerge": ABSENT_SHARD_ID,
        }),
        "ListShards" => json!({ "StreamName": name }),
        "GetShardIterator" => json!({
            "StreamName": name,
            "ShardId": ABSENT_SHARD_ID,
            "ShardIteratorType": "TRIM_HORIZON",
        }),
        "PutRecord" => json!({
            "StreamName": name,
            "Data": "aGVsbG8=",
            "PartitionKey": "a-key",
        }),
        "PutRecords" => json!({
            "StreamName": name,
            "Records": [{ "Data": "aGVsbG8=", "PartitionKey": "a-key" }],
        }),
        "StartStreamEncryption" => json!({
            "StreamName": name,
            "EncryptionType": "KMS",
            "KeyId": "alias/a-customer-key",
        }),
        "StopStreamEncryption" => json!({
            "StreamName": name,
            "EncryptionType": "KMS",
            "KeyId": "alias/a-customer-key",
        }),
        other => panic!("case {other}: this suite builds no body for that operation"),
    }
}

/// Create a provisioned stream of two shards named `name`, without waiting for the transition
/// the create armed: the stream is `CREATING` when this returns.
async fn creating_stream(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 2 }),
        )
        .await;
}

/// Call `operation` against `name` and assert it was refused with `code` carrying the
/// not-active sentence for `status`.
async fn assert_refused_with_the_sentence(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    name: &str,
    code: &str,
    status: &str,
) {
    let (http_status, response) = emulator.call(operation, &body_for(operation, name)).await;
    assert_eq!(
        http_status, 400,
        "case {case}: expected {operation} on a {status} stream to answer 400, got \
         {http_status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        code,
        "case {case}: expected {operation} to refuse a {status} stream with {code}, got \
         {response}"
    );
    assert_eq!(
        error_message(&response),
        not_active_sentence(name, status),
        "case {case}: expected the not-active sentence this service states, got {}",
        error_message(&response)
    );
}

/// The operations that need an `ACTIVE` stream, each carrying the refusal in the modelled
/// `ResourceInUseException` the rule publishes ("For successful operation, the resource must
/// be in the ACTIVE state").
const ACTIVE_ONLY_OPERATIONS: &[&str] = &[
    "UpdateStreamMode",
    "UpdateMaxRecordSize",
    "UpdateShardCount",
    "RegisterStreamConsumer",
    "IncreaseStreamRetentionPeriod",
    "DecreaseStreamRetentionPeriod",
    "EnableEnhancedMonitoring",
    "DisableEnhancedMonitoring",
    "SplitShard",
    "MergeShards",
];

/// A stream whose create has not fallen due yet is refused by every one of them, with the one
/// sentence naming the stored stream name, this account and `CREATING`.
#[tokio::test]
async fn every_active_only_operation_refuses_a_creating_stream_with_the_one_sentence() {
    let emulator = Emulator::new();
    for operation in ACTIVE_ONLY_OPERATIONS {
        let name = format!("creating-{}", operation.to_lowercase());
        // Created and called straight away, so the stream is still inside the transition the
        // create armed when the call arrives.
        creating_stream(&emulator, &name).await;
        assert_refused_with_the_sentence(
            &emulator,
            &name,
            operation,
            &name,
            "ResourceInUseException",
            STREAM_CREATING,
        )
        .await;
    }
}

/// The same sentence for a stream whose removal has been accepted: the state it reports is the
/// state the sentence names.
#[tokio::test]
async fn every_active_only_operation_refuses_a_deleting_stream_with_the_one_sentence() {
    let emulator = Emulator::new();
    let names: Vec<String> = ACTIVE_ONLY_OPERATIONS
        .iter()
        .map(|operation| format!("deleting-{}", operation.to_lowercase()))
        .collect();
    for name in &names {
        creating_stream(&emulator, name).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (operation, name) in ACTIVE_ONLY_OPERATIONS.iter().zip(&names) {
        // The removal is armed and the call arrives before it falls due, so the stream is
        // `DELETING` rather than gone.
        emulator
            .ok(name, "DeleteStream", &json!({ "StreamName": name }))
            .await;
        assert_refused_with_the_sentence(
            &emulator,
            name,
            operation,
            name,
            "ResourceInUseException",
            STREAM_DELETING,
        )
        .await;
    }
}

/// And for a stream whose previous update is still in flight — the recorded row, widened to
/// every operation the rule covers.
#[tokio::test]
async fn every_active_only_operation_refuses_an_updating_stream_with_the_one_sentence() {
    let emulator = Emulator::new();
    let names: Vec<String> = ACTIVE_ONLY_OPERATIONS
        .iter()
        .map(|operation| format!("updating-{}", operation.to_lowercase()))
        .collect();
    for name in &names {
        creating_stream(&emulator, name).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (operation, name) in ACTIVE_ONLY_OPERATIONS.iter().zip(&names) {
        // A shard-count change leaves the stream `UPDATING` and provisioned, so what refuses
        // the call below is the state and not the capacity mode.
        emulator
            .ok(
                name,
                "UpdateShardCount",
                &json!({
                    "StreamName": name,
                    "TargetShardCount": 3,
                    "ScalingType": "UNIFORM_SCALING",
                }),
            )
            .await;
        assert_refused_with_the_sentence(
            &emulator,
            name,
            operation,
            name,
            "ResourceInUseException",
            STREAM_UPDATING,
        )
        .await;
    }
}

/// The operations that serve a stream while its update is in flight ("you can continue to read
/// and write data to your stream while its status is UPDATING") but have nothing to serve for
/// one whose create or whose removal is still due, with the modelled error each carries the
/// refusal in.
const SERVING_OPERATIONS: &[(&str, &str)] = &[
    ("ListShards", "ResourceInUseException"),
    ("GetShardIterator", "ResourceNotFoundException"),
    ("PutRecord", "ResourceNotFoundException"),
    ("PutRecords", "ResourceNotFoundException"),
    ("StartStreamEncryption", "ResourceInUseException"),
    ("StopStreamEncryption", "ResourceInUseException"),
];

/// One sentence across the modelled errors too: an operation that carries the refusal in a
/// not-found error states it exactly as one that carries it in an in-use error does.
#[tokio::test]
async fn a_serving_operation_states_the_same_sentence_in_its_own_modelled_error() {
    let emulator = Emulator::new();
    for (operation, code) in SERVING_OPERATIONS {
        let name = format!("creating-{}", operation.to_lowercase());
        creating_stream(&emulator, &name).await;
        assert_refused_with_the_sentence(
            &emulator, &name, operation, &name, code, STREAM_CREATING,
        )
        .await;
    }

    let names: Vec<String> = SERVING_OPERATIONS
        .iter()
        .map(|(operation, _)| format!("deleting-{}", operation.to_lowercase()))
        .collect();
    for name in &names {
        creating_stream(&emulator, name).await;
    }
    tokio::time::sleep(past_activation()).await;
    for ((operation, code), name) in SERVING_OPERATIONS.iter().zip(&names) {
        emulator
            .ok(name, "DeleteStream", &json!({ "StreamName": name }))
            .await;
        assert_refused_with_the_sentence(
            &emulator, name, operation, name, code, STREAM_DELETING,
        )
        .await;
    }
}

/// `DeleteStream` is NOT one of them: a stream is "ready for read and write operations or
/// deletion" once `ACTIVE`, but the removal does not WAIT for that state — the delete of a
/// stream whose create or whose change is still in flight is accepted, and only a stream
/// whose removal has already been accepted has nothing left to accept, which is what the one
/// sentence names for `DELETING`.
#[tokio::test]
async fn a_delete_is_accepted_before_the_stream_is_active_and_refused_once_it_is_deleting() {
    let emulator = Emulator::new();

    // A stream still inside the transition its create armed.
    let creating = "delete-while-creating";
    creating_stream(&emulator, creating).await;
    emulator
        .ok(
            creating,
            "DeleteStream",
            &json!({ "StreamName": creating }),
        )
        .await;
    // That accepted delete is what put the stream in `DELETING`, so a second one has
    // nothing left to accept.
    assert_refused_with_the_sentence(
        &emulator,
        creating,
        "DeleteStream",
        creating,
        "ResourceInUseException",
        STREAM_DELETING,
    )
    .await;

    // And a stream whose shard-count change is still in flight.
    let updating = "delete-while-updating";
    creating_stream(&emulator, updating).await;
    tokio::time::sleep(past_activation()).await;
    emulator
        .ok(
            updating,
            "UpdateShardCount",
            &json!({
                "StreamName": updating,
                "TargetShardCount": 3,
                "ScalingType": "UNIFORM_SCALING",
            }),
        )
        .await;
    emulator
        .ok(
            updating,
            "DeleteStream",
            &json!({ "StreamName": updating }),
        )
        .await;
    assert_refused_with_the_sentence(
        &emulator,
        updating,
        "DeleteStream",
        updating,
        "ResourceInUseException",
        STREAM_DELETING,
    )
    .await;
}

/// The stream a refusal names is the STORED one: a request that addresses the stream by its
/// ARN, by its name, or by both is refused with the very same sentence, which names neither
/// the ARN nor the identifier the request used.
#[tokio::test]
async fn the_sentence_names_the_stored_stream_whichever_identifier_the_request_used() {
    let emulator = Emulator::new();
    let name = "identifier-forms";
    creating_stream(&emulator, name).await;

    let arn = stream_arn(REGION, name);
    // (the case, the identifier members the request carries)
    let cases: Vec<(&str, Value)> = vec![
        ("by-name", json!({ "StreamName": name })),
        ("by-arn", json!({ "StreamARN": arn })),
        (
            "by-both",
            json!({ "StreamName": name, "StreamARN": arn }),
        ),
    ];
    for (case, identifiers) in &cases {
        let mut body = identifiers.clone();
        body["ShardLevelMetrics"] = json!(["IncomingBytes"]);
        let (status, response) = emulator.call("EnableEnhancedMonitoring", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceInUseException",
            "case {case}: expected ResourceInUseException, got {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            not_active_sentence(name, STREAM_CREATING),
            "case {case}: expected the sentence to name the stored stream, got {message}"
        );
        assert!(
            !message.contains(&arn),
            "case {case}: the sentence names the stream, not the ARN the request used, got \
             {message}"
        );
    }
}
