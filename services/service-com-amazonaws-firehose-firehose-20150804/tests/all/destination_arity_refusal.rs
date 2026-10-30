// SPDX-License-Identifier: CC0-1.0

//! The sentence `CreateDeliveryStream` refuses a request that names no destination with.
//!
//! The rule under test: "A Firehose stream is configured with a single destination", so a
//! create that carries no destination configuration at all is refused with HTTP 400
//! `InvalidArgumentException` and the one fixed sentence stating that arity rule. The
//! sentence interpolates nothing — it names no member, because the request supplied none to
//! name, and it is the same whatever else the request carried — and the refused create
//! stores nothing.
//!
//! The rows span what the rule must NOT depend on: the stream name, the declared
//! `DeliveryStreamType`, the source configuration that type is fed by, the tags, and a
//! server-side-encryption input. A create carrying exactly one destination is accepted
//! beside them, so what the rows isolate is the destination's arity and not the other
//! members.
//!
//! The harness (`Emulator`, `error_code`, `error_message`, `s3_configuration`) is the
//! lifecycle suite's.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    error_code, error_message, s3_configuration, Emulator, ROLE_ARN,
};

/// Firehose's sentence for the destination-arity rule, as the recorded refusal words it:
/// fixed text with nothing taken from the request or from state.
const ARITY_REFUSAL: &str = "Exactly one destination configuration is supported for a Firehose";

#[tokio::test]
async fn a_create_naming_no_destination_is_refused_with_the_arity_rule() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, Value)> = vec![
        // The row the report cites: nothing but the name.
        (
            "nothing but the stream name",
            json!({ "DeliveryStreamName": "arity-name-only" }),
        ),
        (
            "a name at the shortest the model allows",
            json!({ "DeliveryStreamName": "a" }),
        ),
        (
            "a name at the longest the model allows",
            json!({ "DeliveryStreamName": "z".repeat(64) }),
        ),
        (
            "the default stream type stated explicitly",
            json!({
                "DeliveryStreamName": "arity-direct-put",
                "DeliveryStreamType": "DirectPut",
            }),
        ),
        (
            "a stream type fed by a source configuration, with that configuration supplied",
            json!({
                "DeliveryStreamName": "arity-kinesis-source",
                "DeliveryStreamType": "KinesisStreamAsSource",
                "KinesisStreamSourceConfiguration": {
                    "KinesisStreamARN":
                        "arn:aws:kinesis:us-west-2:000000000000:stream/arity-source",
                    "RoleARN": ROLE_ARN,
                },
            }),
        ),
        (
            "tags but no destination",
            json!({
                "DeliveryStreamName": "arity-tagged",
                "Tags": [{ "Key": "team", "Value": "data" }],
            }),
        ),
        (
            "server-side encryption but no destination",
            json!({
                "DeliveryStreamName": "arity-encrypted",
                "DeliveryStreamEncryptionConfigurationInput": { "KeyType": "AWS_OWNED_CMK" },
            }),
        ),
    ];

    for (case, request) in rows {
        let (status, refused) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {case:?}: got {refused:?}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, ARITY_REFUSAL,
            "case {case:?}: expected {ARITY_REFUSAL:?}, got {message:?}"
        );
    }

    // Nothing was stored for any refused create.
    let (status, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(status, 200, "list: got {listed:?}");
    assert_eq!(
        listed["DeliveryStreamNames"],
        json!([]),
        "a refused create stores nothing, got {listed:?}"
    );

    // The same requests with ONE destination added are accepted: what the sentence above
    // is about is the destination's arity and not the members beside it.
    for (case, member) in [
        ("the deprecated spelling", "S3DestinationConfiguration"),
        ("the extended spelling", "ExtendedS3DestinationConfiguration"),
    ] {
        let mut request = json!({
            "DeliveryStreamName": format!("arity-accepted-{member}"),
            "Tags": [{ "Key": "team", "Value": "data" }],
        });
        request[member] = s3_configuration();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 200, "case {case:?}: got {created:?}");
    }
}
