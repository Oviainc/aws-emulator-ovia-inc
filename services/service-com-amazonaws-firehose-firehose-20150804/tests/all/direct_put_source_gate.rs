// SPDX-License-Identifier: CC0-1.0

//! The SOURCE-TYPE gate on the data plane, over the real `awsJson1_1` wire.
//!
//! The rule: `PutRecord` and `PutRecordBatch` are the direct-put ingestion path, and a
//! Firehose stream takes direct puts only when its source IS direct puts. `DirectPut` is
//! the one `DeliveryStreamType` of which "provider applications access the Firehose stream
//! directly" (`CreateDeliveryStreamInput$DeliveryStreamType`); every other value names the
//! service that feeds the stream instead, so a direct put to such a stream is refused with
//! the modelled `InvalidArgumentException` (HTTP 400) and nothing is buffered. The refusal
//! names the stream's own `DeliveryStreamType`, so it states which configuration stands in
//! the way.
//!
//! The gate is a property of the stream's stored type, so the table below spans every
//! value of `DeliveryStreamType`
//! (`smithy_query(query="shape",shape="DeliveryStreamType")` -> ["DirectPut",
//! "KinesisStreamAsSource", "MSKAsSource", "DatabaseAsSource"]), the type left absent (it
//! defaults to `DirectPut`), the `DirectPutSourceConfiguration` a direct-put stream may
//! carry, and two destination types; a second test pins that it does not depend on the
//! payload.
//!
//! The harness is [`crate::delivery_stream_lifecycle`]', reused rather than copied.

use serde_json::{json, Value};

use super::delivery_stream_lifecycle::{
    at, error_code, error_message, s3_configuration, Emulator, BUCKET_ARN, PAST_THE_TRANSITION,
    REGION, ROLE_ARN,
};

/// The `DeliveryStreamType` whose producer is a caller of these two operations.
const DIRECT_PUT: &str = "DirectPut";

/// A `KinesisStreamSourceConfiguration`: the ARN of the Kinesis data stream that feeds the
/// Firehose stream and the role Firehose reads it under.
fn kinesis_source(name: &str) -> Value {
    json!({
        "KinesisStreamARN": format!("arn:aws:kinesis:{REGION}:000000000000:stream/{name}"),
        "RoleARN": ROLE_ARN,
    })
}

/// An `MSKSourceConfiguration`: the cluster, the topic and the authentication Firehose
/// reads it with.
fn msk_source(name: &str) -> Value {
    json!({
        "MSKClusterARN": format!("arn:aws:kafka:{REGION}:000000000000:cluster/{name}/1"),
        "TopicName": "events",
        "AuthenticationConfiguration": { "RoleARN": ROLE_ARN, "Connectivity": "PRIVATE" },
    })
}

/// A `DatabaseSourceConfiguration`, every @required member of it
/// (`smithy_query(query="shape",shape="DatabaseSourceConfiguration")`): the engine type
/// (`DatabaseType` is `MySQL` | `PostgreSQL`), the endpoint and port, the database and
/// table patterns, the watermark table, the secret Firehose authenticates with and the VPC
/// endpoint service it reaches the database through.
fn database_source() -> Value {
    json!({
        "Type": "MySQL",
        "Endpoint": "db.example.internal",
        "Port": 3306,
        "SSLMode": "Enabled",
        "Databases": { "Include": ["sales"] },
        "Tables": { "Include": ["orders"] },
        "SnapshotWatermarkTable": "sales.firehose_watermark",
        "DatabaseSourceAuthenticationConfiguration": {
            "SecretsManagerConfiguration": {
                "SecretARN":
                    format!("arn:aws:secretsmanager:{REGION}:000000000000:secret:db-credentials"),
                "RoleARN": ROLE_ARN,
                "Enabled": true,
            },
        },
        "DatabaseSourceVPCConfiguration": {
            "VpcEndpointServiceName":
                "com.amazonaws.vpce.us-west-2.vpce-svc-01234567890abcdef",
        },
    })
}

/// One row of the gate table: a stream created with the declared type and the source
/// configuration that type is configured with, put to directly.
struct Case {
    /// The stream name, which is also the case label.
    stream: &'static str,
    /// The `DeliveryStreamType` the create declares, or `None` to leave the member out.
    stream_type: Option<&'static str>,
    /// The `CreateDeliveryStream` member carrying the destination, and its value.
    destination_member: &'static str,
    destination: Value,
    /// The `DeliveryStreamType` the stream ends up carrying — what a describe reports and,
    /// for a refused case, what the refusal names. `DirectPut` for the absent type.
    stored_type: &'static str,
}

/// The table. The types are every value of `DeliveryStreamType`; the destination members
/// are two of the ones `smithy_query(query="operation",
/// operation="CreateDeliveryStream")` lists, so a row's outcome cannot come from its
/// destination.
fn cases() -> Vec<Case> {
    vec![
        // --- a stream whose source IS direct puts keeps taking them.
        Case {
            stream: "direct-put-declared",
            stream_type: Some(DIRECT_PUT),
            destination_member: "S3DestinationConfiguration",
            destination: s3_configuration(),
            stored_type: DIRECT_PUT,
        },
        // The member left out: "DeliveryStreamType" defaults to DirectPut.
        Case {
            stream: "direct-put-by-default",
            stream_type: None,
            destination_member: "ExtendedS3DestinationConfiguration",
            destination: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            stored_type: DIRECT_PUT,
        },
        // --- a stream fed by another service does not take them.
        Case {
            stream: "kinesis-backed",
            stream_type: Some("KinesisStreamAsSource"),
            destination_member: "S3DestinationConfiguration",
            destination: s3_configuration(),
            stored_type: "KinesisStreamAsSource",
        },
        // The same type with the other destination: the gate is the source, not the sink.
        Case {
            stream: "kinesis-backed-extended-s3",
            stream_type: Some("KinesisStreamAsSource"),
            destination_member: "ExtendedS3DestinationConfiguration",
            destination: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            stored_type: "KinesisStreamAsSource",
        },
        Case {
            stream: "msk-backed",
            stream_type: Some("MSKAsSource"),
            destination_member: "S3DestinationConfiguration",
            destination: s3_configuration(),
            stored_type: "MSKAsSource",
        },
        Case {
            stream: "database-backed",
            stream_type: Some("DatabaseAsSource"),
            destination_member: "S3DestinationConfiguration",
            destination: s3_configuration(),
            stored_type: "DatabaseAsSource",
        },
    ]
}

/// The source configuration member the declared type is configured with, for a stream
/// named `name`; `None` for a type that needs none.
fn source_of(stream_type: Option<&str>, name: &str) -> Option<(&'static str, Value)> {
    match stream_type {
        Some("KinesisStreamAsSource") => {
            Some(("KinesisStreamSourceConfiguration", kinesis_source(name)))
        }
        Some("MSKAsSource") => Some(("MSKSourceConfiguration", msk_source(name))),
        Some("DatabaseAsSource") => Some(("DatabaseSourceConfiguration", database_source())),
        // `DirectPutSourceConfiguration`'s one @required member is a throughput hint, range
        // 1..100 (`smithy_query(query="shape",shape="DirectPutSourceConfiguration")`). It
        // is optional for the type, which the absent-type row above covers.
        Some(DIRECT_PUT) => Some((
            "DirectPutSourceConfiguration",
            json!({ "ThroughputHintInMBs": 1 }),
        )),
        _ => None,
    }
}

/// A record body, base64 as the wire blob requires.
fn record(payload: &[u8]) -> Value {
    json!({ "Data": aws_smithy_types::base64::encode(payload) })
}

/// `PutRecord` one payload into `stream`.
async fn put_record(emulator: &Emulator, stream: &str, payload: &[u8]) -> (u16, Value) {
    emulator
        .call(
            "PutRecord",
            json!({ "DeliveryStreamName": stream, "Record": record(payload) }),
        )
        .await
}

/// The refusal a direct put to a stream with a configured source reads, built from the one
/// fact the handler builds it from: the `DeliveryStreamType` the stream carries.
fn expected_refusal(stream_type: &str) -> String {
    format!("This operation is not permitted on {stream_type} delivery stream type.")
}

/// Assert `answer` is the modelled `InvalidArgumentException` naming `stream_type`.
fn assert_not_permitted(case: &str, stream_type: &str, status: u16, answer: &Value) {
    assert_eq!(status, 400, "case {case:?}: got {status} with {answer:?}");
    assert_eq!(
        error_code(answer),
        "InvalidArgumentException",
        "case {case:?}: got {answer:?}"
    );
    let message = error_message(answer);
    let expected = expected_refusal(stream_type);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_direct_put_is_served_exactly_when_the_streams_source_is_direct_puts() {
    let emulator = Emulator::new();
    let cases = cases();

    // Every stream is created first, so ONE wait covers every creation span.
    for case in &cases {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        if let Some(stream_type) = case.stream_type {
            request["DeliveryStreamType"] = json!(stream_type);
        }
        if let Some((member, configuration)) = source_of(case.stream_type, case.stream) {
            request[member] = configuration;
        }
        request[case.destination_member] = case.destination.clone();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 200, "case {:?}: create got {created:?}", case.stream);
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in &cases {
        let stream = case.stream;
        let refused = case.stored_type != DIRECT_PUT;

        // The type the gate and the refusal read is the stored one, which the describe
        // reports: the stream is ACTIVE and carries the type its create declared.
        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": stream }),
            )
            .await;
        assert_eq!(status, 200, "case {stream:?}: describe got {described:?}");
        assert_eq!(
            at(&described, "DeliveryStreamDescription/DeliveryStreamStatus"),
            &json!("ACTIVE"),
            "case {stream:?}: the stream is ACTIVE before the put, got {described:?}"
        );
        assert_eq!(
            at(&described, "DeliveryStreamDescription/DeliveryStreamType"),
            &json!(case.stored_type),
            "case {stream:?}: the stored type is what the create declared, got {described:?}"
        );

        let (status, put) = put_record(&emulator, stream, b"a payload").await;
        let (batch_status, batch) = emulator
            .call(
                "PutRecordBatch",
                json!({
                    "DeliveryStreamName": stream,
                    "Records": [record(b"first"), record(b"second")],
                }),
            )
            .await;
        let buffered = emulator.buffered(stream).len();

        if refused {
            assert_not_permitted(stream, case.stored_type, status, &put);
            // The whole batch is refused: the gate is a property of the stream, so no
            // record of the batch is one the stream would have taken.
            assert_not_permitted(stream, case.stored_type, batch_status, &batch);
            assert_eq!(
                buffered, 0,
                "case {stream:?}: a refused put buffers nothing, got {buffered} records"
            );
        } else {
            assert_eq!(status, 200, "case {stream:?}: put got {put:?}");
            assert!(
                put["RecordId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {stream:?}: an accepted put answers a RecordId, got {put:?}"
            );
            assert_eq!(batch_status, 200, "case {stream:?}: batch got {batch:?}");
            assert_eq!(
                batch["FailedPutCount"],
                json!(0),
                "case {stream:?}: batch got {batch:?}"
            );
            assert_eq!(
                buffered, 3,
                "case {stream:?}: the accepted put and batch are buffered, got {buffered} \
                 records"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_does_not_depend_on_the_payload() {
    let emulator = Emulator::new();
    let stream = "payload-source-gate";
    emulator
        .create_stream(stream, Some("KinesisStreamAsSource"))
        .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The payloads differ in size and in form: the gate is the stream's source, not the
    // bytes a producer sends.
    let payloads: Vec<(&str, Vec<u8>)> = vec![
        ("one byte", b"x".to_vec()),
        ("plain text", b"a line of log text".to_vec()),
        ("json", br#"{"event":"click"}"#.to_vec()),
        ("64 KiB of zeroes", vec![0u8; 64 * 1024]),
    ];
    for (case, payload) in &payloads {
        let (status, answer) = put_record(&emulator, stream, payload).await;
        assert_not_permitted(case, "KinesisStreamAsSource", status, &answer);
    }
    assert!(
        emulator.buffered(stream).is_empty(),
        "no payload is accepted by a stream a Kinesis data stream feeds"
    );
}
