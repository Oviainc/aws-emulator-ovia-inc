// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `StopAIRecommendationJob`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, and
//! every stop is proven by reading the job back through `DescribeAIRecommendationJob`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a stop of
//! a running job answers the stored job's ARN and moves it into the transient `Stopping`
//! status, whatever its name, Region or optional members; the clock — not the read — owes
//! it `Stopped`, with the reason the stop recorded beside it; a stop
//! already in flight is not restarted; a job whose run is over has nothing to stop; a name
//! nothing created is refused with the modelled `ResourceNotFound`; and a stop moves the
//! one job it names and no other.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator, one request builder
// and one set of body accessors.
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::AI_RECOMMENDATION_JOB_STOPS_AFTER` (300 ms).
const PAST_STOP_DUE: Duration = Duration::from_millis(550);

/// Long enough to pass `state::AI_RECOMMENDATION_JOB_ENDS_AFTER` (400 ms), after which a
/// job left to run has reached its terminal status.
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateAIRecommendationJob).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerRecommendationRole";

/// The workload configuration every job below is created against.
const WORKLOAD_CONFIG: &str = "stop-workload-config";

/// How SageMaker words the `FailureReason` of a job a caller stopped: a fixed sentence
/// carrying nothing of the request (`state::AI_RECOMMENDATION_STOPPED_BY_USER_REASON`).
const STOPPED_BY_USER: &str = "Job stopped by user request";

/// One row of the stop table: a case name, the job's name and Region, and whatever the
/// request carries beyond `CreateAIRecommendationJob`'s required members.
struct Row {
    case: &'static str,
    name: &'static str,
    region: &'static str,
    members: Value,
}

/// The stop table. Rows differ along the axes the rule generalises over: the shape of the
/// job's name (`AIRecommendationJobName` targets `AIEntityName`, `len 1..63` with pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — smithy_query shape=AIEntityName — so the rows
/// sit on both published bounds and in between), the Region the caller signs for, and
/// which optional members the create carried (`OptimizeModel`, `ComputeSpec`,
/// `InferenceSpecification`, `Tags`): none of them has any bearing on whether a running
/// job can be stopped.
fn stop_rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, shortest name",
            // `AIEntityName`'s minimum length of 1.
            name: "a",
            region: "us-east-1",
            members: json!({}),
        },
        Row {
            case: "hyphenated name, another region",
            name: "rec-stop-job-1",
            region: "eu-west-2",
            members: json!({ "OptimizeModel": false }),
        },
        Row {
            case: "longest name, every optional member",
            // `AIEntityName`'s maximum length of 63.
            name: "r0123456789012345678901234567890123456789012345678901234567890a",
            region: "ap-southeast-1",
            members: json!({
                "OptimizeModel": true,
                "ComputeSpec": { "InstanceTypes": ["ml.g5.12xlarge"] },
                "InferenceSpecification": { "Framework": "VLLM" },
                "Tags": [{ "Key": "team", "Value": "inference" }],
            }),
        },
    ]
}

/// A `CreateAIRecommendationJob` body: the job name, the required members and whatever
/// the row adds on top of them.
fn create_body(name: &str, members: &Value) -> Value {
    let mut body = json!({
        "AIRecommendationJobName": name,
        "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
        "RoleArn": ROLE_ARN,
        "ModelSource": { "S3": { "S3Uri": "s3://bucket/models/llama/" } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/recommendations/" },
        "PerformanceTarget": { "Constraints": [{ "Metric": "ttft-ms" }] },
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (member, value) in members.as_object().expect("the row is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// Create the workload configuration every job here references, in `region`.
async fn seed_workload_config(emulator: &Emulator, region: &str) {
    let (status, body) = emulator
        .call_in(
            region,
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case setup region={region}: expected CreateAIWorkloadConfig to answer 200, \
         got {status} with {body:?}"
    );
}

/// Seed one running job and answer the ARN the create minted.
async fn create_job(emulator: &Emulator, row: &Row) -> String {
    seed_workload_config(emulator, row.region).await;
    let (status, created) = emulator
        .call_in(
            row.region,
            "CreateAIRecommendationJob",
            &create_body(row.name, &row.members),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateAIRecommendationJob to answer 200, got {status} \
         with {created:?}",
        row.case
    );
    str_field(&created, "AIRecommendationJobArn")
}

/// `StopAIRecommendationJob` for `name` in `region`, as the raw (status, body) pair, so a
/// test can assert on a refusal as well as on a stopped job.
async fn stop(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "StopAIRecommendationJob",
            &json!({ "AIRecommendationJobName": name }),
        )
        .await
}

/// `DescribeAIRecommendationJob` for `name` in `region`, as the raw (status, body) pair.
async fn describe(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "DescribeAIRecommendationJob",
            &json!({ "AIRecommendationJobName": name }),
        )
        .await
}

/// RULE: a stop of a running AI recommendation job answers the ARN of the stored job and
/// moves it into the transient `Stopping` status; the move to the terminal `Stopped` is
/// owed by the CLOCK, carries the reason the stop recorded — the run ended because the
/// caller asked it to — and is stable across later reads.
#[tokio::test]
async fn stopping_a_running_job_moves_it_to_stopping_then_stopped() {
    for row in stop_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let created_arn = create_job(&emulator, &row).await;

        let (status, stopped) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StopAIRecommendationJob to answer 200, got {status} \
             with {stopped:?}"
        );
        assert_eq!(
            str_field(&stopped, "AIRecommendationJobArn"),
            created_arn,
            "case {case:?}: expected the stop to answer the ARN the create minted \
             ({created_arn}), got {stopped:?}"
        );
        // That ARN is the one `AIRecommendationJobArn`'s modelled pattern requires
        // (smithy_query operation=StopAIRecommendationJob): this account, the Region the
        // caller signed for and the job's own name.
        let expected_arn = format!(
            "arn:aws:sagemaker:{}:{ACCOUNT}:ai-recommendation-job/{}",
            row.region, row.name
        );
        assert_eq!(
            created_arn, expected_arn,
            "case {case:?}: expected the stopped job's ARN to be {expected_arn}, \
             got {created_arn}"
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
            str_field(&stopping, "AIRecommendationJobStatus"),
            "Stopping",
            "case {case:?}: expected the transient Stopping status after the stop, \
             got {stopping:?}"
        );
        assert!(
            stopping.get("EndTime").is_none(),
            "case {case:?}: expected no EndTime while the job is Stopping, got {stopping:?}"
        );
        // The stop records WHY the run ended without a result, and reports it from the
        // moment the job is stopping: AWS's fixed sentence for a job a caller stopped.
        assert_eq!(
            str_field(&stopping, "FailureReason"),
            STOPPED_BY_USER,
            "case {case:?}: expected the stop's reason while the job is Stopping, \
             got {stopping:?}"
        );

        tokio::time::sleep(PAST_STOP_DUE).await;

        let (status, after) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop fell due to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            str_field(&after, "AIRecommendationJobStatus"),
            "Stopped",
            "case {case:?}: expected the job the caller stopped to end in Stopped, \
             got {after:?}"
        );
        assert_eq!(
            str_field(&after, "FailureReason"),
            STOPPED_BY_USER,
            "case {case:?}: expected the stop's reason to stand beside the terminal \
             status, got {after:?}"
        );
        let end_time = after
            .get("EndTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected an EndTime in {after:?}"));
        let creation_time = after
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {after:?}"));
        assert!(
            end_time >= creation_time,
            "case {case:?}: expected EndTime {end_time} at or after CreationTime \
             {creation_time}, got {after:?}"
        );
        assert!(
            after.get("StartTime").is_none(),
            "case {case:?}: expected no StartTime for a run that never started, got {after:?}"
        );
        // Nothing analysed the model, so the list the job holds is reported EMPTY rather
        // than omitted (the rule `read_responses_report_what_a_resource_holds` pins).
        assert_eq!(
            after.get("Recommendations"),
            Some(&json!([])),
            "case {case:?}: expected the empty Recommendations list for a run that never \
             started, got {after:?}"
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
        let created_arn = create_job(&emulator, &row).await;

        let (status, first) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first stop to answer 200, got {status} \
             with {first:?}"
        );

        // Repeats while the stop is in flight answer the same ARN and leave the job where
        // the first stop put it.
        for repeat in 0..2 {
            let (status, again) = stop(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} repeat {repeat}: expected a repeated stop to answer 200, \
                 got {status} with {again:?}"
            );
            assert_eq!(
                str_field(&again, "AIRecommendationJobArn"),
                created_arn,
                "case {case:?} repeat {repeat}: expected the same stored ARN, got {again:?}"
            );
            let (status, described) = describe(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} repeat {repeat}: expected the read to answer 200, \
                 got {status} with {described:?}"
            );
            assert_eq!(
                str_field(&described, "AIRecommendationJobStatus"),
                "Stopping",
                "case {case:?} repeat {repeat}: expected the job to stay Stopping, \
                 got {described:?}"
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
            str_field(&after, "AIRecommendationJobStatus"),
            "Stopped",
            "case {case:?}: expected the repeats not to push the terminal status past the \
             span the first stop started, got {after:?}"
        );
    }
}

/// RULE: `StopAIRecommendationJob` stops a RUNNING job, so a job that has already reached
/// a status nothing leaves on its own has no run to stop: the call is refused with HTTP
/// 400 naming the job and the status it found, and the stored job is left exactly as it
/// was.
#[tokio::test]
async fn a_job_that_is_no_longer_running_has_nothing_to_stop() {
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
            str_field(&described, "AIRecommendationJobStatus"),
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

/// RULE: a stop of an AI recommendation job that does not exist is refused with the
/// modelled `ResourceNotFound` naming the ARN it could not resolve — minted from the
/// Region the caller signed for — and creates nothing.
#[tokio::test]
async fn a_name_that_names_no_job_is_refused_with_resource_not_found() {
    // Rows differ along the ways a name can miss: never created, a prefix and a suffix of
    // a name that exists, a different case of it, and a name never created in another
    // Region, whose refusal names the ARN of the Region the caller signed for.
    let emulator = Emulator::new();
    let existing = Row {
        case: "the one stored job",
        name: "rec-stop-present",
        region: "us-east-1",
        members: json!({}),
    };
    create_job(&emulator, &existing).await;

    let misses: [(&str, &str, &str); 5] = [
        ("never created", "us-east-1", "rec-stop-absent"),
        ("a prefix of a stored name", "us-east-1", "rec-stop"),
        ("a suffix added to a stored name", "us-east-1", "rec-stop-present2"),
        ("a different case of a stored name", "us-east-1", "REC-STOP-PRESENT"),
        ("never created, in another region", "eu-central-1", "rec-stop-elsewhere"),
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
            "ResourceNotFound",
            "case {case:?}: expected the modelled ResourceNotFound, got {refused:?}"
        );
        // Worded as the service words a job it cannot resolve, quoting the name the
        // request presented — the same sentence the read and the delete refuse with.
        let expected_message = format!("Job '{name}' not found.");
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

/// RULE: a stop acts on the ONE job it names: every other stored job keeps the status and
/// members it had, whichever of them the caller stopped.
#[tokio::test]
async fn a_stop_moves_only_the_job_it_names() {
    let emulator = Emulator::new();
    seed_workload_config(&emulator, "us-east-1").await;
    let names = ["rec-stop-a", "rec-stop-b", "rec-stop-c"];
    for name in names {
        let (status, created) = emulator
            .call("CreateAIRecommendationJob", &create_body(name, &json!({})))
            .await;
        assert_eq!(
            status, 200,
            "case setup {name}: expected the create to answer 200, got {status} \
             with {created:?}"
        );
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
            str_field(&described, "AIRecommendationJobStatus"),
            expected,
            "case {name}: expected {expected} after stopping {stopped_name}, \
             got {described:?}"
        );
    }
}
