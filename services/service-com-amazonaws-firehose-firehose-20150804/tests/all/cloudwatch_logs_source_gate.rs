// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch-Logs source gate on the data plane, over the real `awsJson1_1` wire.
//!
//! The rule: a Firehose stream whose destination has data processing ENABLED with a
//! processor of the CloudWatch Logs decompression pipeline (`Decompression`,
//! `CloudWatchLogProcessing` — `ProcessorType` values whose input only a CloudWatch Logs
//! producer can supply) takes data from CloudWatch Logs only, so a direct `PutRecord` /
//! `PutRecordBatch` is refused with `InvalidSourceException` (HTTP 400): "Only requests
//! from CloudWatch Logs are supported when CloudWatch Logs decompression is enabled."
//! Every other stream keeps taking records.
//!
//! The gate is a property of the stream's stored processing configuration, so the table
//! below spans the destination types that carry one, the processor types the model
//! declares, whether processing is enabled, and the position of the processor in the list;
//! two further tests pin that it does not depend on the payload and that it follows the
//! configuration a later `UpdateDestination` leaves behind.

use serde_json::{json, Value};

use super::delivery_stream_lifecycle::{
    error_code, s3_configuration, snowflake_private_key, Emulator, BUCKET_ARN, PAST_THE_TRANSITION,
    REGION, ROLE_ARN,
};
use super::refusal_messages::error_message;

/// One `Processor` of a `ProcessingConfiguration`.
fn processor(kind: &str, parameters: Value) -> Value {
    json!({ "Type": kind, "Parameters": parameters })
}

/// The `Decompression` processor: it inflates the gzip payload CloudWatch Logs delivers.
fn decompression() -> Value {
    processor(
        "Decompression",
        json!([{ "ParameterName": "CompressionFormat", "ParameterValue": "GZIP" }]),
    )
}

/// The `CloudWatchLogProcessing` processor: it extracts the log events out of a CloudWatch
/// Logs `DataMessage` envelope.
fn cloudwatch_log_processing() -> Value {
    processor(
        "CloudWatchLogProcessing",
        json!([{ "ParameterName": "DataMessageExtraction", "ParameterValue": "true" }]),
    )
}

/// The `Lambda` processor — a transform that says nothing about the producer.
fn lambda() -> Value {
    processor(
        "Lambda",
        json!([{
            "ParameterName": "LambdaArn",
            "ParameterValue": format!("arn:aws:lambda:{REGION}:000000000000:function:transform"),
        }]),
    )
}

/// The `AppendDelimiterToRecord` processor.
fn append_delimiter() -> Value {
    processor(
        "AppendDelimiterToRecord",
        json!([{ "ParameterName": "Delimiter", "ParameterValue": "\\n" }]),
    )
}

/// The `MetadataExtraction` processor.
fn metadata_extraction() -> Value {
    processor(
        "MetadataExtraction",
        json!([
            { "ParameterName": "MetadataExtractionQuery", "ParameterValue": "{dept:.dept}" },
            { "ParameterName": "JsonParsingEngine", "ParameterValue": "JQ-1.6" },
        ]),
    )
}

/// The `RecordDeAggregation` processor.
fn record_de_aggregation() -> Value {
    processor(
        "RecordDeAggregation",
        json!([{ "ParameterName": "SubRecordType", "ParameterValue": "JSON" }]),
    )
}

/// A `ProcessingConfiguration` with `Enabled` as given (absent for `None`).
fn processing(enabled: Option<bool>, processors: Vec<Value>) -> Value {
    let mut configuration = json!({ "Processors": processors });
    if let Some(enabled) = enabled {
        configuration["Enabled"] = json!(enabled);
    }
    configuration
}

/// An `ExtendedS3DestinationConfiguration` carrying `processing`.
fn extended_s3(processing: Value) -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": BUCKET_ARN,
        "ProcessingConfiguration": processing,
    })
}

/// One row of the gate table: a stream created with one destination, put to directly.
struct Case {
    /// The stream name, which is also the case label.
    stream: &'static str,
    /// The `CreateDeliveryStream` member carrying the destination.
    config_member: &'static str,
    config: Value,
    /// Whether a direct API put must be refused with `InvalidSourceException`.
    refused: bool,
}

/// The table. The destination members are the ones
/// `smithy_query(query="operation", operation="CreateDeliveryStream")` lists, and the
/// processor types are every value of `ProcessorType`
/// (`smithy_query(query="shape", shape="ProcessorType")`: `RecordDeAggregation`,
/// `Decompression`, `CloudWatchLogProcessing`, `Lambda`, `MetadataExtraction`,
/// `AppendDelimiterToRecord`).
fn cases() -> Vec<Case> {
    vec![
        // --- a CloudWatch-Logs-only pipeline, across the destination types that carry a
        // ProcessingConfiguration and across the two processors that make it up.
        Case {
            stream: "gate-extended-s3-log-processing",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![cloudwatch_log_processing()])),
            refused: true,
        },
        Case {
            stream: "gate-extended-s3-decompression",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![decompression()])),
            refused: true,
        },
        // The whole pipeline as the console configures it, with the gating processors in
        // the middle and at the end of a longer list: position does not matter.
        Case {
            stream: "gate-extended-s3-full-pipeline",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(
                Some(true),
                vec![
                    decompression(),
                    cloudwatch_log_processing(),
                    append_delimiter(),
                ],
            )),
            refused: true,
        },
        Case {
            stream: "gate-extended-s3-last-in-list",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(
                Some(true),
                vec![lambda(), metadata_extraction(), cloudwatch_log_processing()],
            )),
            refused: true,
        },
        Case {
            stream: "gate-redshift",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL":
                    "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(Some(true), vec![decompression()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-elasticsearch",
            config_member: "ElasticsearchDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "DomainARN": format!("arn:aws:es:{REGION}:000000000000:domain/logs-domain"),
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration":
                    processing(Some(true), vec![cloudwatch_log_processing()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-opensearch",
            config_member: "AmazonopensearchserviceDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterEndpoint": "https://search.example.com",
                "IndexName": "docs",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(Some(true), vec![decompression()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-splunk",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration":
                    processing(Some(true), vec![cloudwatch_log_processing()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-http-endpoint",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": {
                    "Url": "https://example.com/firehose",
                    "Name": "example",
                },
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(Some(true), vec![decompression()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-snowflake",
            config_member: "SnowflakeDestinationConfiguration",
            config: json!({
                "AccountUrl": "my-account-id.snowflakecomputing.com",
                "PrivateKey": snowflake_private_key(),
                "User": "loader",
                "Database": "analytics",
                "Schema": "public",
                "Table": "events",
                "RoleARN": ROLE_ARN,
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration":
                    processing(Some(true), vec![cloudwatch_log_processing()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-serverless",
            config_member: "AmazonOpenSearchServerlessDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "CollectionEndpoint": "https://collection.example.com",
                "IndexName": "serverless-index",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(Some(true), vec![decompression()]),
            }),
            refused: true,
        },
        Case {
            stream: "gate-iceberg",
            config_member: "IcebergDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "CatalogConfiguration": {
                    "CatalogARN": format!("arn:aws:glue:{REGION}:000000000000:catalog"),
                },
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration":
                    processing(Some(true), vec![cloudwatch_log_processing()]),
            }),
            refused: true,
        },
        // --- everything else keeps taking records.
        // A plain Amazon S3 destination: the one destination the model gives no
        // `ProcessingConfiguration` member at all.
        Case {
            stream: "open-plain-s3",
            config_member: "S3DestinationConfiguration",
            config: s3_configuration(),
            refused: false,
        },
        // An extended Amazon S3 destination with no processing configured.
        Case {
            stream: "open-extended-s3-no-processing",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            refused: false,
        },
        // Processing enabled with each processor type that is NOT part of the CloudWatch
        // Logs pipeline.
        Case {
            stream: "open-lambda",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![lambda()])),
            refused: false,
        },
        Case {
            stream: "open-append-delimiter",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![append_delimiter()])),
            refused: false,
        },
        Case {
            stream: "open-metadata-extraction",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![metadata_extraction()])),
            refused: false,
        },
        Case {
            stream: "open-record-de-aggregation",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![record_de_aggregation()])),
            refused: false,
        },
        Case {
            stream: "open-every-other-processor",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(
                Some(true),
                vec![
                    record_de_aggregation(),
                    lambda(),
                    metadata_extraction(),
                    append_delimiter(),
                ],
            )),
            refused: false,
        },
        // An empty processor list processes nothing.
        Case {
            stream: "open-no-processors",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(true), vec![])),
            refused: false,
        },
        // Processing turned OFF: "Enables or disables data processing"
        // (`ProcessingConfiguration$Enabled`), so the listed processors are not in force.
        Case {
            stream: "open-disabled-log-processing",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(Some(false), vec![cloudwatch_log_processing()])),
            refused: false,
        },
        Case {
            stream: "open-disabled-decompression",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Raw",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration":
                    processing(Some(false), vec![decompression(), cloudwatch_log_processing()]),
            }),
            refused: false,
        },
        // `Enabled` absent is not enabled either.
        Case {
            stream: "open-unset-enabled",
            config_member: "ExtendedS3DestinationConfiguration",
            config: extended_s3(processing(None, vec![decompression()])),
            refused: false,
        },
    ]
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
            json!({
                "DeliveryStreamName": stream,
                "Record": record(payload),
            }),
        )
        .await
}

/// The refusal a direct put to a CloudWatch-Logs-only stream reads: the account the put
/// was scoped to and the stream it named.
///
/// Derived here from the same two facts the handler builds it from (the emulator account
/// and the caller's stream name) and spelled in the documented sentence form AWS answers,
/// so the assertion is a statement about the message and not a read of the code under
/// test.
fn expected_refusal(stream: &str) -> String {
    format!(
        "Put to Firehose failed for AccountId: {}, FirehoseName: {stream} \
         because the request is not originating from CloudWatch.",
        engine::emulator_account_id()
    )
}

/// Assert `answer` is the modelled `InvalidSourceException` for `stream`.
fn assert_invalid_source(case: &str, stream: &str, status: u16, answer: &Value) {
    assert_eq!(status, 400, "case {case:?}: got {answer:?}");
    assert_eq!(
        error_code(answer),
        "InvalidSourceException",
        "case {case:?}: got {answer:?}"
    );
    let message = error_message(answer);
    let expected = expected_refusal(stream);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_direct_put_is_refused_exactly_when_the_stream_enables_cloudwatch_logs_processing() {
    let emulator = Emulator::new();
    let cases = cases();

    // Every stream is created first, so ONE wait covers every creation span.
    for case in &cases {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.config_member] = case.config.clone();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: create got {created:?}",
            case.stream
        );
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in &cases {
        let stream = case.stream;
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

        if case.refused {
            assert_invalid_source(stream, stream, status, &put);
            // The whole batch is refused: the gate is a property of the stream, so no
            // record of the batch is one the stream would have taken.
            assert_invalid_source(stream, stream, batch_status, &batch);
            assert_eq!(
                buffered, 0,
                "case {stream:?}: a refused put stores nothing, got {buffered} buffered records"
            );
        } else {
            assert_eq!(status, 200, "case {stream:?}: put got {put:?}");
            assert!(
                put["RecordId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {stream:?}: an accepted put answers a RecordId, got {put:?}"
            );
            assert_eq!(
                batch_status, 200,
                "case {stream:?}: put batch got {batch:?}"
            );
            assert_eq!(
                batch["FailedPutCount"],
                json!(0),
                "case {stream:?}: put batch got {batch:?}"
            );
            assert_eq!(
                batch["RequestResponses"].as_array().map(Vec::len),
                Some(2),
                "case {stream:?}: one response entry per record, got {batch:?}"
            );
            assert_eq!(
                buffered, 3,
                "case {stream:?}: the accepted put and batch are buffered, \
                 got {buffered} buffered records"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_does_not_depend_on_the_payload() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "payload-gate",
                "ExtendedS3DestinationConfiguration":
                    extended_s3(processing(Some(true), vec![decompression()])),
            }),
        )
        .await;
    assert_eq!(status, 200, "create payload-gate: got {created:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The payloads differ in size, in form and in whether they even look like something
    // the CloudWatch Logs pipeline could process: the gate is the stream's configuration,
    // not the bytes.
    let payloads: Vec<(&str, Vec<u8>)> = vec![
        ("one byte", b"x".to_vec()),
        ("plain text", b"a line of log text".to_vec()),
        (
            "a CloudWatch Logs data message",
            br#"{"messageType":"DATA_MESSAGE","logEvents":[{"message":"hi"}]}"#.to_vec(),
        ),
        // A gzip member's magic bytes, which is what CloudWatch Logs actually delivers.
        ("gzip bytes", vec![0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00]),
        ("64 KiB of zeroes", vec![0u8; 64 * 1024]),
    ];

    for (case, payload) in &payloads {
        let (status, answer) = put_record(&emulator, "payload-gate", payload).await;
        assert_invalid_source(case, "payload-gate", status, &answer);
    }
    assert!(
        emulator.buffered("payload-gate").is_empty(),
        "no payload is accepted by a CloudWatch-Logs-only stream"
    );
}

#[tokio::test]
async fn the_gate_follows_the_processing_configuration_the_stream_currently_holds() {
    let emulator = Emulator::new();
    // Created with a processor that says nothing about the producer: puts are accepted.
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "updated-gate",
                "ExtendedS3DestinationConfiguration":
                    extended_s3(processing(Some(true), vec![lambda()])),
            }),
        )
        .await;
    assert_eq!(status, 200, "create updated-gate: got {created:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, accepted) = put_record(&emulator, "updated-gate", b"before").await;
    assert_eq!(
        status, 200,
        "created with a Lambda processor: got {accepted:?}"
    );

    // `UpdateDestination` replaces the processing configuration; the gate is computed from
    // what the stream holds afterwards, which the description reports.
    let (status, updated) = emulator
        .call(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": "updated-gate",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": {
                    "ProcessingConfiguration":
                        processing(Some(true), vec![decompression(), cloudwatch_log_processing()]),
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "update to the log pipeline: got {updated:?}");

    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "updated-gate" }),
        )
        .await;
    assert_eq!(status, 200, "describe after the update: got {described:?}");
    let processors = &described["DeliveryStreamDescription"]["Destinations"][0]
        ["ExtendedS3DestinationDescription"]["ProcessingConfiguration"]["Processors"];
    assert_eq!(
        processors[0]["Type"],
        json!("Decompression"),
        "describe after the update: got {described:?}"
    );
    assert_eq!(
        processors[1]["Type"],
        json!("CloudWatchLogProcessing"),
        "describe after the update: got {described:?}"
    );

    let (status, refused) = put_record(&emulator, "updated-gate", b"after").await;
    assert_invalid_source("after the update", "updated-gate", status, &refused);

    // And back: a configuration without a CloudWatch Logs processor opens the stream again.
    let (status, reopened) = emulator
        .call(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": "updated-gate",
                "CurrentDeliveryStreamVersionId": "2",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": {
                    "ProcessingConfiguration": processing(Some(true), vec![lambda()]),
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "update back off the log pipeline: got {reopened:?}"
    );
    let (status, accepted_again) = put_record(&emulator, "updated-gate", b"reopened").await;
    assert_eq!(
        status, 200,
        "after the processor is removed: got {accepted_again:?}"
    );

    // Only the two accepted puts were buffered.
    let buffered: Vec<String> = emulator
        .buffered("updated-gate")
        .into_iter()
        .map(|(_, data)| String::from_utf8_lossy(&data).to_string())
        .collect();
    assert_eq!(
        buffered,
        vec!["before".to_string(), "reopened".to_string()],
        "only the puts made while the stream took direct data are buffered"
    );
}
