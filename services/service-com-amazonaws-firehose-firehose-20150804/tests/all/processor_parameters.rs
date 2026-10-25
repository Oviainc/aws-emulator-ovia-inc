// SPDX-License-Identifier: CC0-1.0

//! What a destination's data-processing configuration reports back, over the real
//! `awsJson1_1` wire.
//!
//! The rule: a stored destination holds its processors' EFFECTIVE parameters, so a
//! description reports, for each processor, the parameters the caller supplied PLUS the
//! ones Firehose supplies itself for that processor type — a processor Firehose runs by
//! invoking another service (`Lambda`) also runs with a retry count and under a role, so
//! the retry count is filled from its documented default ("Currently the following default
//! values are supported: 3 for NumberOfRetries" — `ProcessorParameter$ParameterName`) and
//! the role from the destination's own delivery role whenever the caller named neither —
//! each parameter name once, in the order `ProcessorParameterName` declares them.
//!
//! The table below spans the destination types that carry a `ProcessingConfiguration`
//! (including the two whose delivery role is the intermediate Amazon S3 location's rather
//! than a member of their own), the processor types the model declares, the parameters a
//! caller may supply and the order it submits them in, and both write paths
//! (`CreateDeliveryStream` and `UpdateDestination`).

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::PROCESSOR_PARAMETER_NAME_VALUES;
use service_com_amazonaws_firehose_firehose_20150804::state::PROCESSOR_PARAMETER_NAME_ORDER;

use super::delivery_stream_lifecycle::{s3_configuration, Emulator, BUCKET_ARN, REGION, ROLE_ARN};

/// A second IAM role ARN, so a case can tell a destination's OWN role apart from the one
/// its intermediate Amazon S3 location carries ([`ROLE_ARN`]).
const DESTINATION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-destination";

/// The Lambda function a `Lambda` processor transforms records with.
fn function_arn() -> String {
    format!("arn:aws:lambda:{REGION}:000000000000:function:transform")
}

/// One `ProcessorParameter`.
fn parameter(name: &str, value: &str) -> Value {
    json!({ "ParameterName": name, "ParameterValue": value })
}

/// One `Processor` with a parameter list.
fn processor(kind: &str, parameters: Vec<Value>) -> Value {
    json!({ "Type": kind, "Parameters": parameters })
}

/// A `ProcessingConfiguration` with data processing on.
fn processing(processors: Vec<Value>) -> Value {
    json!({ "Enabled": true, "Processors": processors })
}

/// The expected parameters of one described processor: `None` for a processor that must
/// carry no `Parameters` member at all, otherwise the `(name, value)` pairs in the order
/// they must be reported in.
type ExpectedParameters = Option<Vec<(&'static str, String)>>;

/// One row: a destination created (or updated) with a processing configuration, and what
/// each of its processors must report.
struct Case {
    stream: &'static str,
    config_member: &'static str,
    config: Value,
    description_member: &'static str,
    /// One entry per processor, in the order the configuration lists them.
    expected: Vec<ExpectedParameters>,
}

/// The rows. The destination members are the ones
/// `smithy_query(query="operation", operation="CreateDeliveryStream")` lists, the processor
/// types are values of `ProcessorType` and the parameter names values of
/// `ProcessorParameterName` (`smithy_query(query="shape",
/// shape="ProcessorType,ProcessorParameterName")`).
fn cases() -> Vec<Case> {
    let lambda = function_arn();
    let redshift_url =
        "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev";
    vec![
        // The shape the report cites: a Lambda transform whose caller named the function
        // and the buffering parameters only, submitted in an order of its own.
        Case {
            stream: "lambda-buffered",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![
                        parameter("BufferIntervalInSeconds", "70"),
                        parameter("BufferSizeInMBs", "2"),
                        parameter("LambdaArn", &lambda),
                    ],
                )]),
            }),
            description_member: "ExtendedS3DestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
                ("BufferSizeInMBs", "2".to_string()),
                ("BufferIntervalInSeconds", "70".to_string()),
            ])],
        },
        // The function alone: both service-filled parameters are materialised and nothing
        // else is.
        Case {
            stream: "lambda-bare",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![parameter("LambdaArn", &lambda)],
                )]),
            }),
            description_member: "ExtendedS3DestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
            ])],
        },
        // A caller that named both itself keeps its own values: what Firehose fills in is
        // what the request left out.
        Case {
            stream: "lambda-explicit",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![
                        parameter("RoleArn", ROLE_ARN),
                        parameter("NumberOfRetries", "7"),
                        parameter("LambdaArn", &lambda),
                    ],
                )]),
            }),
            description_member: "ExtendedS3DestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "7".to_string()),
                ("RoleArn", ROLE_ARN.to_string()),
            ])],
        },
        // A name given twice names ONE parameter, so it is reported once.
        Case {
            stream: "lambda-repeated",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![
                        parameter("LambdaArn", &lambda),
                        parameter("LambdaArn", "arn:aws:lambda:us-west-2:000000000000:function:other"),
                    ],
                )]),
            }),
            description_member: "ExtendedS3DestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
            ])],
        },
        // A pipeline: a processor that takes no parameters at all keeps carrying none
        // ("You don't have to put a processor parameter when you select
        // `AppendDelimiterToRecord`" — `Processor`), one in Firehose itself is reported in
        // the canonical order and gets nothing filled in, and only the Lambda transform
        // does. Processor ORDER is the caller's and is untouched.
        Case {
            stream: "lambda-pipeline",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "ProcessingConfiguration": processing(vec![
                    json!({ "Type": "AppendDelimiterToRecord" }),
                    processor(
                        "MetadataExtraction",
                        vec![
                            parameter("JsonParsingEngine", "JQ-1.6"),
                            parameter("MetadataExtractionQuery", "{dept:.dept}"),
                        ],
                    ),
                    processor("Lambda", vec![parameter("LambdaArn", &lambda)]),
                ]),
            }),
            description_member: "ExtendedS3DestinationDescription",
            expected: vec![
                None,
                Some(vec![
                    ("MetadataExtractionQuery", "{dept:.dept}".to_string()),
                    ("JsonParsingEngine", "JQ-1.6".to_string()),
                ]),
                Some(vec![
                    ("LambdaArn", lambda.clone()),
                    ("NumberOfRetries", "3".to_string()),
                    ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
                ]),
            ],
        },
        // A processor Firehose runs itself gets no role and no retry count, whatever
        // destination carries it.
        Case {
            stream: "redshift-de-aggregation",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "ClusterJDBCURL": redshift_url,
                "CopyCommand": { "DataTableName": "events" },
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(vec![processor(
                    "RecordDeAggregation",
                    vec![parameter("SubRecordType", "JSON")],
                )]),
            }),
            description_member: "RedshiftDestinationDescription",
            expected: vec![Some(vec![("SubRecordType", "JSON".to_string())])],
        },
        // The destination's own role is what a Lambda transform runs under, and it is read
        // per destination type: Snowflake carries its own `RoleARN` …
        Case {
            stream: "snowflake-lambda",
            config_member: "SnowflakeDestinationConfiguration",
            config: json!({
                "AccountUrl": "sim-example-account.snowflakecomputing.com",
                "Database": "analytics",
                "Schema": "public",
                "Table": "events",
                "RoleARN": DESTINATION_ROLE_ARN,
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![parameter("LambdaArn", &lambda)],
                )]),
            }),
            description_member: "SnowflakeDestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
            ])],
        },
        // … Splunk's shape declares none, so the role of the intermediate Amazon S3
        // location it always carries is the one it delivers under …
        Case {
            stream: "splunk-lambda",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![parameter("LambdaArn", &lambda)],
                )]),
            }),
            description_member: "SplunkDestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", ROLE_ARN.to_string()),
            ])],
        },
        // … and an HTTP endpoint leaves its own optional, so a create that omits it falls
        // back to the same intermediate location's role …
        Case {
            stream: "http-endpoint-lambda-no-role",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": { "Url": "https://example.com/firehose" },
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![parameter("LambdaArn", &lambda)],
                )]),
            }),
            description_member: "HttpEndpointDestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", ROLE_ARN.to_string()),
            ])],
        },
        // … while one that names it delivers under that.
        Case {
            stream: "http-endpoint-lambda-with-role",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": { "Url": "https://example.com/firehose" },
                "RoleARN": DESTINATION_ROLE_ARN,
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![parameter("LambdaArn", &lambda)],
                )]),
            }),
            description_member: "HttpEndpointDestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
            ])],
        },
        // A configuration that names no processor at all still reports the list it
        // holds, which is the empty one.
        Case {
            stream: "processing-without-processors",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "ProcessingConfiguration": { "Enabled": false },
            }),
            description_member: "ExtendedS3DestinationDescription",
            expected: vec![],
        },
        // An Amazon OpenSearch Service destination, to show the rule is the destination's
        // and not one member's.
        Case {
            stream: "opensearch-lambda",
            config_member: "AmazonopensearchserviceDestinationConfiguration",
            config: json!({
                "RoleARN": DESTINATION_ROLE_ARN,
                "DomainARN": format!("arn:aws:es:{REGION}:000000000000:domain/logs-domain"),
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
                "ProcessingConfiguration": processing(vec![processor(
                    "Lambda",
                    vec![
                        parameter("BufferSizeInMBs", "1"),
                        parameter("LambdaArn", &lambda),
                    ],
                )]),
            }),
            description_member: "AmazonopensearchserviceDestinationDescription",
            expected: vec![Some(vec![
                ("LambdaArn", lambda.clone()),
                ("NumberOfRetries", "3".to_string()),
                ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
                ("BufferSizeInMBs", "1".to_string()),
            ])],
        },
    ]
}

/// The `(ParameterName, ParameterValue)` pairs a described processor reports, in the order
/// it reports them, or `None` when it carries no `Parameters` member.
fn reported(processor: &Value) -> Option<Vec<(String, String)>> {
    let parameters = processor.get("Parameters")?.as_array()?;
    Some(
        parameters
            .iter()
            .map(|parameter| {
                (
                    parameter["ParameterName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    parameter["ParameterValue"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect(),
    )
}

/// `expected` as [`reported`] answers it, so the two compare as one value.
fn as_reported(expected: &ExpectedParameters) -> Option<Vec<(String, String)>> {
    expected.as_ref().map(|pairs| {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    })
}

/// Assert every processor of `description` reports what `expected` says, for the case
/// labelled `case`.
fn assert_processors(case: &str, description: &Value, expected: &[ExpectedParameters]) {
    let processing = &description["ProcessingConfiguration"];
    // A stored configuration always reports its processor list, empty or not.
    assert!(
        processing["Processors"].is_array(),
        "case {case:?}: expected a processor list, got {processing:?}"
    );
    let processors = processing["Processors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        processors.len(),
        expected.len(),
        "case {case:?}: expected {} processors, got {description:?}",
        expected.len()
    );
    for (index, (processor, expected)) in processors.iter().zip(expected).enumerate() {
        let actual = reported(processor);
        let wanted = as_reported(expected);
        assert_eq!(
            actual, wanted,
            "case {case:?} processor {index}: expected {wanted:?}, got {actual:?}"
        );
        // Each name once, whatever the case: the fold is part of the rule.
        if let Some(actual) = actual.as_ref() {
            let mut names: Vec<&str> = actual.iter().map(|(name, _)| name.as_str()).collect();
            names.sort_unstable();
            let count = names.len();
            names.dedup();
            assert_eq!(
                names.len(),
                count,
                "case {case:?} processor {index}: a parameter name is reported twice, \
                 got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_described_processor_reports_the_parameters_the_service_fills_in_too() {
    let emulator = Emulator::new();

    for case in cases() {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.config_member] = case.config.clone();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: create with {} got {created:?}",
            case.stream, case.config_member
        );

        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": case.stream }),
            )
            .await;
        assert_eq!(status, 200, "case {:?}: describe got {described:?}", case.stream);
        let description = &described["DeliveryStreamDescription"]["Destinations"][0]
            [case.description_member];
        assert_processors(case.stream, description, &case.expected);
    }
}

#[tokio::test]
async fn an_updated_destination_reports_the_parameters_of_the_configuration_it_now_holds() {
    let emulator = Emulator::new();
    let lambda = function_arn();

    // Created with a processor Firehose runs itself, so nothing is filled in yet.
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "updated-processors",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": processing(vec![processor(
                        "MetadataExtraction",
                        vec![
                            parameter("JsonParsingEngine", "JQ-1.6"),
                            parameter("MetadataExtractionQuery", "{dept:.dept}"),
                        ],
                    )]),
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "create: got {created:?}");
    tokio::time::sleep(super::delivery_stream_lifecycle::PAST_THE_TRANSITION).await;

    // An update replaces the whole processing block, and the configuration it leaves
    // behind is normalised exactly as a created one is — against the role the update
    // moves the destination onto.
    let (status, updated) = emulator
        .call(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": "updated-processors",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": {
                    "RoleARN": DESTINATION_ROLE_ARN,
                    "ProcessingConfiguration": processing(vec![processor(
                        "Lambda",
                        vec![
                            parameter("BufferIntervalInSeconds", "90"),
                            parameter("LambdaArn", &lambda),
                        ],
                    )]),
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "update: got {updated:?}");

    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "updated-processors" }),
        )
        .await;
    assert_eq!(status, 200, "describe after the update: got {described:?}");
    assert_processors(
        "after the update",
        &described["DeliveryStreamDescription"]["Destinations"][0]
            ["ExtendedS3DestinationDescription"],
        &[Some(vec![
            ("LambdaArn", lambda.clone()),
            ("NumberOfRetries", "3".to_string()),
            ("RoleArn", DESTINATION_ROLE_ARN.to_string()),
            ("BufferIntervalInSeconds", "90".to_string()),
        ])],
    );

    // An update supplies the processing configuration as a WHOLE block, so one that names
    // no processor leaves the destination processing nothing — and the empty list is what
    // the description reports.
    let (status, cleared) = emulator
        .call(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": "updated-processors",
                "CurrentDeliveryStreamVersionId": "2",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": {
                    "ProcessingConfiguration": { "Enabled": false },
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "update that clears the processors: got {cleared:?}");

    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "updated-processors" }),
        )
        .await;
    assert_eq!(status, 200, "describe after the clearing update: got {described:?}");
    assert_processors(
        "after the clearing update",
        &described["DeliveryStreamDescription"]["Destinations"][0]
            ["ExtendedS3DestinationDescription"],
        &[],
    );
}

#[test]
fn the_canonical_parameter_order_covers_every_modelled_parameter_name() {
    // The order is the `ProcessorParameterName` DECLARATION order, which the generated
    // value list (sorted) cannot carry: the two must still describe the same set, so a
    // name added to the model cannot go unordered.
    let mut ordered: Vec<&str> = PROCESSOR_PARAMETER_NAME_ORDER.to_vec();
    ordered.sort_unstable();
    let mut modelled: Vec<&str> = PROCESSOR_PARAMETER_NAME_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        ordered, modelled,
        "the canonical order must cover exactly the modelled parameter names, \
         got {ordered:?} against {modelled:?}"
    );
}
