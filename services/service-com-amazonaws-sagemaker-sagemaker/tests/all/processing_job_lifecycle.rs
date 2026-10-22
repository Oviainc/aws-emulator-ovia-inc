// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's processing-job lifecycle:
//! `CreateProcessingJob` -> `DescribeProcessingJob`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! optional member; the single clock-owed move out of `InProgress`; the refusal of a name
//! already in use and of an unknown name; and each request-shape rule the Smithy model
//! states in prose (`smithy_query(query="shape", shape="ProcessingInput")` and its
//! neighbours), which the schema cannot express and the handler therefore enforces.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator, one request builder
// and one set of body accessors.
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::PROCESSING_JOB_ENDS_AFTER` (400 ms).
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerProcessingRole";

/// `DescribeProcessingJob` for `name`, as the raw (status, body) pair, so a test can
/// assert on a refusal as well as on a stored job.
async fn describe(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call("DescribeProcessingJob", &json!({ "ProcessingJobName": name }))
        .await
}

/// A minimal create request: every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "ProcessingJobName": name,
        "RoleArn": ROLE_ARN,
        "AppSpecification": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/proc:1" },
        "ProcessingResources": {
            "ClusterConfig": {
                "InstanceCount": 1,
                // A member of `ProcessingInstanceType` (smithy_query shape
                // ProcessingClusterConfig -> ProcessingInstanceType).
                "InstanceType": "ml.m5.large",
                "VolumeSizeInGB": 30,
            },
        },
    })
}

/// `minimal_create`, with every member of `extra` merged over it.
fn create_with(name: &str, extra: &Value) -> Value {
    let mut body = minimal_create(name);
    for (key, value) in extra.as_object().expect("a JSON object of extra members") {
        body[key] = value.clone();
    }
    body
}

/// A SageMaker-managed S3 input: `LocalPath` under the directory the model documents.
fn s3_input(name: &str, uri: &str) -> Value {
    json!({
        "InputName": name,
        "S3Input": {
            "S3Uri": uri,
            "LocalPath": format!("/opt/ml/processing/input/{name}"),
            "S3DataType": "S3Prefix",
            "S3InputMode": "File",
        },
    })
}

/// A SageMaker-managed S3 output.
fn s3_output(name: &str, uri: &str) -> Value {
    json!({
        "OutputName": name,
        "S3Output": {
            "S3Uri": uri,
            "LocalPath": format!("/opt/ml/processing/output/{name}"),
            "S3UploadMode": "EndOfJob",
        },
    })
}

/// RULE: a created processing job's ARN is
/// `arn:aws:sagemaker:<signed region>:<account>:processing-job/<ProcessingJobName>`, and
/// the job the matching `Describe` replays carries that same ARN, whatever the name's
/// shape and whichever region the caller signed for.
#[tokio::test]
async fn minted_arn_names_the_signed_region_the_account_and_the_job_name() {
    // Rows differ along the axes the rule generalises over: the name's shape (one
    // character, digits, hyphens, and the 63-character modelled maximum) and the region.
    let names = [
        "p",
        "job7",
        "nightly-feature-engineering-job",
        // `ProcessingJobName` is `len 1..63` (smithy_query operation=CreateProcessingJob),
        // so this row sits on the upper bound.
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let expected = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:processing-job/{name}");

            let (status, created) = emulator
                .call_in(region, "CreateProcessingJob", &minimal_create(name))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: CreateProcessingJob, got {status} with {created}"
            );
            assert_eq!(
                str_field(&created, "ProcessingJobArn"),
                expected,
                "case {case}: ProcessingJobArn, got {created:?}"
            );

            let (status, described) = emulator
                .call_in(
                    region,
                    "DescribeProcessingJob",
                    &json!({ "ProcessingJobName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: DescribeProcessingJob, got {status} with {described}"
            );
            assert_eq!(
                str_field(&described, "ProcessingJobArn"),
                expected,
                "case {case}: the described ARN must be the minted one, got {described:?}"
            );
            assert_eq!(
                str_field(&described, "ProcessingJobName"),
                name,
                "case {case}: ProcessingJobName, got {described:?}"
            );
        }
    }
}

/// RULE: `DescribeProcessingJob` replays every member `CreateProcessingJob` stored,
/// present members and absent ones alike, and reports neither a start time nor an exit
/// message, because nothing ran the job's container image.
#[tokio::test]
async fn describe_replays_exactly_what_create_stored() {
    // Rows differ along input count and kind, output kind, environment size, and which
    // of the optional configuration blocks the request carried.
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", json!({})),
        (
            "one-s3-input-and-one-s3-output",
            json!({
                "ProcessingInputs": [s3_input("data", "s3://bucket/in/")],
                "ProcessingOutputConfig": {
                    "Outputs": [s3_output("result", "s3://bucket/out/")],
                },
            }),
        ),
        (
            "three-inputs-two-outputs-with-kms",
            json!({
                "ProcessingInputs": [
                    s3_input("train", "s3://bucket/train/"),
                    s3_input("validate", "s3://bucket/validate/"),
                    {
                        "InputName": "piped",
                        "S3Input": {
                            "S3Uri": "s3://bucket/piped/manifest.json",
                            "LocalPath": "/opt/ml/processing/input/piped",
                            "S3DataType": "ManifestFile",
                            "S3InputMode": "Pipe",
                            "S3CompressionType": "Gzip",
                            "S3DataDistributionType": "ShardedByS3Key",
                        },
                    },
                ],
                "ProcessingOutputConfig": {
                    "Outputs": [
                        s3_output("first", "s3://bucket/out/first/"),
                        s3_output("second", "s3://bucket/out/second/"),
                    ],
                    "KmsKeyId": "arn:aws:kms:us-east-1:000000000000:key/abc",
                },
            }),
        ),
        (
            "app-managed-input-and-feature-store-output",
            json!({
                "ProcessingInputs": [{
                    "InputName": "app-managed",
                    "AppManaged": true,
                    "S3Input": {
                        "S3Uri": "s3://bucket/app/",
                        "S3DataType": "S3Prefix",
                    },
                }],
                "ProcessingOutputConfig": {
                    "Outputs": [{
                        "OutputName": "records",
                        "AppManaged": true,
                        "FeatureStoreOutput": { "FeatureGroupName": "customer-features" },
                    }],
                },
            }),
        ),
        (
            "dataset-definition-input",
            json!({
                "ProcessingInputs": [{
                    "InputName": "athena",
                    "DatasetDefinition": {
                        "AthenaDatasetDefinition": {
                            "Catalog": "AwsDataCatalog",
                            "Database": "analytics",
                            "QueryString": "SELECT 1",
                            "OutputS3Uri": "s3://bucket/athena/",
                            "OutputFormat": "PARQUET",
                        },
                        "LocalPath": "/opt/ml/processing/input/athena",
                        "DataDistributionType": "FullyReplicated",
                        "InputMode": "File",
                    },
                }],
            }),
        ),
        (
            "environment-network-experiment-and-stopping-condition",
            json!({
                "Environment": { "MODE": "batch", "THREADS": "4", "REGION": "us-east-1" },
                "NetworkConfig": {
                    "EnableInterContainerTrafficEncryption": true,
                    "EnableNetworkIsolation": false,
                    "VpcConfig": {
                        "SecurityGroupIds": ["sg-1", "sg-2"],
                        "Subnets": ["subnet-1"],
                    },
                },
                "ExperimentConfig": {
                    "ExperimentName": "churn",
                    "TrialName": "trial-1",
                    "TrialComponentDisplayName": "preprocess",
                },
                "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
                "Tags": [{ "Key": "team", "Value": "ml" }],
                "AppSpecification": {
                    "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/proc:2",
                    "ContainerEntrypoint": ["python3", "/opt/ml/code/run.py"],
                    "ContainerArguments": ["--mode", "batch"],
                },
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let body = create_with(case, &extra);
        emulator.ok("CreateProcessingJob", &body).await;
        let described = emulator.ok("DescribeProcessingJob", &json!({ "ProcessingJobName": case })).await;

        // Every member the request carried comes back byte-identical. `ProcessingResources`
        // is checked by its own test: the service applies a documented default to it.
        for member in [
            "ProcessingInputs",
            "ProcessingOutputConfig",
            "Environment",
            "NetworkConfig",
            "ExperimentConfig",
            "StoppingCondition",
            "AppSpecification",
        ] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must replay what was stored, got {:?}",
                described.get(member)
            );
        }
        assert_eq!(
            described.get("RoleArn").and_then(Value::as_str),
            Some(ROLE_ARN),
            "case {case}: RoleArn, got {described:?}"
        );
        // Nothing ran the container image, so neither of these is invented.
        for absent in ["ProcessingStartTime", "ExitMessage"] {
            assert!(
                described.get(absent).is_none(),
                "case {case}: {absent} must be absent, got {:?}",
                described.get(absent)
            );
        }
        // A processing job created through the API is owned by no other resource.
        for absent in ["MonitoringScheduleArn", "AutoMLJobArn", "TrainingJobArn"] {
            assert!(
                described.get(absent).is_none(),
                "case {case}: {absent} must be absent, got {:?}",
                described.get(absent)
            );
        }
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: expected a CreationTime in {described}"));
        assert!(
            creation_time > 0.0,
            "case {case}: CreationTime must be the instant of the create, got {creation_time}"
        );
    }
}

/// RULE: a job is accepted in the non-terminal `InProgress` status and owes its single
/// move to a terminal status to the clock: a read before the move is due reports
/// `InProgress` and no end time, a read at or after it reports the terminal status with
/// the reason and the end time stamped at the instant the move was due, and further reads
/// change nothing.
#[tokio::test]
async fn the_status_moves_out_of_in_progress_only_when_the_clock_owes_it() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateProcessingJob", &minimal_create("clock-owed"))
        .await;

    // Two reads straight after the create: the transient status is observable, and
    // reading does not advance it.
    for read in ["first", "second"] {
        let described = emulator
            .ok("DescribeProcessingJob", &json!({ "ProcessingJobName": "clock-owed" }))
            .await;
        assert_eq!(
            str_field(&described, "ProcessingJobStatus"),
            "InProgress",
            "case read={read}: a job must start InProgress, got {described:?}"
        );
        for absent in ["ProcessingEndTime", "FailureReason"] {
            assert!(
                described.get(absent).is_none(),
                "case read={read}: {absent} must be absent while InProgress, got {:?}",
                described.get(absent)
            );
        }
    }

    tokio::time::sleep(PAST_JOB_END_DUE).await;

    let settled = emulator
        .ok("DescribeProcessingJob", &json!({ "ProcessingJobName": "clock-owed" }))
        .await;
    let status = str_field(&settled, "ProcessingJobStatus");
    // `ProcessingJobStatus`'s terminal members (smithy_query shape=ProcessingJobStatus:
    // Completed | Failed | InProgress | Stopped | Stopping).
    assert!(
        ["Completed", "Failed", "Stopped"].contains(&status.as_str()),
        "case settled: the job must reach a terminal status, got {status}"
    );
    assert_eq!(
        status, "Failed",
        "case settled: nothing ran the container image, so the run must end Failed, got \
         {settled:?}"
    );
    assert!(
        !str_field(&settled, "FailureReason").is_empty(),
        "case settled: a Failed job must say why, got {settled:?}"
    );
    let end_time = settled
        .get("ProcessingEndTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case settled: expected a ProcessingEndTime in {settled}"));
    let last_modified = settled
        .get("LastModifiedTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case settled: expected a LastModifiedTime in {settled}"));
    assert!(
        (end_time - last_modified).abs() < f64::EPSILON,
        "case settled: the end time and the last modification are both the instant the \
         move was due, got end={end_time} last_modified={last_modified}"
    );
    assert!(
        settled.get("ProcessingStartTime").is_none(),
        "case settled: no start time is invented for a container that never started, got \
         {:?}",
        settled.get("ProcessingStartTime")
    );

    // The terminal state is stable: a later read answers identically, including the
    // instants, so no read moves the resource on.
    let again = emulator
        .ok("DescribeProcessingJob", &json!({ "ProcessingJobName": "clock-owed" }))
        .await;
    assert_eq!(
        again, settled,
        "case re-read: a settled job must answer identically, got {again:?}"
    );
}

/// RULE: `ProcessingJobName` is unique per account and Region, so a create naming a job
/// that already exists is refused with `ResourceInUse` and leaves the stored job as it
/// was.
#[tokio::test]
async fn a_name_already_in_use_is_refused_and_changes_nothing() {
    // Rows differ along the shape of the name and along what the second request asked
    // for, so the refusal is about the NAME rather than about the body.
    let cases: Vec<(&str, Value)> = vec![
        ("same-body", json!({})),
        (
            "different-image",
            json!({ "AppSpecification": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/other:9" } }),
        ),
        (
            "different-role-and-inputs",
            json!({
                "RoleArn": "arn:aws:iam::000000000000:role/OtherRole",
                "ProcessingInputs": [s3_input("data", "s3://other/in/")],
            }),
        ),
    ];

    for (case, second) in cases {
        let emulator = Emulator::new();
        let first = emulator.ok("CreateProcessingJob", &minimal_create(case)).await;
        let before = emulator.ok("DescribeProcessingJob", &json!({ "ProcessingJobName": case })).await;

        let (status, body) = emulator
            .call("CreateProcessingJob", &create_with(case, &second))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceInUse",
            "case {case}: expected ResourceInUse, got {body:?}"
        );
        assert!(
            message(&body).contains(case),
            "case {case}: the refusal must name the job, got {:?}",
            message(&body)
        );

        let after = emulator.ok("DescribeProcessingJob", &json!({ "ProcessingJobName": case })).await;
        assert_eq!(
            after, before,
            "case {case}: the refused create must not have touched the stored job, got \
             {after:?}"
        );
        assert_eq!(
            str_field(&after, "ProcessingJobArn"),
            str_field(&first, "ProcessingJobArn"),
            "case {case}: the stored job keeps the first create's ARN, got {after:?}"
        );
    }
}

/// RULE: `DescribeProcessingJob` for a name no create stored is refused with the modelled
/// `ResourceNotFound`.
#[tokio::test]
async fn an_unknown_job_name_is_refused_with_resource_not_found() {
    let emulator = Emulator::new();
    emulator.ok("CreateProcessingJob", &minimal_create("stored")).await;

    // Rows differ along how the name misses: never created, a prefix of a stored name, a
    // suffix of one, and a case-shifted spelling of one.
    for name in ["missing", "store", "storedX", "STORED"] {
        let (status, body) = describe(&emulator, name).await;
        assert_eq!(
            status, 400,
            "case {name}: an unknown job must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case {name}: expected ResourceNotFound, got {body:?}"
        );
        assert!(
            message(&body).contains(name),
            "case {name}: the refusal must name what was looked up, got {:?}",
            message(&body)
        );
    }
}

/// RULE: a create whose members break one of the rules the model states in prose is
/// refused with a 400 `ValidationException` naming the member, and stores nothing.
#[tokio::test]
async fn request_shapes_the_model_rules_out_are_refused_and_persist_nothing() {
    // One row per documented rule, each quoted on the member it comes from.
    let cases: Vec<(&str, Value, &str)> = vec![
        // ProcessingInput: "must specify exactly one of either S3Input or
        // DatasetDefinition types."
        (
            "input-with-neither",
            json!({ "ProcessingInputs": [{ "InputName": "data" }] }),
            "S3Input",
        ),
        (
            "input-with-both",
            json!({
                "ProcessingInputs": [{
                    "InputName": "data",
                    "S3Input": {
                        "S3Uri": "s3://bucket/in/",
                        "LocalPath": "/opt/ml/processing/input/data",
                        "S3DataType": "S3Prefix",
                    },
                    "DatasetDefinition": { "LocalPath": "/opt/ml/processing/input/data" },
                }],
            }),
            "DatasetDefinition",
        ),
        // ProcessingS3Input$LocalPath: "a required parameter when AppManaged is False
        // (default)".
        (
            "managed-input-without-local-path",
            json!({
                "ProcessingInputs": [{
                    "InputName": "data",
                    "S3Input": { "S3Uri": "s3://bucket/in/", "S3DataType": "S3Prefix" },
                }],
            }),
            "LocalPath",
        ),
        // ProcessingS3Input$LocalPath: "must begin with /opt/ml/processing/".
        (
            "managed-input-with-foreign-local-path",
            json!({
                "ProcessingInputs": [{
                    "InputName": "data",
                    "S3Input": {
                        "S3Uri": "s3://bucket/in/",
                        "LocalPath": "/tmp/data",
                        "S3DataType": "S3Prefix",
                    },
                }],
            }),
            "/opt/ml/processing/",
        ),
        // ProcessingS3Input$S3CompressionType: "Gzip can only be used when Pipe mode is
        // specified as the S3InputMode."
        (
            "gzip-without-pipe-mode",
            json!({
                "ProcessingInputs": [{
                    "InputName": "data",
                    "S3Input": {
                        "S3Uri": "s3://bucket/in/",
                        "LocalPath": "/opt/ml/processing/input/data",
                        "S3DataType": "S3Prefix",
                        "S3InputMode": "File",
                        "S3CompressionType": "Gzip",
                    },
                }],
            }),
            "S3CompressionType",
        ),
        // ProcessingOutput: "must specify exactly one of either S3Output or
        // FeatureStoreOutput types."
        (
            "output-with-neither",
            json!({ "ProcessingOutputConfig": { "Outputs": [{ "OutputName": "result" }] } }),
            "S3Output",
        ),
        (
            "output-with-both",
            json!({
                "ProcessingOutputConfig": {
                    "Outputs": [{
                        "OutputName": "result",
                        "AppManaged": true,
                        "S3Output": {
                            "S3Uri": "s3://bucket/out/",
                            "S3UploadMode": "EndOfJob",
                        },
                        "FeatureStoreOutput": { "FeatureGroupName": "features" },
                    }],
                },
            }),
            "FeatureStoreOutput",
        ),
        // ProcessingOutput$FeatureStoreOutput: "only supported when AppManaged is
        // specified."
        (
            "feature-store-output-without-app-managed",
            json!({
                "ProcessingOutputConfig": {
                    "Outputs": [{
                        "OutputName": "records",
                        "FeatureStoreOutput": { "FeatureGroupName": "features" },
                    }],
                },
            }),
            "AppManaged",
        ),
        // ProcessingClusterConfig$SelectedInstanceType / $SelectedInstanceCount: "This
        // field is read-only and isn't accepted in CreateProcessingJob requests."
        (
            "selected-instance-type-in-the-request",
            json!({
                "ProcessingResources": {
                    "ClusterConfig": {
                        "InstanceCount": 1,
                        "InstanceType": "ml.m5.large",
                        "VolumeSizeInGB": 30,
                        "SelectedInstanceType": "ml.m5.large",
                    },
                },
            }),
            "SelectedInstanceType",
        ),
        (
            "selected-instance-count-in-the-request",
            json!({
                "ProcessingResources": {
                    "ClusterConfig": {
                        "InstanceCount": 1,
                        "InstanceType": "ml.m5.large",
                        "VolumeSizeInGB": 30,
                        "SelectedInstanceCount": 2,
                    },
                },
            }),
            "SelectedInstanceCount",
        ),
        // ProcessingClusterConfig$InstanceType: "InstancePreferences is mutually
        // exclusive with InstanceType."
        (
            "preferences-beside-instance-type",
            json!({
                "ProcessingResources": {
                    "ClusterConfig": {
                        "InstanceCount": 1,
                        "InstanceType": "ml.m5.large",
                        "VolumeSizeInGB": 30,
                        "InstancePreferences": [{ "InstanceType": "ml.m5.xlarge" }],
                    },
                },
            }),
            "mutually exclusive",
        ),
        // ProcessingInstancePreference$InstanceCount: rejected when set "on only some
        // preferences".
        (
            "instance-count-on-some-preferences",
            json!({
                "ProcessingResources": {
                    "ClusterConfig": {
                        "VolumeSizeInGB": 30,
                        "InstancePreferences": [
                            { "InstanceType": "ml.m5.large", "InstanceCount": 2 },
                            { "InstanceType": "ml.m5.xlarge" },
                        ],
                    },
                },
            }),
            "InstanceCount",
        ),
        // ... "both per preference and in ProcessingClusterConfig".
        (
            "instance-count-in-both-places",
            json!({
                "ProcessingResources": {
                    "ClusterConfig": {
                        "InstanceCount": 3,
                        "VolumeSizeInGB": 30,
                        "InstancePreferences": [
                            { "InstanceType": "ml.m5.large", "InstanceCount": 2 },
                        ],
                    },
                },
            }),
            "InstanceCount",
        ),
        // ... or "omit it in both places".
        (
            "instance-count-nowhere",
            json!({
                "ProcessingResources": {
                    "ClusterConfig": {
                        "VolumeSizeInGB": 30,
                        "InstancePreferences": [{ "InstanceType": "ml.m5.large" }],
                    },
                },
            }),
            "InstanceCount",
        ),
    ];

    for (case, extra, expected_in_message) in cases {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call("CreateProcessingJob", &create_with(case, &extra))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the request to be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body:?}"
        );
        assert!(
            message(&body).contains(expected_in_message),
            "case {case}: the refusal must name {expected_in_message}, got {:?}",
            message(&body)
        );

        let (status, described) = describe(&emulator, case).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused create must store nothing, got {status} with {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFound",
            "case {case}: a refused create must store nothing, got {described:?}"
        );
    }
}

/// RULE: the cluster the job runs on is the requested one with the documented default
/// applied (`InstanceCount` "default value is 1") and, when the request ranked instance
/// types instead of naming one, with the first-ranked type reported as the selected one
/// together with the count that type launches.
#[tokio::test]
async fn the_described_cluster_carries_the_documented_default_and_selection() {
    // Rows differ along how the request expressed its compute: a named type with and
    // without a count, and preference lists of one and three entries with the count
    // placed per preference or once for the job.
    struct Case {
        name: &'static str,
        cluster: Value,
        instance_count: Option<i64>,
        selected_type: Option<&'static str>,
        selected_count: Option<i64>,
    }
    let cases = vec![
        Case {
            name: "named-type-with-count",
            cluster: json!({
                "InstanceCount": 4,
                "InstanceType": "ml.m5.large",
                "VolumeSizeInGB": 30,
            }),
            instance_count: Some(4),
            selected_type: None,
            selected_count: None,
        },
        Case {
            name: "named-type-without-count",
            cluster: json!({ "InstanceType": "ml.m5.xlarge", "VolumeSizeInGB": 50 }),
            instance_count: Some(1),
            selected_type: None,
            selected_count: None,
        },
        Case {
            name: "one-preference-with-its-own-count",
            cluster: json!({
                "VolumeSizeInGB": 30,
                "InstancePreferences": [{ "InstanceType": "ml.m5.large", "InstanceCount": 2 }],
            }),
            instance_count: None,
            selected_type: Some("ml.m5.large"),
            selected_count: Some(2),
        },
        Case {
            name: "three-preferences-with-per-preference-counts",
            cluster: json!({
                "VolumeSizeInGB": 100,
                "InstancePreferences": [
                    { "InstanceType": "ml.c5.xlarge", "InstanceCount": 5 },
                    { "InstanceType": "ml.m5.large", "InstanceCount": 3 },
                    { "InstanceType": "ml.m5.xlarge", "InstanceCount": 1 },
                ],
            }),
            instance_count: None,
            selected_type: Some("ml.c5.xlarge"),
            selected_count: Some(5),
        },
        Case {
            name: "three-preferences-with-one-count-for-the-job",
            cluster: json!({
                "InstanceCount": 7,
                "VolumeSizeInGB": 100,
                "InstancePreferences": [
                    { "InstanceType": "ml.m5.xlarge" },
                    { "InstanceType": "ml.c5.xlarge" },
                    { "InstanceType": "ml.m5.large" },
                ],
            }),
            instance_count: Some(7),
            selected_type: Some("ml.m5.xlarge"),
            selected_count: Some(7),
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        let body = create_with(
            case.name,
            &json!({ "ProcessingResources": { "ClusterConfig": case.cluster } }),
        );
        emulator.ok("CreateProcessingJob", &body).await;
        let described = emulator
            .ok("DescribeProcessingJob", &json!({ "ProcessingJobName": case.name }))
            .await;
        let cluster = described
            .get("ProcessingResources")
            .and_then(|resources| resources.get("ClusterConfig"))
            .unwrap_or_else(|| {
                panic!("case {}: expected a ClusterConfig in {described}", case.name)
            });

        assert_eq!(
            cluster.get("InstanceCount").and_then(Value::as_i64),
            case.instance_count,
            "case {}: InstanceCount, got {cluster:?}",
            case.name
        );
        assert_eq!(
            cluster.get("SelectedInstanceType").and_then(Value::as_str),
            case.selected_type,
            "case {}: SelectedInstanceType, got {cluster:?}",
            case.name
        );
        assert_eq!(
            cluster.get("SelectedInstanceCount").and_then(Value::as_i64),
            case.selected_count,
            "case {}: SelectedInstanceCount, got {cluster:?}",
            case.name
        );
        // The volume size the request named is never rewritten.
        assert_eq!(
            cluster.get("VolumeSizeInGB"),
            body["ProcessingResources"]["ClusterConfig"].get("VolumeSizeInGB"),
            "case {}: VolumeSizeInGB, got {cluster:?}",
            case.name
        );
    }
}
