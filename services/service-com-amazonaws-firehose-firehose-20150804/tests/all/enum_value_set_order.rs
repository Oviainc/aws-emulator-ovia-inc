// SPDX-License-Identifier: CC0-1.0

//! How this service STATES an enumerated type's admissible set when it refuses a value
//! outside it, over the real `awsJson1_1` wire.
//!
//! The rule: a value outside a modelled enumerated type is refused at the request
//! boundary with `ValidationException` (HTTP 400) and the clause `Value at '<member path>'
//! failed to satisfy constraint: Member must satisfy enum value set: [<values>]`, where
//! `<values>` is the type's WHOLE membership listed in the order the set's own hash table
//! is walked. That order belongs to the enumerated TYPE: one type reads the same way under
//! every member targeting it — any operation, any destination, any depth, any list index —
//! and it is neither the order the model declares the values in nor a sort of them.
//!
//! The rows differ along what the rule generalises over: which enumerated type failed
//! (`ProcessorType`, `ProcessorParameterName`, `CompressionFormat`, `HECEndpointType`),
//! which operation carried it (`CreateDeliveryStream`, `UpdateDestination`), which
//! destination member it sat under (extended Amazon S3, Redshift, Splunk), how deep it sat
//! (a destination member, a list element, a list element inside a list element) and which
//! 1-based index of the list it sat at.
//!
//! The expected `ProcessorType` set is the order Firehose's own refusal states it in; every
//! stated set's MEMBERSHIP is checked against the model-derived value lists in `ops::types`
//! (`smithy_query(query="shape", shape="ProcessorType,ProcessorParameterName,
//! CompressionFormat,HECEndpointType")`), so a listing here cannot drift from the type the
//! model declares.
//!
//! The last two tests cover the other half of the same request: an incomplete processor is
//! Firehose's own SEMANTIC verdict (`InvalidArgumentException`), which must not share a
//! code with the structural refusal above, and neither refusal may leave a stream behind —
//! while the configuration that names no processor at all is admitted and echoed.
//!
//! The harness (`Emulator`, `s3_configuration`, …) is the lifecycle suite's.

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::{
    COMPRESSION_FORMAT_VALUES, H_E_C_ENDPOINT_TYPE_VALUES, PROCESSOR_PARAMETER_NAME_VALUES,
    PROCESSOR_TYPE_VALUES,
};

use super::delivery_stream_lifecycle::{
    error_code, error_message, s3_configuration, Emulator, BUCKET_ARN, PAST_THE_TRANSITION, REGION,
    ROLE_ARN,
};

/// The code a request refused at the model's boundary carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The code Firehose's own verdict on a processor it could never run carries — a different
/// code, so a client can tell a structural request error from a semantic one.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// The order Firehose states `ProcessorType`'s set in, read off its own refusal of a
/// processor type outside the set. Its membership is the model's `ProcessorType`
/// (`PROCESSOR_TYPE_VALUES`), and the order is neither that declaration order
/// (`RecordDeAggregation, Decompression, CloudWatchLogProcessing, Lambda,
/// MetadataExtraction, AppendDelimiterToRecord`) nor a sort of the values.
const PROCESSOR_TYPE_SET: &str = "Decompression, Lambda, RecordDeAggregation, \
                                  MetadataExtraction, AppendDelimiterToRecord, \
                                  CloudWatchLogProcessing";

/// A value no enumerated type of this model declares, so every row below breaks exactly
/// one enum constraint and nothing else.
const OFF_ENUM: &str = "NotAModelledValue";

/// One `Processor` of a type the model declares, complete enough that nothing but the
/// member under test is in violation.
fn lambda_processor() -> Value {
    json!({
        "Type": "Lambda",
        "Parameters": [{
            "ParameterName": "LambdaArn",
            "ParameterValue": format!("arn:aws:lambda:{REGION}:000000000000:function:transform"),
        }],
    })
}

/// One row: the request to make, the member path the refused value sat at, the set the
/// refusal must state, and the model-derived list of that type's values.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    path: &'static str,
    value_set: Option<&'static str>,
    declared: &'static [&'static str],
}

/// Every row: four enumerated types, two operations, three destination members, three
/// depths and two list indices.
fn rows() -> Vec<Row> {
    vec![
        // The shape the report cites: a processor type outside the enum, at the first
        // element of an extended Amazon S3 destination's processor list.
        Row {
            case: "a processor type outside the enum on a create",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-processor-type",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{
                            "Type": OFF_ENUM,
                            "Parameters": [{
                                "ParameterName": "LambdaArn",
                                "ParameterValue": "x",
                            }],
                        }],
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.processingConfiguration.processors.1.member.type",
            value_set: Some(PROCESSOR_TYPE_SET),
            declared: PROCESSOR_TYPE_VALUES,
        },
        // The SAME type one list element further along: the stated set does not depend on
        // where in the list the value sat.
        Row {
            case: "a processor type outside the enum at the second element",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-processor-type-second",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [lambda_processor(), json!({ "Type": OFF_ENUM })],
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.processingConfiguration.processors.2.member.type",
            value_set: Some(PROCESSOR_TYPE_SET),
            declared: PROCESSOR_TYPE_VALUES,
        },
        // … and under another destination member, which carries its own
        // `ProcessingConfiguration`.
        Row {
            case: "a processor type outside the enum under a Redshift destination",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-processor-type-redshift",
                "RedshiftDestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "ClusterJDBCURL":
                        "jdbc:redshift://example.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                    "CopyCommand": { "DataTableName": "events" },
                    "S3Configuration": s3_configuration(),
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{ "Type": OFF_ENUM }],
                    },
                },
            }),
            path: "redshiftDestinationConfiguration.processingConfiguration.processors.1.member.type",
            value_set: Some(PROCESSOR_TYPE_SET),
            declared: PROCESSOR_TYPE_VALUES,
        },
        // … and on the other operation that WRITES a destination configuration, whose
        // member is an update rather than a configuration.
        Row {
            case: "a processor type outside the enum on an update",
            operation: "UpdateDestination",
            body: json!({
                "DeliveryStreamName": "off-enum-processor-type-update",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": {
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{ "Type": OFF_ENUM }],
                    },
                },
            }),
            path: "extendedS3DestinationUpdate.processingConfiguration.processors.1.member.type",
            value_set: Some(PROCESSOR_TYPE_SET),
            declared: PROCESSOR_TYPE_VALUES,
        },
        // A SECOND enumerated type, one level deeper still — a parameter name inside a
        // processor inside the processor list.
        Row {
            case: "a processor parameter name outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-parameter-name",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{
                            "Type": "Lambda",
                            "Parameters": [{
                                "ParameterName": OFF_ENUM,
                                "ParameterValue": "x",
                            }],
                        }],
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.processingConfiguration.processors.1.member\
                   .parameters.1.member.parameterName",
            value_set: None,
            declared: PROCESSOR_PARAMETER_NAME_VALUES,
        },
        // A THIRD type, on a plain destination member rather than inside a list.
        Row {
            case: "a compression format outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-compression-format",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "CompressionFormat": OFF_ENUM,
                },
            }),
            path: "extendedS3DestinationConfiguration.compressionFormat",
            value_set: None,
            declared: COMPRESSION_FORMAT_VALUES,
        },
        // A FOURTH type, of two values, under a destination of its own.
        Row {
            case: "an HEC endpoint type outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-hec-endpoint-type",
                "SplunkDestinationConfiguration": {
                    "HECEndpoint": "https://splunk.example.com:8088",
                    "HECEndpointType": OFF_ENUM,
                    "HECToken": "hec-token",
                    "S3Configuration": s3_configuration(),
                },
            }),
            path: "splunkDestinationConfiguration.hECEndpointType",
            value_set: None,
            declared: H_E_C_ENDPOINT_TYPE_VALUES,
        },
    ]
}

/// The `enum value set: [ … ]` clause's contents, or `None` when the message states none.
fn stated_set(message: &str) -> Option<&str> {
    let (_, tail) = message.split_once("Member must satisfy enum value set: [")?;
    tail.split_once(']').map(|(set, _)| set)
}

#[tokio::test]
async fn a_value_outside_an_enumerated_type_is_refused_with_that_type_whole_set() {
    let emulator = Emulator::new();

    for row in rows() {
        let (status, refused) = emulator.call(row.operation, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {refused:?}",
            row.case
        );
        let code = error_code(&refused);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {:?}: expected {VALIDATION_EXCEPTION}, got {code:?}",
            row.case
        );

        // The envelope, the subject, the member path and the requirement's wording — the
        // clause with the set left out, so this assertion pins the position and the
        // wording and the ones below pin the set.
        let message = error_message(&refused);
        let prefix = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: \
             Member must satisfy enum value set: [",
            row.path
        );
        assert!(
            message.starts_with(&prefix) && message.ends_with(']'),
            "case {:?}: expected {prefix:?} … ']', got {message:?}",
            row.case
        );
        let Some(stated) = stated_set(&message) else {
            panic!(
                "case {:?}: the message states no enum value set, got {message:?}",
                row.case
            );
        };
        // The stated set is the type the MODEL declares, re-ordered: every value once,
        // and no value the model does not declare.
        let mut listed: Vec<&str> = stated.split(", ").collect();
        let count = listed.len();
        listed.sort_unstable();
        listed.dedup();
        let mut declared: Vec<&str> = row.declared.to_vec();
        declared.sort_unstable();
        assert_eq!(
            listed.len(),
            count,
            "case {:?}: a value is stated twice, got {stated:?}",
            row.case
        );
        assert_eq!(
            listed, declared,
            "case {:?}: expected the modelled membership {declared:?}, got {stated:?}",
            row.case
        );

        // And where the refusal's own order is known, it is exactly that order.
        if let Some(value_set) = row.value_set {
            assert_eq!(
                stated, value_set,
                "case {:?}: expected the set stated as [{value_set}], got [{stated}]",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn one_enumerated_type_is_stated_the_same_way_wherever_it_is_carried() {
    let emulator = Emulator::new();

    // Group the rows by the type that failed and assert each group states ONE set: the
    // order is a property of the type, not of the member, operation, destination or depth
    // that carried the value.
    let mut seen: Vec<(&'static [&'static str], &'static str, String)> = Vec::new();
    for row in rows() {
        let (_, refused) = emulator.call(row.operation, row.body.clone()).await;
        let message = error_message(&refused);
        let stated = stated_set(&message)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the message states no enum value set, got {message:?}",
                    row.case
                )
            })
            .to_string();
        if let Some((_, first_case, first)) =
            seen.iter().find(|(declared, _, _)| *declared == row.declared)
        {
            assert_eq!(
                &stated, first,
                "case {:?}: the same type was stated as [{first}] by case {first_case:?}, \
                 got [{stated}]",
                row.case
            );
        } else {
            seen.push((row.declared, row.case, stated));
        }
    }

    // Neither the declaration order nor a sort of the values: a listing that happened to
    // be one of those would make the rows above pass for the wrong reason.
    let (_, _, processor_types) = seen
        .iter()
        .find(|(declared, _, _)| *declared == PROCESSOR_TYPE_VALUES)
        .expect("a ProcessorType row must have been refused");
    let declared = PROCESSOR_TYPE_VALUES.to_vec();
    let mut sorted = declared.clone();
    sorted.sort_unstable();
    assert_ne!(
        processor_types, &sorted.join(", "),
        "the stated set must not be a sort of the values, got [{processor_types}]"
    );
    assert_ne!(
        processor_types,
        &[
            "RecordDeAggregation",
            "Decompression",
            "CloudWatchLogProcessing",
            "Lambda",
            "MetadataExtraction",
            "AppendDelimiterToRecord",
        ]
        .join(", "),
        "the stated set must not be the model's declaration order, got [{processor_types}]"
    );
}

#[tokio::test]
async fn a_structural_refusal_and_a_semantic_one_do_not_share_a_code_or_leave_a_stream() {
    let emulator = Emulator::new();

    // A Lambda processor with no `LambdaArn` is Firehose's own verdict on a processor it
    // could never run: a modelled argument fault, not a structural one.
    let (status, refused) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "semantic-refusal",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{ "Type": "Lambda" }],
                    },
                },
            }),
        )
        .await;
    assert_eq!(status, 400, "the incomplete processor got {status} {refused:?}");
    let semantic_code = error_code(&refused);
    assert_eq!(
        semantic_code, INVALID_ARGUMENT,
        "expected {INVALID_ARGUMENT}, got {semantic_code:?} {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        "LambdaArn is required when Lambda processor is used.",
        "the incomplete processor got {refused:?}"
    );
    assert_ne!(
        semantic_code, VALIDATION_EXCEPTION,
        "a semantic verdict must not share the structural code, got {semantic_code:?}"
    );

    // The structural refusal of the same create, so both kinds are in hand before the
    // "nothing was persisted" check below.
    let (status, structural) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "structural-refusal",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{ "Type": OFF_ENUM }],
                    },
                },
            }),
        )
        .await;
    assert_eq!(status, 400, "the off-enum processor type got {status} {structural:?}");
    assert_eq!(
        error_code(&structural),
        VALIDATION_EXCEPTION,
        "expected {VALIDATION_EXCEPTION}, got {structural:?}"
    );

    // Neither refusal leaves a stream behind: the create is judged before any state is
    // written.
    for name in ["semantic-refusal", "structural-refusal"] {
        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused create must leave nothing behind, got {described:?}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {described:?}"
        );
    }

    // And the configuration that names no processor at all is admitted, reaches ACTIVE and
    // is echoed as submitted.
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "no-processors",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": { "Enabled": false, "Processors": [] },
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "the processor-less create got {status} {created:?}");
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "no-processors" }),
        )
        .await;
    assert_eq!(status, 200, "describe got {status} {described:?}");
    let description = &described["DeliveryStreamDescription"];
    assert_eq!(
        description["DeliveryStreamStatus"],
        json!("ACTIVE"),
        "the created stream must reach ACTIVE, got {description:?}"
    );
    let processing =
        &description["Destinations"][0]["ExtendedS3DestinationDescription"]["ProcessingConfiguration"];
    assert_eq!(
        processing["Enabled"],
        json!(false),
        "the echo must report data processing off, got {processing:?}"
    );
    assert_eq!(
        processing["Processors"],
        json!([]),
        "the echo must report the empty processor list, got {processing:?}"
    );
}
