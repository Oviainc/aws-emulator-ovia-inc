// SPDX-License-Identifier: CC0-1.0

//! The arguments a destination configuration and a batch put carry, judged over the real
//! `awsJson1_1` wire.
//!
//! Four rules are asserted here, each over a SET of inputs rather than over one request:
//!
//! 1. a `BucketARN` names an Amazon S3 bucket and nothing else. The member's modelled
//!    `@pattern` (`smithy_query(query="shape", shape="BucketARN")` ->
//!    `^arn:.*:s3:::[\w\.\-]{1,255}$`) is what says so, and a value it does not admit is
//!    refused with the constraint refusal the request boundary states for a `@pattern`
//!    violation — naming the member the value was submitted at. The rule is keyed to the
//!    shape, so it holds for every member that targets it: a destination's own bucket, the
//!    bucket of the intermediate Amazon S3 location another destination type delivers
//!    through, a backup location's, and the same members on `UpdateDestination`;
//! 2. a data processor carries the parameters its TYPE cannot run without. The model
//!    expresses `Parameters` as one generic list for every `ProcessorType`
//!    (`smithy_query(query="shape", shape="Processor")`), so which of them a given type
//!    requires is the service's rule, and a processor that omits one is refused with
//!    `InvalidArgumentException`;
//! 3. `PutRecordBatch` refuses a request whose records together exceed the documented
//!    per-request ceiling ("Each record in the request can be as large as 1,000 KB (before
//!    base64 encoding), up to a limit of 4 MB for the entire request" — `PutRecordBatch`),
//!    and refuses it WHOLE: no record of an oversized batch is accepted;
//! 4. every value `CompressionFormat` admits can be configured and is reported back, and a
//!    value outside the set is refused with a clause enumerating exactly that set;
//! 5. a customer managed CMK the stream's own account does not own cannot carry the grant
//!    Firehose needs, so it is refused as a KMS access denial — the modelled
//!    `InvalidKMSResourceException` relaying that KMS exception type.
//!
//! The harness (`Emulator`, `error_code`, …) is the lifecycle suite's.

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::COMPRESSION_FORMAT_VALUES;

use super::delivery_stream_lifecycle::{
    error_code, error_message, Emulator, BUCKET_ARN, PAST_THE_TRANSITION, REGION, ROLE_ARN,
};

/// The error code the framework answers a modelled input-constraint violation with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The error code this operation's own argument faults carry.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// `BucketARN`'s modelled `@pattern` as a refusal REPORTS it: the expression a value must
/// satisfy, with the anchoring of the model's total match normalised away — the form this
/// service states every `@pattern` requirement in.
const REPORTED_BUCKET_ARN_PATTERN: &str = r"arn:.*:s3:::[\w\.\-]{1,255}";

/// The refusal a `@pattern` violation at `path` is stated with.
fn pattern_clause(path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {REPORTED_BUCKET_ARN_PATTERN}"
    )
}

/// One place a request can carry a `BucketARN`, as the destination configuration that
/// carries it there.
struct BucketMember {
    case: &'static str,
    /// The `CreateDeliveryStream` member carrying the destination.
    member: &'static str,
    /// Builds that destination configuration with `bucket_arn` at the place under test.
    config: fn(&str) -> Value,
    /// The dotted path a refusal about that place names.
    path: &'static str,
}

/// A Redshift destination's `ClusterJDBCURL`, in the form its modelled pattern describes.
fn redshift_url() -> &'static str {
    "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev"
}

/// Every member below targets the one `BucketARN` shape, so the rule has to hold for all
/// of them — including the ones the issue did not cite (a backup location, and the
/// intermediate location of a destination that is not Amazon S3 at all).
fn bucket_members() -> Vec<BucketMember> {
    vec![
        BucketMember {
            case: "an extended Amazon S3 destination's own bucket",
            member: "ExtendedS3DestinationConfiguration",
            config: |bucket_arn| json!({ "RoleARN": ROLE_ARN, "BucketARN": bucket_arn }),
            path: "extendedS3DestinationConfiguration.bucketARN",
        },
        BucketMember {
            case: "a plain Amazon S3 destination's bucket",
            member: "S3DestinationConfiguration",
            config: |bucket_arn| json!({ "RoleARN": ROLE_ARN, "BucketARN": bucket_arn }),
            path: "s3DestinationConfiguration.bucketARN",
        },
        BucketMember {
            case: "an extended destination's backup location",
            member: "ExtendedS3DestinationConfiguration",
            config: |bucket_arn| {
                json!({
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "S3BackupMode": "Enabled",
                    "S3BackupConfiguration": { "RoleARN": ROLE_ARN, "BucketARN": bucket_arn },
                })
            },
            path: "extendedS3DestinationConfiguration.s3BackupConfiguration.bucketARN",
        },
        BucketMember {
            case: "the intermediate location of an Amazon Redshift destination",
            member: "RedshiftDestinationConfiguration",
            config: |bucket_arn| {
                json!({
                    "RoleARN": ROLE_ARN,
                    "ClusterJDBCURL": redshift_url(),
                    "CopyCommand": { "DataTableName": "events" },
                    "S3Configuration": { "RoleARN": ROLE_ARN, "BucketARN": bucket_arn },
                })
            },
            path: "redshiftDestinationConfiguration.s3Configuration.bucketARN",
        },
        BucketMember {
            case: "the intermediate location of a Splunk destination",
            member: "SplunkDestinationConfiguration",
            config: |bucket_arn| {
                json!({
                    "HECEndpoint": "https://splunk.example.com:8088",
                    "HECEndpointType": "Raw",
                    "HECToken": "hec-token",
                    "S3Configuration": { "RoleARN": ROLE_ARN, "BucketARN": bucket_arn },
                })
            },
            path: "splunkDestinationConfiguration.s3Configuration.bucketARN",
        },
    ]
}

/// One value submitted as a `BucketARN`, and whether the modelled pattern admits it.
struct BucketValue {
    case: &'static str,
    value: String,
    admitted: bool,
}

/// The values. Each refused one violates the pattern in a different way, and the admitted
/// ones span what the expression allows that the cited example did not: a dotted name, an
/// underscored one, another partition, and the bucket-name run at its published bound
/// (`{1,255}`) as well as one character past it.
fn bucket_values() -> Vec<BucketValue> {
    vec![
        BucketValue {
            case: "not an ARN at all",
            value: "firehose-destination".to_string(),
            admitted: false,
        },
        BucketValue {
            case: "an ARN that names another service",
            value: format!("arn:aws:sqs:{REGION}:000000000000:deliveries"),
            admitted: false,
        },
        BucketValue {
            case: "an Amazon S3 ARN carrying a Region and an account, which bucket ARNs do not",
            value: format!("arn:aws:s3:{REGION}:000000000000:demo-bucket"),
            admitted: false,
        },
        BucketValue {
            case: "a bucket name holding a character the expression excludes",
            value: "arn:aws:s3:::demo bucket".to_string(),
            admitted: false,
        },
        BucketValue {
            case: "an Amazon S3 ARN with no bucket name at all",
            value: "arn:aws:s3:::".to_string(),
            admitted: false,
        },
        BucketValue {
            case: "a bucket name with a path appended",
            value: "arn:aws:s3:::demo-bucket/raw".to_string(),
            admitted: false,
        },
        BucketValue {
            case: "a bucket name one character past the bound",
            value: format!("arn:aws:s3:::{}", "b".repeat(256)),
            admitted: false,
        },
        BucketValue {
            case: "a bucket name at the bound",
            value: format!("arn:aws:s3:::{}", "b".repeat(255)),
            admitted: true,
        },
        BucketValue {
            case: "a hyphenated bucket name",
            value: "arn:aws:s3:::demo-bucket".to_string(),
            admitted: true,
        },
        BucketValue {
            case: "a dotted bucket name",
            value: "arn:aws:s3:::logs.delivery.demo".to_string(),
            admitted: true,
        },
        BucketValue {
            case: "an underscored bucket name, which the expression's class admits",
            value: "arn:aws:s3:::demo_bucket".to_string(),
            admitted: true,
        },
        BucketValue {
            case: "a bucket of another partition",
            value: "arn:aws-cn:s3:::demo-bucket".to_string(),
            admitted: true,
        },
    ]
}

#[tokio::test]
async fn a_bucket_arn_is_judged_against_the_modelled_pattern_wherever_it_is_submitted() {
    let emulator = Emulator::new();

    for (member_index, member) in bucket_members().iter().enumerate() {
        for (value_index, value) in bucket_values().iter().enumerate() {
            let case = format!("{} / {}", member.case, value.case);
            let stream = format!("bucket-{member_index}-{value_index}");
            let mut request = json!({ "DeliveryStreamName": stream });
            request[member.member] = (member.config)(&value.value);
            let (status, answered) = emulator.call("CreateDeliveryStream", request).await;

            if value.admitted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be admitted, got {answered:?}"
                );
                // Admitted means STORED: the stream reports the bucket it was created with.
                let (status, described) = emulator
                    .call(
                        "DescribeDeliveryStream",
                        json!({ "DeliveryStreamName": stream }),
                    )
                    .await;
                assert_eq!(status, 200, "case {case:?}: describe got {described:?}");
                let reported = serde_json::to_string(
                    &described["DeliveryStreamDescription"]["Destinations"][0],
                )
                .unwrap_or_default();
                assert!(
                    reported.contains(value.value.as_str()),
                    "case {case:?}: expected the destination to report {:?}, got {reported}",
                    value.value
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                VALIDATION_EXCEPTION,
                "case {case:?}: got {answered:?}"
            );
            assert_eq!(
                error_message(&answered),
                pattern_clause(member.path),
                "case {case:?}: expected the clause naming {:?}, got {answered:?}",
                member.path
            );
            // A refused create wrote nothing: the stream it named does not exist.
            let (status, described) = emulator
                .call(
                    "DescribeDeliveryStream",
                    json!({ "DeliveryStreamName": stream }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected no stream to have been created, got {described:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_that_repoints_a_destination_at_a_value_that_is_no_bucket_arn_is_refused() {
    let emulator = Emulator::new();
    emulator.create_stream("bucket-update", None).await;
    // Only an ACTIVE stream's destination can be updated.
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // (case, the update member, the update, the path a refusal names)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an extended destination's own bucket",
            "ExtendedS3DestinationUpdate",
            json!({ "BucketARN": "not-an-arn" }),
            "extendedS3DestinationUpdate.bucketARN",
        ),
        (
            "a plain destination's bucket",
            "S3DestinationUpdate",
            json!({ "BucketARN": format!("arn:aws:sqs:{REGION}:000000000000:deliveries") }),
            "s3DestinationUpdate.bucketARN",
        ),
        (
            "the intermediate location of a Splunk destination",
            "SplunkDestinationUpdate",
            json!({ "S3Update": { "BucketARN": "arn:aws:s3:::bad bucket" } }),
            "splunkDestinationUpdate.s3Update.bucketARN",
        ),
    ];

    for (case, member, update, path) in rows {
        let mut request = json!({
            "DeliveryStreamName": "bucket-update",
            "CurrentDeliveryStreamVersionId": "1",
            "DestinationId": "destinationId-000000000001",
        });
        request[member] = update;
        let (status, answered) = emulator.call("UpdateDestination", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            VALIDATION_EXCEPTION,
            "case {case:?}: got {answered:?}"
        );
        assert_eq!(
            error_message(&answered),
            pattern_clause(path),
            "case {case:?}: expected the clause naming {path:?}, got {answered:?}"
        );
    }

    // The stream kept the destination it had, and its version never moved.
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "bucket-update" }),
        )
        .await;
    assert_eq!(status, 200, "describe got {described:?}");
    let description = &described["DeliveryStreamDescription"];
    assert_eq!(
        description["VersionId"],
        json!("1"),
        "a refused update must not move the version, got {description:?}"
    );
    assert_eq!(
        description["Destinations"][0]["S3DestinationDescription"]["BucketARN"],
        json!(BUCKET_ARN),
        "a refused update must leave the destination alone, got {description:?}"
    );
}

/// One `ProcessorParameter`.
fn parameter(name: &str, value: &str) -> Value {
    json!({ "ParameterName": name, "ParameterValue": value })
}

/// The Lambda function a `Lambda` processor transforms records with.
fn function_arn() -> String {
    format!("arn:aws:lambda:{REGION}:000000000000:function:transform")
}

/// One processing configuration, and the refusal it must draw — `None` when the
/// configuration is complete and the create must be admitted.
struct ProcessorRow {
    case: &'static str,
    processing: Value,
    refusal: Option<String>,
}

/// The refusal a processor missing `parameter` draws.
fn missing_parameter(parameter: &str, processor_type: &str) -> Option<String> {
    Some(format!(
        "{parameter} is required when {processor_type} processor is used."
    ))
}

/// The rows. They span the `ProcessorType` values the model declares
/// (`smithy_query(query="shape", shape="ProcessorType")`), the ways a parameter can be
/// absent (no `Parameters` member, an empty list, a list holding only other parameters),
/// the position of the incomplete processor in a pipeline, and whether data processing is
/// switched on — the configuration is judged as SUBMITTED either way, because it is stored
/// either way.
fn processor_rows() -> Vec<ProcessorRow> {
    let lambda = function_arn();
    vec![
        ProcessorRow {
            case: "a Lambda processor with no Parameters member",
            processing: json!({ "Enabled": true, "Processors": [{ "Type": "Lambda" }] }),
            refusal: missing_parameter("LambdaArn", "Lambda"),
        },
        ProcessorRow {
            case: "a Lambda processor with an empty parameter list",
            processing: json!({
                "Enabled": true,
                "Processors": [{ "Type": "Lambda", "Parameters": [] }],
            }),
            refusal: missing_parameter("LambdaArn", "Lambda"),
        },
        ProcessorRow {
            case: "a Lambda processor parameterized with everything but the function",
            processing: json!({
                "Enabled": true,
                "Processors": [{
                    "Type": "Lambda",
                    "Parameters": [
                        parameter("NumberOfRetries", "5"),
                        parameter("BufferSizeInMBs", "1"),
                    ],
                }],
            }),
            refusal: missing_parameter("LambdaArn", "Lambda"),
        },
        ProcessorRow {
            case: "an incomplete Lambda processor with data processing switched off",
            processing: json!({
                "Enabled": false,
                "Processors": [{ "Type": "Lambda", "Parameters": [] }],
            }),
            refusal: missing_parameter("LambdaArn", "Lambda"),
        },
        ProcessorRow {
            case: "a pipeline whose SECOND processor is the incomplete one",
            processing: json!({
                "Enabled": true,
                "Processors": [
                    { "Type": "AppendDelimiterToRecord" },
                    { "Type": "RecordDeAggregation" },
                ],
            }),
            refusal: missing_parameter("SubRecordType", "RecordDeAggregation"),
        },
        ProcessorRow {
            case: "a metadata extraction naming a program but no engine",
            processing: json!({
                "Enabled": true,
                "Processors": [{
                    "Type": "MetadataExtraction",
                    "Parameters": [parameter("MetadataExtractionQuery", "{dept:.dept}")],
                }],
            }),
            refusal: missing_parameter("JsonParsingEngine", "MetadataExtraction"),
        },
        ProcessorRow {
            case: "a metadata extraction naming an engine but no program",
            processing: json!({
                "Enabled": true,
                "Processors": [{
                    "Type": "MetadataExtraction",
                    "Parameters": [parameter("JsonParsingEngine", "JQ-1.6")],
                }],
            }),
            refusal: missing_parameter("MetadataExtractionQuery", "MetadataExtraction"),
        },
        ProcessorRow {
            case: "a complete Lambda processor",
            processing: json!({
                "Enabled": true,
                "Processors": [{
                    "Type": "Lambda",
                    "Parameters": [parameter("LambdaArn", &lambda)],
                }],
            }),
            refusal: None,
        },
        ProcessorRow {
            case: "a complete metadata extraction",
            processing: json!({
                "Enabled": true,
                "Processors": [{
                    "Type": "MetadataExtraction",
                    "Parameters": [
                        parameter("MetadataExtractionQuery", "{dept:.dept}"),
                        parameter("JsonParsingEngine", "JQ-1.6"),
                    ],
                }],
            }),
            refusal: None,
        },
        ProcessorRow {
            case: "a complete record de-aggregation",
            processing: json!({
                "Enabled": true,
                "Processors": [{
                    "Type": "RecordDeAggregation",
                    "Parameters": [parameter("SubRecordType", "JSON")],
                }],
            }),
            refusal: None,
        },
        ProcessorRow {
            case: "a processor type that requires no parameter of its own",
            processing: json!({
                "Enabled": true,
                "Processors": [{ "Type": "AppendDelimiterToRecord" }],
            }),
            refusal: None,
        },
        ProcessorRow {
            case: "the CloudWatch Logs pipeline, which Firehose parameterizes itself",
            processing: json!({
                "Enabled": false,
                "Processors": [
                    { "Type": "Decompression" },
                    { "Type": "CloudWatchLogProcessing" },
                ],
            }),
            refusal: None,
        },
        ProcessorRow {
            case: "a configuration that lists no processor at all",
            processing: json!({ "Enabled": true, "Processors": [] }),
            refusal: None,
        },
    ]
}

#[tokio::test]
async fn a_processor_that_omits_a_parameter_its_type_requires_is_refused() {
    let emulator = Emulator::new();

    for (index, row) in processor_rows().iter().enumerate() {
        let stream = format!("processor-{index}");
        let (status, answered) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                        "ProcessingConfiguration": row.processing.clone(),
                    },
                }),
            )
            .await;

        match row.refusal.as_deref() {
            Some(refusal) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected a refusal, got {answered:?}",
                    row.case
                );
                assert_eq!(
                    error_code(&answered),
                    INVALID_ARGUMENT,
                    "case {:?}: got {answered:?}",
                    row.case
                );
                assert_eq!(
                    error_message(&answered),
                    refusal,
                    "case {:?}: expected {refusal:?}, got {answered:?}",
                    row.case
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the create to be admitted, got {answered:?}",
                    row.case
                );
                // Admitted means stored: the processing configuration is reported back.
                let (status, described) = emulator
                    .call(
                        "DescribeDeliveryStream",
                        json!({ "DeliveryStreamName": stream }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: describe got {described:?}",
                    row.case
                );
                let processing = &described["DeliveryStreamDescription"]["Destinations"][0]
                    ["ExtendedS3DestinationDescription"]["ProcessingConfiguration"];
                assert!(
                    processing["Processors"].is_array(),
                    "case {:?}: expected the stored processors, got {described:?}",
                    row.case
                );
            }
        }
    }
}

#[tokio::test]
async fn an_update_carrying_an_incomplete_processor_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    emulator.create_stream("processor-update", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, answered) = emulator
        .call(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": "processor-update",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": {
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{ "Type": "Lambda", "Parameters": [] }],
                    },
                },
            }),
        )
        .await;
    assert_eq!(status, 400, "expected a refusal, got {answered:?}");
    assert_eq!(
        error_code(&answered),
        INVALID_ARGUMENT,
        "got {answered:?}"
    );
    assert_eq!(
        error_message(&answered),
        missing_parameter("LambdaArn", "Lambda").unwrap_or_default(),
        "got {answered:?}"
    );

    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "processor-update" }),
        )
        .await;
    assert_eq!(status, 200, "describe got {described:?}");
    assert_eq!(
        described["DeliveryStreamDescription"]["VersionId"],
        json!("1"),
        "a refused update must not move the version, got {described:?}"
    );
}

/// The documented per-request ceiling: "up to a limit of 4 MB for the entire request"
/// (`PutRecordBatch`), counted in the 1,024-byte units the modelled per-record bound of
/// the same sentence (1,000 KB -> 1,024,000 bytes) counts in.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// The sentence an oversized batch is refused with.
fn oversized_refusal() -> String {
    format!("Records size exceeds {} MB limit", MAX_REQUEST_BYTES / (1024 * 1024))
}

/// The largest ONE record may be, which bounds how a row reaches a given aggregate:
/// `Data`'s modelled `@length` max (`smithy_query(query="shape", shape="Data")`), the
/// "1,000 KB (before base64 encoding)" of the same documented sentence.
const MAX_RECORD_BYTES: usize = 1_024_000;

/// One batch put: the size of each record it carries, and whether the aggregate is within
/// the ceiling.
struct BatchRow {
    case: &'static str,
    sizes: Vec<usize>,
    accepted: bool,
}

/// The rows differ along the two dimensions the ceiling is a bound over — the record COUNT
/// and each record's SIZE — and sit either side of it, including exactly at the limit and
/// one byte past it. Every record of every row is within the modelled per-record bound, so
/// what decides a row is the aggregate and nothing else.
fn batch_rows() -> Vec<BatchRow> {
    /// The rest of a batch of [`MAX_RECORD_BYTES`] records that brings the aggregate to
    /// exactly the ceiling.
    const REMAINDER: usize = MAX_REQUEST_BYTES - 4 * MAX_RECORD_BYTES;
    vec![
        BatchRow {
            case: "a handful of large records, together within the ceiling",
            sizes: vec![1_000_000; 4],
            accepted: true,
        },
        BatchRow {
            case: "the same records with one more, together past the ceiling",
            sizes: vec![1_000_000; 5],
            accepted: false,
        },
        BatchRow {
            case: "records that together are exactly the ceiling",
            sizes: vec![
                MAX_RECORD_BYTES,
                MAX_RECORD_BYTES,
                MAX_RECORD_BYTES,
                MAX_RECORD_BYTES,
                REMAINDER,
            ],
            accepted: true,
        },
        BatchRow {
            case: "records that together are one byte past the ceiling",
            sizes: vec![
                MAX_RECORD_BYTES,
                MAX_RECORD_BYTES,
                MAX_RECORD_BYTES,
                MAX_RECORD_BYTES,
                REMAINDER + 1,
            ],
            accepted: false,
        },
        BatchRow {
            case: "the largest legal record count, each record small",
            sizes: vec![1_000; 500],
            accepted: true,
        },
        BatchRow {
            case: "the largest legal record count, together past the ceiling",
            sizes: vec![MAX_REQUEST_BYTES / 500 + 1; 500],
            accepted: false,
        },
        BatchRow {
            case: "one record of the largest legal size",
            sizes: vec![MAX_RECORD_BYTES],
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn a_batch_put_past_the_request_ceiling_is_refused_whole() {
    let emulator = Emulator::new();

    for (index, row) in batch_rows().iter().enumerate() {
        let stream = format!("batch-{index}");
        emulator.create_stream(&stream, None).await;
        // Only an ACTIVE stream accepts data.
        tokio::time::sleep(PAST_THE_TRANSITION).await;

        let records: Vec<Value> = row
            .sizes
            .iter()
            .map(|bytes| json!({ "Data": aws_smithy_types::base64::encode(vec![b'x'; *bytes]) }))
            .collect();
        let (status, answered) = emulator
            .call(
                "PutRecordBatch",
                json!({ "DeliveryStreamName": stream, "Records": records }),
            )
            .await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: expected the batch to be accepted, got {answered:?}",
                row.case
            );
            assert_eq!(
                answered["FailedPutCount"],
                json!(0),
                "case {:?}: got {answered:?}",
                row.case
            );
            assert_eq!(
                emulator.buffered(&stream).len(),
                row.sizes.len(),
                "case {:?}: expected every record to be stored",
                row.case
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {:?}: expected a refusal, got {answered:?}",
            row.case
        );
        assert_eq!(
            error_code(&answered),
            INVALID_ARGUMENT,
            "case {:?}: got {answered:?}",
            row.case
        );
        assert_eq!(
            error_message(&answered),
            oversized_refusal(),
            "case {:?}: got {answered:?}",
            row.case
        );
        // Refused WHOLE: not one record of the batch was accepted.
        assert!(
            emulator.buffered(&stream).is_empty(),
            "case {:?}: expected no record to be stored, got {:?} records",
            row.case,
            emulator.buffered(&stream).len()
        );
    }
}

#[tokio::test]
async fn a_single_record_put_is_not_bound_by_the_batch_ceiling() {
    let emulator = Emulator::new();
    emulator.create_stream("single-put", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The ceiling is the BATCH request's; one record of the largest legal size is accepted
    // by `PutRecord` whatever a batch of them would have weighed.
    let payload = aws_smithy_types::base64::encode(vec![b'y'; MAX_RECORD_BYTES]);
    let (status, answered) = emulator
        .call(
            "PutRecord",
            json!({ "DeliveryStreamName": "single-put", "Record": { "Data": payload } }),
        )
        .await;
    assert_eq!(status, 200, "expected the put to be accepted, got {answered:?}");
    assert_eq!(
        emulator.buffered("single-put").len(),
        1,
        "expected the record to be stored"
    );
}

#[tokio::test]
async fn every_value_the_compression_format_set_admits_can_be_configured_and_is_reported() {
    let emulator = Emulator::new();

    // The set is the generated one, read from the model rather than written out here, so a
    // value the service admits cannot be missing from this assertion.
    for (index, format) in COMPRESSION_FORMAT_VALUES.iter().enumerate() {
        let stream = format!("compression-{index}");
        let (status, created) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                        "CompressionFormat": format,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {format:?}: expected the create to be admitted, got {created:?}"
        );
        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": stream }),
            )
            .await;
        assert_eq!(status, 200, "case {format:?}: describe got {described:?}");
        assert_eq!(
            described["DeliveryStreamDescription"]["Destinations"][0]
                ["ExtendedS3DestinationDescription"]["CompressionFormat"],
            json!(format),
            "case {format:?}: expected the stream to report the format it was created \
             with, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_compression_format_outside_the_set_is_refused_with_the_whole_set() {
    let emulator = Emulator::new();

    for (index, value) in ["BROTLI", "gzip", ""].iter().enumerate() {
        let (status, answered) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": format!("compression-refused-{index}"),
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                        "CompressionFormat": value,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected a refusal, got {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            VALIDATION_EXCEPTION,
            "case {value:?}: got {answered:?}"
        );
        let message = error_message(&answered);
        let reported = reported_value_set(&message)
            .unwrap_or_else(|| panic!("case {value:?}: expected a value set in {message:?}"));
        let mut reported: Vec<&str> = reported;
        reported.sort_unstable();
        let mut modelled: Vec<&str> = COMPRESSION_FORMAT_VALUES.to_vec();
        modelled.sort_unstable();
        assert_eq!(
            reported, modelled,
            "case {value:?}: the refusal must enumerate exactly the values the service \
             admits, got {message:?}"
        );
    }
}

/// The values an enum-value clause enumerates, as it enumerates them.
fn reported_value_set(message: &str) -> Option<Vec<&str>> {
    let (_, set) = message.split_once('[')?;
    let (set, _) = set.split_once(']')?;
    Some(set.split(", ").collect())
}

/// The account every resource this emulator mints belongs to, which is the account a
/// stream's own CMK has to be in.
const ACCOUNT: &str = "000000000000";

/// The sentence a CMK Firehose is not permitted to use is refused with.
const KMS_ACCESS_DENIED: &str = "AccessDenied by KMS while using the CMK. Check the key \
     policy to ensure that the KMS grant related apis are allowed. Try again after the key \
     policy has the right permissions.";

/// A `CUSTOMER_MANAGED_CMK` encryption input naming a key of `account`.
fn encryption_input(account: &str) -> Value {
    json!({
        "KeyType": "CUSTOMER_MANAGED_CMK",
        "KeyARN": format!("arn:aws:kms:{REGION}:{account}:key/{}", "0".repeat(32)),
    })
}

#[tokio::test]
async fn a_cmk_outside_the_stream_s_own_account_is_refused_as_a_kms_access_denial() {
    let emulator = Emulator::new();

    // Accounts other than the emulator's own, spanning the digits a `KeyARN` admits.
    for (index, account) in ["111122223333", "999999999999", "000000000001"]
        .iter()
        .enumerate()
    {
        let (status, answered) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": format!("cmk-{index}"),
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                    },
                    "DeliveryStreamEncryptionConfigurationInput": encryption_input(account),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {account:?}: expected a refusal, got {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidKMSResourceException",
            "case {account:?}: got {answered:?}"
        );
        assert_eq!(
            answered["code"],
            json!("AccessDeniedException"),
            "case {account:?}: expected the relayed KMS exception type, got {answered:?}"
        );
        assert_eq!(
            error_message(&answered),
            KMS_ACCESS_DENIED,
            "case {account:?}: got {answered:?}"
        );
    }

    // The same rule on the other operation that takes the configuration: a started
    // encryption names a CMK too, and the account test is the same one.
    emulator.create_stream("cmk-start", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, answered) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({
                "DeliveryStreamName": "cmk-start",
                "DeliveryStreamEncryptionConfigurationInput": encryption_input("111122223333"),
            }),
        )
        .await;
    assert_eq!(status, 400, "the start got {answered:?}");
    assert_eq!(
        error_code(&answered),
        "InvalidKMSResourceException",
        "the start got {answered:?}"
    );
    assert_eq!(
        error_message(&answered),
        KMS_ACCESS_DENIED,
        "the start got {answered:?}"
    );

    // A CMK of the stream's own account is not refused by this rule: what decides it then
    // is what KMS answers about the key, which the sibling suite asserts with KMS mounted.
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "cmk-own-account",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                },
                "DeliveryStreamEncryptionConfigurationInput": encryption_input(ACCOUNT),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a CMK of this account must not be refused by the account rule, got {created:?}"
    );
}
