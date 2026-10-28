// SPDX-License-Identifier: CC0-1.0

//! What a destination configuration READS BACK as, over the real `awsJson1_1` wire.
//!
//! Two rules, each asserted over a set of inputs rather than one request:
//!
//! 1. a parameter an `UpdateDestination` does not name keeps the value the destination
//!    already held — including the parameters INSIDE a block whose model says the block
//!    outlives being switched off ("Set it to false if you want to disable format
//!    conversion while preserving the configuration details" —
//!    `DataFormatConversionConfiguration$Enabled`), so disabling conversion is not a way to
//!    lose the schema a stream is bound to;
//! 2. a destination description reports the CloudWatch logging state Firehose acts on,
//!    whichever destination type carries the block: a destination that never asked for
//!    logging reports it OFF rather than leaving the member out.
//!
//! The harness (`Emulator`, `s3_configuration`, the ARNs, `at`) is the lifecycle suite's.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    at, s3_configuration, Emulator, BUCKET_ARN, PAST_THE_TRANSITION, REGION, ROLE_ARN,
};

/// The Glue database the streams below convert against.
const DATABASE: &str = "analytics";
/// The Glue table the streams below convert against.
const TABLE: &str = "events";

/// The `DataFormatConversionConfiguration` every row of the merge table starts from: format
/// conversion ON, bound to a Glue table, reading JSON and writing Parquet.
fn conversion() -> Value {
    json!({
        "Enabled": true,
        "SchemaConfiguration": {
            "RoleARN": ROLE_ARN,
            "DatabaseName": DATABASE,
            "TableName": TABLE,
            "Region": REGION,
            "VersionId": "LATEST",
        },
        "InputFormatConfiguration": { "Deserializer": { "OpenXJsonSerDe": {} } },
        "OutputFormatConfiguration": {
            "Serializer": { "ParquetSerDe": { "Compression": "SNAPPY" } }
        },
    })
}

/// An `ExtendedS3DestinationConfiguration` with its two required members and the conversion
/// block above.
fn extended_s3_with_conversion() -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": BUCKET_ARN,
        "Prefix": "raw/",
        "DataFormatConversionConfiguration": conversion(),
    })
}

/// Create a stream whose only destination is `member`: `config`.
async fn create(emulator: &Emulator, stream: &str, member: &str, config: Value) {
    let mut request = json!({ "DeliveryStreamName": stream });
    request[member] = config;
    let (status, created) = emulator.call("CreateDeliveryStream", request).await;
    assert_eq!(status, 200, "case {stream:?}: create got {created:?}");
}

/// The `DeliveryStreamDescription` of `stream`.
async fn describe(emulator: &Emulator, stream: &str) -> Value {
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    assert_eq!(status, 200, "case {stream:?}: describe got {described:?}");
    described["DeliveryStreamDescription"].clone()
}

/// The sole `DestinationDescription` of `stream`.
async fn destination_of(emulator: &Emulator, stream: &str) -> Value {
    describe(emulator, stream).await["Destinations"][0].clone()
}

/// Apply `update` to the stream's sole destination, at the version it currently reports.
async fn update(emulator: &Emulator, stream: &str, member: &str, update: Value) -> (u16, Value) {
    let description = describe(emulator, stream).await;
    let version = description["VersionId"].as_str().unwrap_or_default();
    let destination_id = description["Destinations"][0]["DestinationId"]
        .as_str()
        .unwrap_or_default();
    let mut request = json!({
        "DeliveryStreamName": stream,
        "CurrentDeliveryStreamVersionId": version,
        "DestinationId": destination_id,
    });
    request[member] = update;
    emulator.call("UpdateDestination", request).await
}

/// One row of the merge table: a partial `ExtendedS3DestinationUpdate` applied to a stream
/// that starts from [`extended_s3_with_conversion`].
struct MergeCase {
    case: &'static str,
    stream: &'static str,
    /// The updates to apply in order, so a row can disable and re-enable conversion.
    updates: Vec<Value>,
    /// `(path inside ExtendedS3DestinationDescription, expected value)` afterwards. A
    /// `Value::Null` expectation means the member must be absent.
    expected: Vec<(&'static str, Value)>,
}

/// The rows differ along what the rule generalises over: which member of the conversion
/// block the update names, how many it names, and whether it names one at all. Every row
/// asserts the parameters the update did NOT name are still there.
fn merge_cases() -> Vec<MergeCase> {
    let kept_schema: Vec<(&'static str, Value)> = vec![
        (
            "DataFormatConversionConfiguration/SchemaConfiguration/DatabaseName",
            json!(DATABASE),
        ),
        (
            "DataFormatConversionConfiguration/SchemaConfiguration/TableName",
            json!(TABLE),
        ),
        (
            "DataFormatConversionConfiguration/SchemaConfiguration/RoleARN",
            json!(ROLE_ARN),
        ),
        (
            "DataFormatConversionConfiguration/SchemaConfiguration/Region",
            json!(REGION),
        ),
        (
            "DataFormatConversionConfiguration/SchemaConfiguration/VersionId",
            json!("LATEST"),
        ),
        (
            "DataFormatConversionConfiguration/InputFormatConfiguration/Deserializer/\
             OpenXJsonSerDe",
            json!({}),
        ),
        (
            "DataFormatConversionConfiguration/OutputFormatConfiguration/Serializer/\
             ParquetSerDe/Compression",
            json!("SNAPPY"),
        ),
    ];

    let mut cases = vec![
        // The row the report cites: conversion is switched off and nothing else is named.
        MergeCase {
            case: "conversion disabled and nothing else named",
            stream: "conversion-disabled",
            updates: vec![json!({
                "DataFormatConversionConfiguration": { "Enabled": false }
            })],
            expected: {
                let mut expected = kept_schema.clone();
                expected.push((
                    "DataFormatConversionConfiguration/Enabled",
                    json!(false),
                ));
                expected
            },
        },
        // Switched off, then back on: the block the first update preserved is the one the
        // second re-enables, so the caller never had to re-send it.
        MergeCase {
            case: "conversion disabled and then re-enabled",
            stream: "conversion-round-trip",
            updates: vec![
                json!({ "DataFormatConversionConfiguration": { "Enabled": false } }),
                json!({ "DataFormatConversionConfiguration": { "Enabled": true } }),
            ],
            expected: {
                let mut expected = kept_schema.clone();
                expected.push(("DataFormatConversionConfiguration/Enabled", json!(true)));
                expected
            },
        },
        // One member of the schema binding is renamed: the others keep their values.
        MergeCase {
            case: "only the Glue table renamed",
            stream: "conversion-retable",
            updates: vec![json!({
                "DataFormatConversionConfiguration": {
                    "SchemaConfiguration": { "TableName": "events_v2" }
                }
            })],
            expected: vec![
                (
                    "DataFormatConversionConfiguration/SchemaConfiguration/TableName",
                    json!("events_v2"),
                ),
                (
                    "DataFormatConversionConfiguration/SchemaConfiguration/DatabaseName",
                    json!(DATABASE),
                ),
                (
                    "DataFormatConversionConfiguration/SchemaConfiguration/RoleARN",
                    json!(ROLE_ARN),
                ),
                ("DataFormatConversionConfiguration/Enabled", json!(true)),
            ],
        },
        // The serializer is a choice of ONE output format, so a supplied one stands for
        // the whole choice — while the schema binding beside it is kept.
        MergeCase {
            case: "only the serializer replaced",
            stream: "conversion-reserialize",
            updates: vec![json!({
                "DataFormatConversionConfiguration": {
                    "OutputFormatConfiguration": { "Serializer": { "OrcSerDe": {} } }
                }
            })],
            expected: vec![
                (
                    "DataFormatConversionConfiguration/OutputFormatConfiguration/Serializer/\
                     OrcSerDe",
                    json!({}),
                ),
                (
                    "DataFormatConversionConfiguration/OutputFormatConfiguration/Serializer/\
                     ParquetSerDe",
                    Value::Null,
                ),
                (
                    "DataFormatConversionConfiguration/SchemaConfiguration/DatabaseName",
                    json!(DATABASE),
                ),
                ("DataFormatConversionConfiguration/Enabled", json!(true)),
            ],
        },
    ];

    // An update that names a parameter BESIDE the conversion block leaves the whole block
    // alone — the same retention rule, one level up.
    cases.push(MergeCase {
        case: "a destination parameter beside the conversion block",
        stream: "conversion-untouched",
        updates: vec![json!({ "Prefix": "curated/" })],
        expected: {
            let mut expected = kept_schema;
            expected.push(("DataFormatConversionConfiguration/Enabled", json!(true)));
            expected.push(("Prefix", json!("curated/")));
            expected
        },
    });
    cases
}

#[tokio::test]
async fn an_unnamed_conversion_parameter_keeps_the_value_the_destination_held() {
    let emulator = Emulator::new();
    let cases = merge_cases();

    for case in &cases {
        create(
            &emulator,
            case.stream,
            "ExtendedS3DestinationConfiguration",
            extended_s3_with_conversion(),
        )
        .await;
    }
    // An update is accepted only on an `ACTIVE` stream.
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in &cases {
        for (step, body) in case.updates.iter().enumerate() {
            let (status, answered) = update(
                &emulator,
                case.stream,
                "ExtendedS3DestinationUpdate",
                body.clone(),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: update {step} got {answered:?}",
                case.case
            );
        }

        let destination = destination_of(&emulator, case.stream).await;
        let described = destination["ExtendedS3DestinationDescription"].clone();
        for (path, expected) in &case.expected {
            let actual = at(&described, path);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {path} to be {expected:?}, got {actual:?} in \
                 {described:?}",
                case.case
            );
        }
    }
}

/// One destination configuration, and the paths whose `CloudWatchLoggingOptions` the
/// description must report.
struct LoggingCase {
    case: &'static str,
    stream: &'static str,
    config_member: &'static str,
    config: Value,
    /// `(path to a logging block in the description, the `Enabled` it must report)`.
    blocks: Vec<(&'static str, Value)>,
    /// `(path, expected)` for a log group or stream the configuration named, which the
    /// description keeps as given.
    named: Vec<(&'static str, Value)>,
}

/// The rows span the destination types that carry a `CloudWatchLoggingOptions` member
/// (`smithy_query(query="shape", shape="S3DestinationDescription,
/// ExtendedS3DestinationDescription,RedshiftDestinationDescription,
/// ElasticsearchDestinationDescription,SplunkDestinationDescription,
/// HttpEndpointDestinationDescription")`), the block being absent, present and off,
/// present and on, and present without `Enabled` — plus the nested Amazon S3 block a
/// destination delivers through, which carries one of its own.
fn logging_cases() -> Vec<LoggingCase> {
    let redshift_url =
        "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev";
    vec![
        // The row the report cites: an extended Amazon S3 destination that never mentions
        // CloudWatch logging.
        LoggingCase {
            case: "an extended Amazon S3 destination with no logging block",
            stream: "logging-extended-s3-absent",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            blocks: vec![(
                "ExtendedS3DestinationDescription/CloudWatchLoggingOptions/Enabled",
                json!(false),
            )],
            named: vec![],
        },
        LoggingCase {
            case: "an extended Amazon S3 destination that enabled logging",
            stream: "logging-extended-s3-on",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "CloudWatchLoggingOptions": {
                    "Enabled": true,
                    "LogGroupName": "/aws/kinesisfirehose/demo",
                    "LogStreamName": "S3Delivery",
                },
            }),
            blocks: vec![(
                "ExtendedS3DestinationDescription/CloudWatchLoggingOptions/Enabled",
                json!(true),
            )],
            named: vec![
                (
                    "ExtendedS3DestinationDescription/CloudWatchLoggingOptions/LogGroupName",
                    json!("/aws/kinesisfirehose/demo"),
                ),
                (
                    "ExtendedS3DestinationDescription/CloudWatchLoggingOptions/LogStreamName",
                    json!("S3Delivery"),
                ),
            ],
        },
        // A block supplied WITHOUT `Enabled`: the state Firehose acts on is logging off,
        // and the names it was given are kept.
        LoggingCase {
            case: "a logging block that names a group but not whether logging is on",
            stream: "logging-extended-s3-unset",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "CloudWatchLoggingOptions": { "LogGroupName": "/aws/kinesisfirehose/unset" },
            }),
            blocks: vec![(
                "ExtendedS3DestinationDescription/CloudWatchLoggingOptions/Enabled",
                json!(false),
            )],
            named: vec![(
                "ExtendedS3DestinationDescription/CloudWatchLoggingOptions/LogGroupName",
                json!("/aws/kinesisfirehose/unset"),
            )],
        },
        LoggingCase {
            case: "a plain Amazon S3 destination with no logging block",
            stream: "logging-s3-absent",
            config_member: "S3DestinationConfiguration",
            config: s3_configuration(),
            blocks: vec![(
                "S3DestinationDescription/CloudWatchLoggingOptions/Enabled",
                json!(false),
            )],
            named: vec![],
        },
        // A destination that delivers THROUGH Amazon S3 carries two logging blocks: its
        // own and the nested one. Both report the state Firehose acts on.
        LoggingCase {
            case: "a Redshift destination with logging off explicitly",
            stream: "logging-redshift",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL": redshift_url,
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": s3_configuration(),
                "CloudWatchLoggingOptions": { "Enabled": false },
            }),
            blocks: vec![
                (
                    "RedshiftDestinationDescription/CloudWatchLoggingOptions/Enabled",
                    json!(false),
                ),
                (
                    "RedshiftDestinationDescription/S3DestinationDescription/\
                     CloudWatchLoggingOptions/Enabled",
                    json!(false),
                ),
            ],
            named: vec![],
        },
        LoggingCase {
            case: "an Elasticsearch destination with no logging block",
            stream: "logging-elasticsearch",
            config_member: "ElasticsearchDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
            }),
            blocks: vec![
                (
                    "ElasticsearchDestinationDescription/CloudWatchLoggingOptions/Enabled",
                    json!(false),
                ),
                (
                    "ElasticsearchDestinationDescription/S3DestinationDescription/\
                     CloudWatchLoggingOptions/Enabled",
                    json!(false),
                ),
            ],
            named: vec![],
        },
        LoggingCase {
            case: "a Splunk destination with no logging block",
            stream: "logging-splunk",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(),
            }),
            blocks: vec![(
                "SplunkDestinationDescription/CloudWatchLoggingOptions/Enabled",
                json!(false),
            )],
            named: vec![],
        },
        LoggingCase {
            case: "an HTTP endpoint destination with logging on",
            stream: "logging-http-endpoint",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": { "Url": "https://example.com/firehose" },
                "S3Configuration": s3_configuration(),
                "CloudWatchLoggingOptions": { "Enabled": true },
            }),
            blocks: vec![(
                "HttpEndpointDestinationDescription/CloudWatchLoggingOptions/Enabled",
                json!(true),
            )],
            named: vec![],
        },
    ]
}

#[tokio::test]
async fn every_destination_description_reports_whether_cloudwatch_logging_is_on() {
    let emulator = Emulator::new();

    for case in logging_cases() {
        create(
            &emulator,
            case.stream,
            case.config_member,
            case.config.clone(),
        )
        .await;
        let destination = destination_of(&emulator, case.stream).await;
        for (path, expected) in &case.blocks {
            let actual = at(&destination, path);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {path} to be {expected:?}, got {actual:?} in \
                 {destination:?}",
                case.case
            );
        }
        for (path, expected) in &case.named {
            let actual = at(&destination, path);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {path} to be {expected:?}, got {actual:?} in \
                 {destination:?}",
                case.case
            );
        }
    }
}
