// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `StopProcessingJob`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, and
//! every stop is proven by reading the job back through `DescribeProcessingJob`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a stop of
//! a running job answers an empty body and moves the stored job into the transient
//! `Stopping` status, whatever its name, Region or optional members; the clock — not the
//! read — owes it `Stopped`, with no failure reason beside a status the caller asked for;
//! a stop already in flight is not restarted; a job whose run is over has nothing to
//! stop; a name nothing created resolves to no stop target and is refused with HTTP 400
//! `ValidationException`; and a stop moves the one job it names and no other.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator, one request builder
// and one set of body accessors.
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};

/// Long enough to pass `state::PROCESSING_JOB_STOPS_AFTER` (300 ms).
const PAST_STOP_DUE: Duration = Duration::from_millis(550);

/// Long enough to pass `state::PROCESSING_JOB_ENDS_AFTER` (400 ms), after which a job
/// left to run has reached its terminal status.
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateProcessingJob).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerProcessingRole";

/// One row of the stop table: a case name, the job's name and Region, and whatever the
/// request carries beyond `CreateProcessingJob`'s required members.
struct Row {
    case: &'static str,
    name: &'static str,
    region: &'static str,
    members: Value,
}

/// The stop table. Rows differ along the axes the rule generalises over: the shape of the
/// job's name (`ProcessingJobName` is `len 1..63` with pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — smithy_query operation=StopProcessingJob — so
/// the rows sit on both published bounds and in between), the Region the caller signs
/// for, and which optional members the create carried (`ProcessingInputs`,
/// `ProcessingOutputConfig`, `Environment`, `StoppingCondition`, `Tags`,
/// `NetworkConfig`): none of them has any bearing on whether a running job can be
/// stopped.
fn stop_rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, shortest name",
            // `ProcessingJobName`'s minimum length of 1.
            name: "p",
            region: "us-east-1",
            members: json!({}),
        },
        Row {
            case: "hyphenated name, another region, inputs and outputs",
            name: "nightly-feature-engineering-job",
            region: "eu-west-2",
            members: json!({
                "ProcessingInputs": [{
                    "InputName": "raw",
                    "S3Input": {
                        "S3Uri": "s3://bucket/raw/",
                        "LocalPath": "/opt/ml/processing/input/raw",
                        "S3DataType": "S3Prefix",
                        "S3InputMode": "File",
                        "S3DataDistributionType": "ShardedByS3Key",
                    },
                }],
                "ProcessingOutputConfig": {
                    "Outputs": [{
                        "OutputName": "features",
                        "S3Output": {
                            "S3Uri": "s3://bucket/features/",
                            "LocalPath": "/opt/ml/processing/output/features",
                            "S3UploadMode": "EndOfJob",
                        },
                    }],
                },
                "Environment": { "MODE": "batch" },
            }),
        },
        Row {
            case: "longest name, every optional block",
            // `ProcessingJobName`'s maximum length of 63.
            name: "a234567890123456789012345678901234567890123456789012345678901b",
            region: "ap-southeast-1",
            members: json!({
                "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
                "NetworkConfig": { "EnableNetworkIsolation": true },
                "ExperimentConfig": { "ExperimentName": "stop-experiment" },
                "Tags": [{ "Key": "team", "Value": "data" }],
            }),
        },
    ]
}

/// A `CreateProcessingJob` body: the job name, the required members and whatever the row
/// adds on top of them.
fn create_body(name: &str, members: &Value) -> Value {
    let mut body = json!({
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
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (member, value) in members.as_object().expect("the row is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// Seed one running job and answer the ARN the create minted.
async fn create_job(emulator: &Emulator, row: &Row) -> String {
    let (status, created) = emulator
        .call_in(
            row.region,
            "CreateProcessingJob",
            &create_body(row.name, &row.members),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateProcessingJob to answer 200, got {status} with {created:?}",
        row.case
    );
    str_field(&created, "ProcessingJobArn")
}

/// `StopProcessingJob` for `name` in `region`, as the raw (status, body) pair, so a test
/// can assert on a refusal as well as on a stopped job.
///
/// `pub(crate)` so the sibling suite that adds the stop-target axes this one does not cover
/// drives the same request rather than a second copy of it.
pub(crate) async fn stop(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "StopProcessingJob",
            &json!({ "ProcessingJobName": name }),
        )
        .await
}

/// `DescribeProcessingJob` for `name` in `region`, as the raw (status, body) pair.
///
/// `pub(crate)` for the same reason [`stop`] is.
pub(crate) async fn describe(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "DescribeProcessingJob",
            &json!({ "ProcessingJobName": name }),
        )
        .await
}

/// RULE: a stop of a running processing job is accepted with the operation's empty output
/// and moves the stored job into the transient `Stopping` status; the move to the terminal
/// `Stopped` is owed by the CLOCK, carries no failure reason — stopping is what the caller
/// asked for — and is stable across later reads.
#[tokio::test]
async fn stopping_a_running_job_moves_it_to_stopping_then_stopped() {
    for row in stop_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let created_arn = create_job(&emulator, &row).await;

        let (status, stopped) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StopProcessingJob to answer 200, got {status} \
             with {stopped:?}"
        );
        // `StopProcessingJob`'s output shape is `Unit` (smithy_query
        // operation=StopProcessingJob), so the accepted call carries no members.
        assert_eq!(
            stopped,
            json!({}),
            "case {case:?}: expected the stop's modelled empty output, got {stopped:?}"
        );

        // Straight after the stop the terminal move is not due yet, so the transient
        // status is observable.
        let (status, stopping) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop to answer 200, got {status} \
             with {stopping:?}"
        );
        assert_eq!(
            str_field(&stopping, "ProcessingJobStatus"),
            "Stopping",
            "case {case:?}: expected the transient Stopping status after the stop, \
             got {stopping:?}"
        );
        assert_eq!(
            str_field(&stopping, "ProcessingJobArn"),
            created_arn,
            "case {case:?}: expected the stopped job to keep the ARN the create minted \
             ({created_arn}), got {stopping:?}"
        );
        assert!(
            stopping.get("FailureReason").is_none() && stopping.get("ProcessingEndTime").is_none(),
            "case {case:?}: expected neither FailureReason nor ProcessingEndTime while the \
             job is Stopping, got {stopping:?}"
        );

        tokio::time::sleep(PAST_STOP_DUE).await;

        let (status, after) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop fell due to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            str_field(&after, "ProcessingJobStatus"),
            "Stopped",
            "case {case:?}: expected the job the caller stopped to end in Stopped, \
             got {after:?}"
        );
        assert!(
            after.get("FailureReason").is_none(),
            "case {case:?}: expected no FailureReason beside a status the caller asked \
             for, got {after:?}"
        );
        let end_time = after
            .get("ProcessingEndTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a ProcessingEndTime in {after:?}")
            });
        let creation_time = after
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {after:?}"));
        assert!(
            end_time >= creation_time,
            "case {case:?}: expected ProcessingEndTime {end_time} at or after CreationTime \
             {creation_time}, got {after:?}"
        );
        assert!(
            after.get("ProcessingStartTime").is_none() && after.get("ExitMessage").is_none(),
            "case {case:?}: expected neither ProcessingStartTime nor ExitMessage for a run \
             that never started, got {after:?}"
        );

        // The move happened once: a later read reports exactly the same job, so the
        // terminal status and its timestamps are stable and no read re-stamps them.
        let (status, again) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to answer 200, got {status} \
             with {again:?}"
        );
        assert_eq!(
            again, after,
            "case {case:?}: expected the stopped job to be stable across reads, \
             got {again:?}"
        );
    }
}

/// RULE: a stop of a job whose stop is already in flight is accepted and changes nothing:
/// the job stays in `Stopping` and the span the first stop started is not restarted, so
/// the terminal status still falls due when that first stop made it due.
#[tokio::test]
async fn a_stop_already_in_flight_is_not_restarted() {
    for row in stop_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        create_job(&emulator, &row).await;

        let (status, first) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first stop to answer 200, got {status} \
             with {first:?}"
        );
        let (status, stopping) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the first stop to answer 200, \
             got {status} with {stopping:?}"
        );

        // Repeats while the stop is in flight answer the same way and leave the job where
        // the first stop put it, timestamps included.
        for repeat in 0..2 {
            let (status, again) = stop(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} repeat {repeat}: expected a repeated stop to answer 200, \
                 got {status} with {again:?}"
            );
            let (status, described) = describe(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} repeat {repeat}: expected the read to answer 200, \
                 got {status} with {described:?}"
            );
            assert_eq!(
                described, stopping,
                "case {case:?} repeat {repeat}: expected the repeated stop to leave the \
                 job exactly where the first stop put it, got {described:?}"
            );
        }

        tokio::time::sleep(PAST_STOP_DUE).await;

        let (status, after) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop fell due to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            str_field(&after, "ProcessingJobStatus"),
            "Stopped",
            "case {case:?}: expected the repeats not to push the terminal status past the \
             span the first stop started, got {after:?}"
        );
    }
}

/// RULE: a job that has already reached a status nothing leaves on its own has no run
/// left to stop: the call is refused with HTTP 400 `ValidationException` naming the job
/// and the status it found, and the stored job is left exactly as it was.
#[tokio::test]
async fn a_job_whose_run_is_over_has_nothing_to_stop() {
    // Rows differ along HOW the job reached a terminal status — left to run until the
    // clock ended it, or stopped by an earlier call — and along the job's name, Region and
    // optional members.
    let rows = stop_rows();
    for (index, row) in rows.iter().enumerate() {
        // The first row is left to run; the rest are stopped first, so both ways into a
        // terminal status are covered over different names, Regions and member sets.
        let stop_first = index > 0;
        let case = row.case;
        let emulator = Emulator::new();
        create_job(&emulator, row).await;

        let expected_status = if stop_first {
            let (status, stopped) = stop(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first stop to answer 200, got {status} \
                 with {stopped:?}"
            );
            tokio::time::sleep(PAST_STOP_DUE).await;
            "Stopped"
        } else {
            tokio::time::sleep(PAST_JOB_END_DUE).await;
            "Failed"
        };

        let (status, described) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} with {described:?}"
        );
        assert_eq!(
            str_field(&described, "ProcessingJobStatus"),
            expected_status,
            "case {case:?}: expected the job to have reached {expected_status} before the \
             stop under test, got {described:?}"
        );

        let (status, refused) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a stop of a {expected_status} job to be refused with \
             HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the refusal of a job that is found but not running to \
             be a ValidationException, got {refused:?}"
        );
        let refusal = message(&refused);
        assert!(
            refusal.contains(row.name) && refusal.contains(expected_status),
            "case {case:?}: expected the refusal to name the job {:?} and the status \
             {expected_status} it found, got {refusal:?}",
            row.name
        );

        // The refusal changed nothing: the job still reports what it reported before it.
        let (status, unchanged) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the refusal to answer 200, \
             got {status} with {unchanged:?}"
        );
        assert_eq!(
            unchanged, described,
            "case {case:?}: expected the refused stop to leave the stored job untouched, \
             got {unchanged:?}"
        );
    }
}

/// RULE: a stop of a processing job this account holds no record of resolves to no stop
/// target: it is refused with HTTP 400 `ValidationException` carrying the service's
/// stop-target template and the name the request presented, and creates nothing.
#[tokio::test]
async fn a_name_that_names_no_job_is_refused_as_a_validation_exception() {
    // Rows differ along the ways a name can miss: never created, a prefix and a suffix of
    // a name that exists, a different case of it, and a name created in ANOTHER Region.
    let emulator = Emulator::new();
    let existing = Row {
        case: "the one stored job",
        name: "proc-stop-present",
        region: "us-east-1",
        members: json!({}),
    };
    create_job(&emulator, &existing).await;

    let misses: [(&str, &str, &str); 4] = [
        ("never created", "us-east-1", "proc-stop-absent"),
        ("a prefix of a stored name", "us-east-1", "proc-stop"),
        ("a suffix added to a stored name", "us-east-1", "proc-stop-present2"),
        ("a different case of a stored name", "us-east-1", "PROC-STOP-PRESENT"),
    ];
    for (case, region, name) in misses {
        let (status, refused) = stop(&emulator, region, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a stop of an unknown name to be refused with HTTP \
             400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException for a stop target that does \
             not resolve, got {refused:?}"
        );
        // The service's stop-target template, carrying the name the request presented.
        let expected_message = format!("Could not find job to update with name: {name}");
        assert_eq!(
            message(&refused),
            expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, \
             got {refused:?}"
        );

        // Nothing was created for the name the stop could not resolve.
        let (status, described) = describe(&emulator, region, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the name to still resolve to no job, got {status} \
             with {described:?}"
        );
    }
}

/// RULE: a stop acts on the ONE job it names: every other stored job keeps the status it
/// had, whichever of them the caller stopped.
#[tokio::test]
async fn a_stop_moves_only_the_job_it_names() {
    let emulator = Emulator::new();
    let names = ["proc-stop-a", "proc-stop-b", "proc-stop-c"];
    for name in names {
        create_job(
            &emulator,
            &Row {
                case: "one of three stored jobs",
                name,
                region: "us-east-1",
                members: json!({}),
            },
        )
        .await;
    }

    let stopped_name = names[1];
    let (status, stopped) = stop(&emulator, "us-east-1", stopped_name).await;
    assert_eq!(
        status, 200,
        "case {stopped_name}: expected the stop to answer 200, got {status} with {stopped:?}"
    );

    for name in names {
        let (status, described) = describe(&emulator, "us-east-1", name).await;
        assert_eq!(
            status, 200,
            "case {name}: expected the read to answer 200, got {status} with {described:?}"
        );
        let expected = if name == stopped_name {
            "Stopping"
        } else {
            "InProgress"
        };
        assert_eq!(
            str_field(&described, "ProcessingJobStatus"),
            expected,
            "case {name}: expected {expected} after stopping {stopped_name}, \
             got {described:?}"
        );
    }
}
