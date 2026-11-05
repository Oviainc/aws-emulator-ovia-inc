// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's AI recommendation jobs:
//! `CreateAIRecommendationJob` -> `DescribeAIRecommendationJob` ->
//! `DeleteAIRecommendationJob`, beside the `CreateAIWorkloadConfig` each job references.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! combination of the optional, enum- and union-valued members; the documented
//! `OptimizeModel` default; the single clock-owed move out of `InProgress`; the refusal
//! of a name already in use; the refusal of a job whose workload configuration does not
//! exist; the refusal of a read or a delete of a name that was never created; and the
//! release a delete performs.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator, one request builder
// and one set of body accessors.
use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::AI_RECOMMENDATION_JOB_ENDS_AFTER` (400 ms).
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateAIRecommendationJob).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerRecommendationRole";

/// The workload configuration every job below is created against.
const WORKLOAD_CONFIG: &str = "rec-workload-config";

/// `Tags` with `count` distinct key/value pairs.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// `AdapterSource$ModelPackageArns` / `$S3Uris` with `count` entries. Both lists are
/// modelled `len 1..10` (smithy_query shape=AIAdapterSource).
fn adapter_model_packages(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "AdapterId": format!("adapter-{index}"),
                    "ModelPackageArn": format!(
                        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package/adapters/{index}"
                    ),
                })
            })
            .collect(),
    )
}

/// See [`adapter_model_packages`]: the same adapters named by S3 URI instead.
fn adapter_s3_uris(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "AdapterId": format!("adapter-{index}"),
                    "S3Uri": format!("s3://bucket/adapters/{index}/"),
                })
            })
            .collect(),
    )
}

/// One row of the round-trip table: a case name and the members a
/// `CreateAIRecommendationJob` request carries beyond `AIRecommendationJobName`.
struct Row {
    case: &'static str,
    members: Value,
}

/// The required members of a minimal request, as a JSON object the rows extend.
fn required_members() -> Value {
    json!({
        "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
        "RoleArn": ROLE_ARN,
        "ModelSource": { "S3": { "S3Uri": "s3://bucket/models/llama/" } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/recommendations/" },
        "PerformanceTarget": { "Constraints": [{ "Metric": "ttft-ms" }] },
    })
}

/// A request body: the job name, the required members and whatever the row overrides or
/// adds on top of them.
fn create_body(name: &str, members: &Value) -> Value {
    let mut body = json!({ "AIRecommendationJobName": name });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (member, value) in required_members()
        .as_object()
        .expect("the required members are a JSON object")
    {
        object.insert(member.clone(), value.clone());
    }
    for (member, value) in members.as_object().expect("the row is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// The round-trip table. Rows differ along the axes the round-trip rule generalises
/// over: which optional members are present (`InferenceSpecification`, `ComputeSpec`,
/// `AdapterSource`, `OptimizeModel`, `Tags`, `OutputConfig`'s own optional members), the
/// enum member chosen out of each published catalog (`AIRecommendationMetric`'s
/// `ttft-ms`, `throughput` and `cost`; `AIRecommendationInferenceFramework`'s `LMI` and
/// `VLLM`; `AIRecommendationInstanceType` and `AICapacityReservationPreference` entries
/// — all from `smithy_query(query="shape",…)`), which member of the `ModelSource` and
/// `AdapterSource` unions is filled and how many entries its list carries (1 and the
/// modelled maximum of 10, and 1 and the modelled maximum of 3 instance types), whether
/// the workload configuration is named by name or by ARN, the form of an S3 URI (`s3://`
/// and `https://`, both admitted by `S3Uri`'s modelled pattern
/// `^(https|s3)://([^/]+)/?(.*)$`), the number of performance constraints, and how many
/// tags there are (0, 1 and the modelled `TagList` maximum of 50).
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only",
            members: json!({}),
        },
        Row {
            case: "workload configuration named by ARN",
            members: json!({
                "AIWorkloadConfigIdentifier": format!(
                    "arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{WORKLOAD_CONFIG}"
                ),
            }),
        },
        Row {
            case: "model artifacts named by an https S3 URI",
            members: json!({
                "ModelSource": { "S3": { "S3Uri": "https://bucket.s3.amazonaws.com/models/" } },
            }),
        },
        Row {
            case: "output configuration with a model package group and MLflow tracking",
            members: json!({
                "OutputConfig": {
                    "S3OutputLocation": "s3://bucket/recommendations/run-1",
                    "ModelPackageGroupIdentifier": "optimized-llama-group",
                    "MlflowConfig": {
                        // `AIMlflowResourceArn`'s modelled pattern admits exactly the
                        // `mlflow-app` and `mlflow-tracking-server` kinds.
                        "MlflowResourceArn": format!(
                            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-tracking-server/mlflow-1"
                        ),
                        "MlflowExperimentName": "rec.experiment-1",
                        "MlflowRunName": "run_1",
                    },
                },
            }),
        },
        Row {
            case: "model package group named by ARN",
            members: json!({
                "OutputConfig": {
                    "S3OutputLocation": "s3://bucket/recommendations/",
                    "ModelPackageGroupIdentifier": format!(
                        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:model-package-group/llama-group"
                    ),
                },
            }),
        },
        Row {
            case: "every published performance metric constrained at once",
            members: json!({
                "PerformanceTarget": {
                    "Constraints": [
                        { "Metric": "ttft-ms" },
                        { "Metric": "throughput" },
                        { "Metric": "cost" },
                    ],
                },
            }),
        },
        Row {
            case: "cost-only performance target with the LMI framework",
            members: json!({
                "PerformanceTarget": { "Constraints": [{ "Metric": "cost" }] },
                "InferenceSpecification": { "Framework": "LMI" },
            }),
        },
        Row {
            case: "throughput target with the vLLM framework and one instance type",
            members: json!({
                "PerformanceTarget": { "Constraints": [{ "Metric": "throughput" }] },
                "InferenceSpecification": { "Framework": "VLLM" },
                "ComputeSpec": { "InstanceTypes": ["ml.g6e.2xlarge"] },
            }),
        },
        Row {
            case: "three instance types (the modelled maximum) and a capacity reservation",
            members: json!({
                "ComputeSpec": {
                    "InstanceTypes": ["ml.g5.xlarge", "ml.g6.48xlarge", "ml.p5.48xlarge"],
                    "CapacityReservationConfig": {
                        "CapacityReservationPreference": "capacity-reservations-only",
                        "MlReservationArns": [format!(
                            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:ml-reservation/mr-1"
                        )],
                    },
                },
            }),
        },
        Row {
            case: "one LoRA adapter named by model package ARN",
            members: json!({ "AdapterSource": { "ModelPackageArns": adapter_model_packages(1) } }),
        },
        Row {
            case: "ten LoRA adapters (the modelled maximum) named by S3 URI",
            members: json!({ "AdapterSource": { "S3Uris": adapter_s3_uris(10) } }),
        },
        Row {
            case: "model optimization explicitly disallowed",
            members: json!({ "OptimizeModel": false }),
        },
        Row {
            case: "model optimization explicitly allowed",
            members: json!({ "OptimizeModel": true }),
        },
        Row {
            case: "empty tag list",
            members: json!({ "Tags": tags(0) }),
        },
        Row {
            case: "one tag",
            members: json!({ "Tags": tags(1) }),
        },
        Row {
            case: "fifty tags (TagList maximum)",
            members: json!({ "Tags": tags(50) }),
        },
        Row {
            case: "every member at once",
            members: json!({
                "RoleArn": "arn:aws:iam::000000000000:role/service-role/Rec+Role.1",
                "ModelSource": { "S3": { "S3Uri": "s3://bucket/models/all/" } },
                "OutputConfig": {
                    "S3OutputLocation": "https://bucket.s3.amazonaws.com/out/",
                    "ModelPackageGroupIdentifier": "optimized-llama-group",
                    "MlflowConfig": {
                        "MlflowResourceArn": format!(
                            "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/app-1"
                        ),
                    },
                },
                "PerformanceTarget": {
                    "Constraints": [{ "Metric": "throughput" }, { "Metric": "cost" }],
                },
                "InferenceSpecification": { "Framework": "VLLM" },
                "ComputeSpec": {
                    "InstanceTypes": ["ml.g6e.48xlarge", "ml.p6-b200.48xlarge"],
                    "CapacityReservationConfig": {
                        "CapacityReservationPreference": "capacity-reservations-only",
                    },
                },
                "AdapterSource": { "ModelPackageArns": adapter_model_packages(3) },
                "OptimizeModel": false,
                "Tags": tags(2),
            }),
        },
    ]
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

/// `DescribeAIRecommendationJob` for `name`, as the raw (status, body) pair, so a test
/// can assert on a refusal as well as on a stored job.
async fn describe(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call(
            "DescribeAIRecommendationJob",
            &json!({ "AIRecommendationJobName": name }),
        )
        .await
}

/// `DeleteAIRecommendationJob` for `name`, as the raw (status, body) pair.
async fn delete(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call(
            "DeleteAIRecommendationJob",
            &json!({ "AIRecommendationJobName": name }),
        )
        .await
}

/// RULE: a created AI recommendation job is readable back under its own name, and the
/// read replays exactly the members the create stored — nothing invented, nothing
/// dropped — plus the ARN the create answered, the non-terminal status an accepted job
/// starts in, the effective `OptimizeModel` and a creation timestamp.
#[tokio::test]
async fn describe_replays_every_member_the_create_stored() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let name = "rec-round-trip";
        let body = create_body(name, &row.members);

        let (status, created) = emulator.call("CreateAIRecommendationJob", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateAIRecommendationJob to answer 200, got {status} \
             with {created:?}"
        );
        let arn = str_field(&created, "AIRecommendationJobArn");
        assert_eq!(
            arn,
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-recommendation-job/{name}"),
            "case {case:?}: expected the minted ARN to name the resource kind and name, \
             got {created:?}"
        );

        let (status, described) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAIRecommendationJob to answer 200, got {status} \
             with {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIRecommendationJobName"),
            name,
            "case {case:?}: expected the stored name, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIRecommendationJobArn"),
            arn,
            "case {case:?}: expected the same ARN the create answered, got {described:?}"
        );
        // An accepted job starts in the one non-terminal member of
        // `AIRecommendationJobStatus` (smithy_query shape=AIRecommendationJobStatus:
        // InProgress, Completed, Failed, Stopping, Stopped), so a caller reading straight
        // after the create still observes it.
        assert_eq!(
            str_field(&described, "AIRecommendationJobStatus"),
            "InProgress",
            "case {case:?}: expected a freshly created job to report InProgress, \
             got {described:?}"
        );
        assert!(
            described.get("StartTime").is_none() && described.get("EndTime").is_none(),
            "case {case:?}: expected no run timestamps before the job ends, got {described:?}"
        );
        // Nothing analysed the model, so no recommendation is invented for it — and the
        // list the job holds is reported EMPTY rather than omitted (the rule the
        // `read_responses_report_what_a_resource_holds` suite pins).
        assert_eq!(
            described.get("Recommendations"),
            Some(&json!([])),
            "case {case:?}: expected the empty Recommendations list for a run that never \
             started, got {:?}",
            described.get("Recommendations")
        );

        // Every member the request carried comes back exactly as sent; one the request
        // omitted is absent, never defaulted to a value a caller cannot distinguish.
        for member in [
            "AIWorkloadConfigIdentifier",
            "RoleArn",
            "ModelSource",
            "PerformanceTarget",
            "InferenceSpecification",
            "ComputeSpec",
            "AdapterSource",
        ] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: expected {member} to round-trip, got {:?}",
                described.get(member)
            );
        }

        // `OptimizeModel` is documented "The default is true", so a request that named
        // none reads back the value the job would run under rather than nothing.
        assert_eq!(
            described.get("OptimizeModel"),
            Some(body.get("OptimizeModel").unwrap_or(&Value::Bool(true))),
            "case {case:?}: expected the effective OptimizeModel, got {:?}",
            described.get("OptimizeModel")
        );

        // `OutputConfig` is reported as an `AIRecommendationOutputResult`, which carries
        // the stored destinations and nothing else. Its `S3OutputLocation` is required
        // there, and a create naming none is refused (see
        // `a_create_naming_no_s3_output_location_is_refused`), so every accepted row
        // round-trips the location it sent.
        let sent_output = body.get("OutputConfig").expect("a row sends OutputConfig");
        let read_output = described
            .get("OutputConfig")
            .unwrap_or_else(|| panic!("case {case:?}: expected an OutputConfig in {described:?}"));
        assert_eq!(
            read_output.get("S3OutputLocation"),
            Some(
                sent_output
                    .get("S3OutputLocation")
                    .expect("every accepted row names an S3OutputLocation")
            ),
            "case {case:?}: expected S3OutputLocation to round-trip, got {read_output:?}"
        );
        for member in ["ModelPackageGroupIdentifier", "MlflowConfig"] {
            assert_eq!(
                read_output.get(member),
                sent_output.get(member),
                "case {case:?}: expected OutputConfig {member} to round-trip, \
                 got {read_output:?}"
            );
        }

        let sent_tags = body
            .get("Tags")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let read_tags = described
            .get("Tags")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            read_tags, sent_tags,
            "case {case:?}: expected the stored tags to round-trip, got {:?}",
            described.get("Tags")
        );
        // The tag set is the job's own collection, so the member is present whatever is in
        // it: a job created with no tags reports the EMPTY list rather than going missing
        // (the rule the `read_responses_report_what_a_resource_holds` suite pins).
        if sent_tags.is_empty() {
            assert_eq!(
                described.get("Tags"),
                Some(&json!([])),
                "case {case:?}: expected Tags to be the empty list when none were given, \
                 got {:?}",
                described.get("Tags")
            );
        }

        // The creation timestamp is present and stable: a second read reports the same
        // instant, so the read is reporting rather than re-stamping.
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {described:?}"));
        let (status, again) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second read to answer 200, got {status} with {again:?}"
        );
        assert_eq!(
            again.get("CreationTime").and_then(Value::as_f64),
            Some(creation_time),
            "case {case:?}: expected CreationTime to be stable across reads, got {:?}",
            again.get("CreationTime")
        );
        assert_eq!(
            again, described,
            "case {case:?}: expected a read to change nothing a later read sees, got {again:?}"
        );
    }
}

/// RULE: the ARN a create mints is
/// `arn:aws:sagemaker:<signed region>:<account>:ai-recommendation-job/<name>`, whatever
/// the name's shape and whichever region the caller signed for (the emulator default when
/// the request is unsigned), and a name outside the modelled constraints never reaches
/// the store.
#[tokio::test]
async fn minted_arns_carry_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: the name's shape and the
    // region. `AIRecommendationJobName` targets `AIEntityName`, `len 1..63` with pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` (smithy_query shape=AIEntityName), so the rows
    // sit on both published bounds and in between.
    let names = [
        "r",
        "rec1",
        "nightly-llama-recommendation-job",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            seed_workload_config(&emulator, region).await;
            let case = format!("region={region} name={name}");
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateAIRecommendationJob",
                    &create_body(name, &json!({})),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateAIRecommendationJob to answer 200, got {status} \
                 with {created:?}"
            );
            assert_eq!(
                str_field(&created, "AIRecommendationJobArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:ai-recommendation-job/{name}"),
                "case {case:?}: expected the ARN to carry the signed region and the name, \
                 got {created:?}"
            );
        }
    }

    // A name outside the modelled constraints never reaches the store: the boundary
    // validator refuses it, so no job is created for it. `too_long` is one past the
    // published upper bound of `AIEntityName`'s `len 1..63`.
    let too_long = "a".repeat(64);
    for name in ["-leading-hyphen", "under_score", too_long.as_str()] {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let (status, body) = emulator
            .call("CreateAIRecommendationJob", &create_body(name, &json!({})))
            .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected a constraint refusal with HTTP 400, got {status} \
             with {body:?}"
        );
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned setup: expected CreateAIWorkloadConfig to answer 200, got {status} \
         with {body:?}"
    );
    let (status, body) = emulator
        .call_unsigned(
            "CreateAIRecommendationJob",
            &create_body("unsigned-rec", &json!({})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected CreateAIRecommendationJob to answer 200, got {status} \
         with {body:?}"
    );
    assert_eq!(
        str_field(&body, "AIRecommendationJobArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-recommendation-job/unsigned-rec"),
        "case unsigned: expected the ARN to fall back to the default region, got {body:?}"
    );
}

/// RULE: an accepted recommendation job owes exactly one status move to the CLOCK: it
/// reports the non-terminal `InProgress` until the move falls due, then the terminal
/// status with the reason the run ended, an `EndTime` at the instant the move was due and
/// still no `StartTime` and no recommendation — and the terminal report is stable across
/// later reads.
#[tokio::test]
async fn a_job_leaves_in_progress_only_once_the_clock_owes_it() {
    // The move out of `InProgress` does not depend on which optional members the request
    // carried, and each row waits for the due instant — so this rule is exercised over
    // two representative rows (the minimal request and the one carrying every member),
    // while the round-trip and refusal rules above span the whole table.
    let rows = round_trip_rows();
    let clock_rows = [&rows[0], &rows[rows.len() - 1]];
    for row in clock_rows {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let name = "rec-clock";

        let (status, created) = emulator
            .call("CreateAIRecommendationJob", &create_body(name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateAIRecommendationJob to answer 200, got {status} \
             with {created:?}"
        );

        // Straight after the create the move is not due yet, so the transient status is
        // observable.
        let (status, before) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first read to answer 200, got {status} with {before:?}"
        );
        assert_eq!(
            str_field(&before, "AIRecommendationJobStatus"),
            "InProgress",
            "case {case:?}: expected InProgress before the move falls due, got {before:?}"
        );
        assert!(
            before.get("FailureReason").is_none(),
            "case {case:?}: expected no FailureReason while the job is InProgress, \
             got {before:?}"
        );

        tokio::time::sleep(PAST_JOB_END_DUE).await;

        let (status, after) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the due instant to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            str_field(&after, "AIRecommendationJobStatus"),
            "Failed",
            "case {case:?}: expected the job to end in the modelled Failed status — nothing \
             analysed the model here — got {after:?}"
        );
        assert!(
            !str_field(&after, "FailureReason").is_empty(),
            "case {case:?}: expected a FailureReason beside the terminal status, got {after:?}"
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
        assert_eq!(
            after.get("Recommendations"),
            Some(&json!([])),
            "case {case:?}: expected the empty Recommendations list for a run that never \
             started, got {after:?}"
        );

        // The move happened once: a later read reports exactly the same job, so the
        // terminal status and its timestamps are stable and no read re-stamps them.
        let (status, again) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to answer 200, got {status} \
             with {again:?}"
        );
        assert_eq!(
            again, after,
            "case {case:?}: expected the settled job to be stable across reads, got {again:?}"
        );
    }
}

/// RULE: `AIRecommendationJobName` is unique per account and Region, so a create naming
/// one that already exists is refused with `ResourceInUse` and leaves the stored job
/// exactly as the first create left it.
#[tokio::test]
async fn a_name_already_in_use_is_refused_and_the_stored_job_is_untouched() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let name = "rec-duplicate";

        let (status, first) = emulator
            .call("CreateAIRecommendationJob", &create_body(name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to answer 200, got {status} with {first:?}"
        );
        let (status, before) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first read to answer 200, got {status} with {before:?}"
        );

        // A second create under the same name, carrying DIFFERENT members, must not
        // overwrite the first.
        let (status, refused) = emulator
            .call(
                "CreateAIRecommendationJob",
                &create_body(
                    name,
                    &json!({
                        "RoleArn": "arn:aws:iam::000000000000:role/OtherRecommendationRole",
                        "ModelSource": { "S3": { "S3Uri": "s3://other-bucket/models/" } },
                        "OutputConfig": { "S3OutputLocation": "s3://other-bucket/out/" },
                        "PerformanceTarget": { "Constraints": [{ "Metric": "cost" }] },
                        "Tags": tags(1),
                    }),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate create to answer 400, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case {case:?}: expected the modelled ResourceInUse code, got {refused:?}"
        );

        let (status, after) = describe(&emulator, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the refusal to answer 200, got {status} \
             with {after:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused create to change nothing, got {after:?}"
        );
    }
}

/// RULE: a recommendation job analyses the workload its referenced AI workload
/// configuration defines, so a create naming a configuration that does not exist is
/// refused with `ResourceNotFound` and stores nothing — whichever form (name or ARN)
/// named it.
#[tokio::test]
async fn a_job_referencing_an_unknown_workload_configuration_is_refused() {
    let emulator = Emulator::new();
    seed_workload_config(&emulator, "us-east-1").await;

    // Rows differ along the ways a reference can miss: never created, a prefix of one
    // that was, a case variant (names are matched exactly), and the ARN form of each.
    let bare = ["absent-config", "rec", "REC-WORKLOAD-CONFIG"];
    let mut identifiers: Vec<String> = bare.iter().map(|name| name.to_string()).collect();
    identifiers.extend(
        bare.iter()
            .map(|name| format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{name}")),
    );

    for (index, identifier) in identifiers.iter().enumerate() {
        let name = format!("rec-missing-ref-{index}");
        let (status, body) = emulator
            .call(
                "CreateAIRecommendationJob",
                &create_body(
                    &name,
                    &json!({ "AIWorkloadConfigIdentifier": identifier.clone() }),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case identifier={identifier:?}: expected HTTP 400 for a workload \
             configuration that does not exist, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case identifier={identifier:?}: expected the modelled ResourceNotFound code, \
             got {body:?}"
        );
        // The refused create stored nothing, so the job name is still free to read-miss.
        let (status, read) = describe(&emulator, &name).await;
        assert_eq!(
            status, 400,
            "case identifier={identifier:?}: expected the refused job to be absent, \
             got {status} with {read:?}"
        );
    }

    // And a job naming the configuration that WAS created is accepted, so the refusals
    // above are about the reference and not about a create that refuses everything.
    let (status, body) = emulator
        .call(
            "CreateAIRecommendationJob",
            &create_body("rec-present-ref", &json!({})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case identifier={WORKLOAD_CONFIG:?}: expected HTTP 200 for the configuration that \
         exists, got {status} with {body:?}"
    );
}

/// RULE: the read's `OutputConfig` (an `AIRecommendationOutputResult`) REQUIRES an
/// `S3OutputLocation` whose modelled `S3Uri` pattern `^(https|s3)://([^/]+)/?(.*)$`
/// admits no empty value, and this emulator mints no default results bucket — so a
/// create whose `OutputConfig` names no output location is refused with HTTP 400 and
/// stores nothing, whatever else that configuration carries.
#[tokio::test]
async fn a_create_naming_no_s3_output_location_is_refused() {
    let emulator = Emulator::new();
    seed_workload_config(&emulator, "us-east-1").await;

    // Rows differ along what an `OutputConfig` lacking the location can still carry: an
    // empty structure, only the model package group, only the MLflow destination, and
    // both of those at once.
    let configs = [
        ("empty output configuration", json!({})),
        (
            "only a model package group",
            json!({ "ModelPackageGroupIdentifier": "optimized-llama-group" }),
        ),
        (
            "only an MLflow destination",
            json!({
                "MlflowConfig": {
                    "MlflowResourceArn": format!(
                        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-app/app-1"
                    ),
                },
            }),
        ),
        (
            "a model package group and an MLflow destination",
            json!({
                "ModelPackageGroupIdentifier": "optimized-llama-group",
                "MlflowConfig": {
                    "MlflowResourceArn": format!(
                        "arn:aws:sagemaker:us-east-1:{ACCOUNT}:mlflow-tracking-server/mlflow-1"
                    ),
                },
            }),
        ),
    ];

    for (index, (case, output_config)) in configs.iter().enumerate() {
        let name = format!("rec-no-output-{index}");
        let (status, body) = emulator
            .call(
                "CreateAIRecommendationJob",
                &create_body(&name, &json!({ "OutputConfig": output_config.clone() })),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an OutputConfig naming no \
             S3OutputLocation, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the bad-request ValidationException code rather than \
             a modelled resource failure, got {body:?}"
        );
        // The refused create stored nothing, so the job name is still free to read-miss.
        let (status, read) = describe(&emulator, &name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refused job to be absent, got {status} with {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFound",
            "case {case:?}: expected the modelled ResourceNotFound code, got {read:?}"
        );
    }

    // The same request WITH an output location is accepted, so the refusals above are
    // about the missing member and not about a create that refuses every OutputConfig.
    let (status, body) = emulator
        .call(
            "CreateAIRecommendationJob",
            &create_body(
                "rec-with-output",
                &json!({
                    "OutputConfig": {
                        "S3OutputLocation": "s3://bucket/recommendations/",
                        "ModelPackageGroupIdentifier": "optimized-llama-group",
                    },
                }),
            ),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"output location present\": expected HTTP 200, got {status} with {body:?}"
    );
}

/// RULE: a read of an AI recommendation job that does not exist is refused with
/// `ResourceNotFound`, never answered with a 200 carrying empty members — and distinct
/// names are distinct jobs, each reporting its own ARN and members.
#[tokio::test]
async fn distinct_names_are_distinct_jobs_and_an_unknown_name_is_refused() {
    let emulator = Emulator::new();
    seed_workload_config(&emulator, "us-east-1").await;
    let rows = round_trip_rows();

    for (index, row) in rows.iter().enumerate() {
        let name = format!("rec{index}");
        let (status, created) = emulator
            .call("CreateAIRecommendationJob", &create_body(&name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected create {name} to answer 200, got {status} with {created:?}",
            row.case
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let name = format!("rec{index}");
        let (status, described) = describe(&emulator, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected read {name} to answer 200, got {status} with {described:?}",
            row.case
        );
        assert_eq!(
            str_field(&described, "AIRecommendationJobArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-recommendation-job/{name}"),
            "case {:?}: expected {name}'s own ARN, got {described:?}",
            row.case
        );
        let expected = create_body(&name, &row.members);
        for member in ["ModelSource", "PerformanceTarget", "RoleArn", "ComputeSpec"] {
            assert_eq!(
                described.get(member),
                expected.get(member),
                "case {:?}: expected {name}'s own {member}, got {:?}",
                row.case,
                described.get(member)
            );
        }
    }

    // Rows differ along the ways a name can miss: never created, a prefix of one that
    // was, a suffix, and a case variant.
    for name in ["absent", "rec", "ec0", "REC0", "rec00"] {
        let (status, body) = describe(&emulator, name).await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected HTTP 400 for a name that was never created, \
             got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case name={name:?}: expected the modelled ResourceNotFound code, got {body:?}"
        );
    }
}

/// RULE: deleting an AI recommendation job releases exactly that job — it answers the ARN
/// the job carried, the name is refused by every later read and delete, the name is free
/// for a new create, and every other job is untouched. A name nothing created is refused
/// with `ResourceNotFound` rather than answered 200.
#[tokio::test]
async fn deleting_a_job_releases_it_and_frees_its_name() {
    let rows = round_trip_rows();
    // Rows differ along which members the deleted job carried, and the loop deletes one
    // job out of a store holding several, so the release is shown to be per-name.
    for (index, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        seed_workload_config(&emulator, "us-east-1").await;
        let case = row.case;
        let target = format!("rec-delete-{index}");
        let bystander = format!("rec-keep-{index}");

        for name in [&target, &bystander] {
            let (status, created) = emulator
                .call("CreateAIRecommendationJob", &create_body(name, &row.members))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected create {name} to answer 200, got {status} \
                 with {created:?}"
            );
        }
        let (_, described) = describe(&emulator, &target).await;
        let stored_arn = str_field(&described, "AIRecommendationJobArn");

        // A job is released only once its run has finished, so the delete waits for the
        // move out of `InProgress` the clock owes rather than destroying work in flight.
        tokio::time::sleep(PAST_JOB_END_DUE).await;

        let (status, deleted) = delete(&emulator, &target).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteAIRecommendationJob to answer 200, got {status} \
             with {deleted:?}"
        );
        assert_eq!(
            str_field(&deleted, "AIRecommendationJobArn"),
            stored_arn,
            "case {case:?}: expected the delete to answer the ARN the job carried, \
             got {deleted:?}"
        );

        // The release is observable: the next read of this name misses.
        let (status, read) = describe(&emulator, &target).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the released job to be absent, got {status} with {read:?}"
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFound",
            "case {case:?}: expected the modelled ResourceNotFound code, got {read:?}"
        );

        // A second delete of the same name names no job, so it is refused rather than
        // answered 200 for a release it did not perform.
        let (status, again) = delete(&emulator, &target).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second delete to answer 400, got {status} \
             with {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFound",
            "case {case:?}: expected the modelled ResourceNotFound code, got {again:?}"
        );

        // Every other job is untouched by the delete.
        let (status, kept) = describe(&emulator, &bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the other job to survive the delete, got {status} \
             with {kept:?}"
        );

        // And the released name is free for a new create, which mints the same ARN again.
        let (status, recreated) = emulator
            .call(
                "CreateAIRecommendationJob",
                &create_body(&target, &json!({})),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the released name to be free for a new create, \
             got {status} with {recreated:?}"
        );
        assert_eq!(
            str_field(&recreated, "AIRecommendationJobArn"),
            stored_arn,
            "case {case:?}: expected the re-created job to carry the same ARN, \
             got {recreated:?}"
        );
    }
}
