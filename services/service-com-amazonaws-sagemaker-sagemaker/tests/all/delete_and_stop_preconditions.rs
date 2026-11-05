// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the PRECONDITIONS a SageMaker delete answers before it
//! releases anything, and for what a released resource looks like afterwards.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) on the harness `inference_deployment_lifecycle` declares, so what these
//! tests exercise is what a real SDK client gets.
//!
//! The rule under test, over TABLES of inputs rather than one recorded call:
//!
//! * a delete resolves its target FIRST and is refused — never answered 200 — when this
//!   account holds no such resource, and when the resource has not yet left the
//!   non-terminal status its deletion requires it to leave;
//! * a refused delete changes NOTHING: the resource is still read back, still holds its
//!   name against a new create, and can still be stopped;
//! * an accepted delete makes the resource unreadable by every later caller and frees its
//!   name.
//!
//! The other resources that rule covers are pinned by their own suites:
//! `crate::feature_group_deletion` (the feature group's `Deleting` window and its
//! not-found sentence), `crate::endpoint_config_deletion` and
//! `crate::rejection_fidelity` (the endpoint and endpoint-configuration deletes of a name
//! the account does not hold), `crate::algorithm_lifecycle` (the accepted algorithm
//! delete) and `crate::ai_recommendation_job_lifecycle` (the accepted job delete).

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// Long enough to pass `state::ALGORITHM_COMPLETED_AFTER` (300 ms), so the clock owes a
/// created algorithm its terminal status by the time the next call runs.
const PAST_ALGORITHM_SETTLED: Duration = Duration::from_millis(600);

/// Long enough to pass `state::AI_RECOMMENDATION_JOB_ENDS_AFTER` (400 ms) and
/// `state::AI_RECOMMENDATION_JOB_STOPS_AFTER` (300 ms), so the clock owes a job its
/// terminal status by the time the next call runs.
const PAST_JOB_SETTLED: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateAIRecommendationJob).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerRecommendationRole";

/// The AI workload configuration every recommendation job below references.
const WORKLOAD_CONFIG: &str = "precondition-workload-config";

/// The ARN a SageMaker resource of `kind` named `name` carries in `region`, as the
/// modelled `*Arn` patterns spell it.
fn arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:{kind}/{name}")
}

/// How this service states an `AppImageConfigName` nothing is stored under. Spelled out
/// here rather than imported from the crate, so the wording itself is under test.
fn app_image_config_missing(region: &str, name: &str) -> String {
    format!(
        "AppImageConfig [{}] does not exist",
        arn(region, "app-image-config", name)
    )
}

/// A `CreateAppImageConfig` body: the name, plus whatever app configuration the row
/// carries (every one of the three is optional).
fn app_image_config_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({ "AppImageConfigName": name });
    let object = body.as_object_mut().expect("a JSON object body");
    for (member, value) in extra.as_object().expect("a JSON object of extra members") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// A `TrainingSpecification` carrying the three members the shape marks required
/// (smithy_query query="shape" shape="TrainingSpecification").
fn training_specification() -> Value {
    json!({
        "TrainingImage": "demo-training:1",
        "SupportedTrainingInstanceTypes": ["ml.m5.large"],
        "TrainingChannels": [{
            "Name": "train",
            "SupportedContentTypes": ["text/csv"],
            "SupportedInputModes": ["File"],
        }],
    })
}

/// An `AlgorithmValidationSpecification` with the one profile the model allows
/// (`ValidationProfiles` is `len 1..1`), each member it marks required filled.
fn validation_specification() -> Value {
    json!({
        "ValidationRole": "arn:aws:iam::000000000000:role/SMValidationRole",
        "ValidationProfiles": [{
            "ProfileName": "profile1",
            "TrainingJobDefinition": {
                "TrainingInputMode": "File",
                "InputDataConfig": [{
                    "ChannelName": "train",
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://example-bucket/train/",
                        },
                    },
                }],
                "OutputDataConfig": { "S3OutputPath": "s3://example-bucket/out/" },
                "ResourceConfig": {
                    "InstanceType": "ml.m5.large",
                    "InstanceCount": 1,
                    "VolumeSizeInGB": 10,
                },
                "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
            },
        }],
    })
}

/// A `CreateAlgorithm` body: the name, the required `TrainingSpecification` and whatever
/// the row adds.
fn algorithm_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "AlgorithmName": name,
        "TrainingSpecification": training_specification(),
    });
    let object = body.as_object_mut().expect("a JSON object body");
    for (member, value) in extra.as_object().expect("a JSON object of extra members") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// A `CreateAIRecommendationJob` body carrying every member the operation marks required.
fn recommendation_job_body(name: &str) -> Value {
    json!({
        "AIRecommendationJobName": name,
        "AIWorkloadConfigIdentifier": WORKLOAD_CONFIG,
        "RoleArn": ROLE_ARN,
        "ModelSource": { "S3": { "S3Uri": "s3://bucket/models/llama/" } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/recommendations/" },
        "PerformanceTarget": { "Constraints": [{ "Metric": "ttft-ms" }] },
    })
}

/// Create the AI workload configuration every recommendation job here references.
async fn seed_workload_config(emulator: &Emulator) {
    emulator
        .ok(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": WORKLOAD_CONFIG }),
        )
        .await;
}

/// RULE: a delete whose target this account does not hold is REFUSED, carrying the
/// refusal class and the sentence that resource's family words the fact with — never a
/// 200 that would report a release it did not perform.
#[tokio::test]
async fn a_delete_of_a_target_the_account_does_not_hold_is_refused() {
    // Rows differ along the resource kind (so the rule is asserted over the class, not
    // one operation), along the refusal class each kind's model admits, and along HOW the
    // name misses: never created, a case variant of a stored name, a prefix of one, and a
    // name with a stored one as its prefix. A resource whose name differs only in case
    // exists throughout, so each refusal is about the identifier as sent rather than
    // about an empty store.
    let kinds: [(&str, &str, &str, &str); 3] = [
        (
            "app-image-config",
            "DeleteAppImageConfig",
            "AppImageConfigName",
            "ResourceNotFound",
        ),
        (
            "algorithm",
            "DeleteAlgorithm",
            "AlgorithmName",
            "ValidationException",
        ),
        (
            "ai-recommendation-job",
            "DeleteAIRecommendationJob",
            "AIRecommendationJobName",
            "ResourceNotFound",
        ),
    ];
    let misses = ["absent", "Present", "presen", "present2"];

    for (kind, operation, member, expected_code) in kinds {
        let emulator = Emulator::new();
        seed_workload_config(&emulator).await;
        // One resource of this kind is stored under `present` throughout.
        match kind {
            "app-image-config" => {
                emulator
                    .ok(
                        "CreateAppImageConfig",
                        &app_image_config_body("present", &json!({})),
                    )
                    .await;
            }
            "algorithm" => {
                emulator
                    .ok("CreateAlgorithm", &algorithm_body("present", &json!({})))
                    .await;
            }
            _ => {
                emulator
                    .ok(
                        "CreateAIRecommendationJob",
                        &recommendation_job_body("present"),
                    )
                    .await;
            }
        }

        for missing in misses {
            let case = format!("{kind}/{missing}");
            let (status, refused) = emulator
                .call(operation, &json!({ member: missing }))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a delete of a target the account \
                 does not hold, got {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                expected_code,
                "case {case:?}: expected {expected_code}, got {refused:?}"
            );
            // Each family words the fact its own recorded way; what every row shares is
            // that the refusal names what the request asked for.
            let expected_message = match kind {
                "app-image-config" => app_image_config_missing("us-east-1", missing),
                "algorithm" => {
                    format!("Algorithm {} does not exist.", arn("us-east-1", "algorithm", missing))
                }
                _ => format!("Job '{missing}' not found."),
            };
            if kind == "ai-recommendation-job" {
                // This delete relays the inner service's rendering, whose tail carries
                // that call's own request id, so the sentence is asserted as the prefix
                // it is.
                assert!(
                    message(&refused).starts_with(&expected_message),
                    "case {case:?}: expected the refusal to open with \
                     {expected_message:?}, got {:?}",
                    message(&refused)
                );
            } else {
                assert_eq!(
                    message(&refused),
                    expected_message,
                    "case {case:?}: refusal sentence, got {:?}",
                    message(&refused)
                );
            }
        }
    }
}

/// RULE: deleting an AppImageConfig releases exactly that configuration — every later
/// read of the name is refused, a second delete is refused, the name is free for a new
/// create, and every other configuration is untouched.
#[tokio::test]
async fn deleting_an_app_image_config_makes_it_unreadable_and_frees_its_name() {
    // Rows differ along which app configuration the deleted record carried (the three
    // optional members `CreateAppImageConfig` accepts, and none at all) and along the
    // Region the request signs for, because the ARN the refusal names is the one the
    // signed Region resolves.
    let rows: [(&str, &str, Value); 4] = [
        ("none", "us-east-1", json!({})),
        (
            "kernel-gateway",
            "eu-west-1",
            json!({
                "KernelGatewayImageConfig": {
                    "KernelSpecs": [{ "Name": "python3", "DisplayName": "Python 3" }],
                    "FileSystemConfig": { "DefaultUid": 0, "DefaultGid": 0 },
                },
            }),
        ),
        (
            "jupyter-lab",
            "ap-southeast-2",
            json!({
                "JupyterLabAppImageConfig": {
                    "ContainerConfig": { "ContainerEntrypoint": ["/bin/bash"] },
                },
            }),
        ),
        (
            "code-editor",
            "us-west-2",
            json!({
                "CodeEditorAppImageConfig": {
                    "ContainerConfig": { "ContainerArguments": ["--verbose"] },
                },
            }),
        ),
    ];

    for (case, region, extra) in rows {
        let emulator = Emulator::new();
        let name = format!("probe-{case}");
        emulator
            .call_in(
                region,
                "CreateAppImageConfig",
                &app_image_config_body(&name, &extra),
            )
            .await;
        emulator
            .call_in(
                region,
                "CreateAppImageConfig",
                &app_image_config_body("bystander", &json!({})),
            )
            .await;

        // It reads back before the delete, so the refusal afterwards is the delete's doing.
        let (read_status, described) = emulator
            .call_in(
                region,
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": &name }),
            )
            .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected the stored configuration to read back, got \
             {read_status} with {described:?}"
        );
        assert_eq!(
            str_field(&described, "AppImageConfigArn"),
            arn(region, "app-image-config", &name),
            "case {case:?}: AppImageConfigArn, got {described:?}"
        );

        let (delete_status, deleted) = emulator
            .call_in(
                region,
                "DeleteAppImageConfig",
                &json!({ "AppImageConfigName": &name }),
            )
            .await;
        assert_eq!(
            delete_status, 200,
            "case {case:?}: expected HTTP 200 from DeleteAppImageConfig, got \
             {delete_status} with {deleted:?}"
        );
        let members = match &deleted {
            Value::Null => 0,
            Value::Object(members) => members.len(),
            other => panic!("case {case:?}: expected an empty output, got {other}"),
        };
        assert_eq!(
            members, 0,
            "case {case:?}: the modelled output has no members, got {deleted:?}"
        );

        // The deletion is OBSERVABLE: the very next read of the name is refused, which is
        // what lets a client polling for the deletion terminate.
        let (after_status, after) = emulator
            .call_in(
                region,
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": &name }),
            )
            .await;
        assert_eq!(
            after_status, 400,
            "case {case:?}: expected the deleted configuration to be gone, got \
             {after_status} with {after:?}"
        );
        assert_eq!(
            error_code(&after),
            "ResourceNotFound",
            "case {case:?}: expected the modelled ResourceNotFound code, got {after:?}"
        );
        assert_eq!(
            message(&after),
            app_image_config_missing(region, &name),
            "case {case:?}: refusal sentence after the delete, got {:?}",
            message(&after)
        );

        // The update of the same name resolves it the same way, and a second delete is
        // refused rather than reporting a release it did not perform.
        for operation in ["UpdateAppImageConfig", "DeleteAppImageConfig"] {
            let (status, refused) = emulator
                .call_in(region, operation, &json!({ "AppImageConfigName": &name }))
                .await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "ResourceNotFound"),
                "case {case:?}/{operation}: expected 400 ResourceNotFound, got \
                 {status} with {refused:?}"
            );
            assert_eq!(
                message(&refused),
                app_image_config_missing(region, &name),
                "case {case:?}/{operation}: refusal sentence, got {:?}",
                message(&refused)
            );
        }

        // Only that record went: the sibling is still described.
        let (survivor_status, survivor) = emulator
            .call_in(
                region,
                "DescribeAppImageConfig",
                &json!({ "AppImageConfigName": "bystander" }),
            )
            .await;
        assert_eq!(
            survivor_status, 200,
            "case {case:?}: expected the other configuration to survive, got \
             {survivor_status} with {survivor:?}"
        );

        // And the freed name is creatable again, minting the same ARN.
        let (recreate_status, recreated) = emulator
            .call_in(
                region,
                "CreateAppImageConfig",
                &app_image_config_body(&name, &json!({})),
            )
            .await;
        assert_eq!(
            recreate_status, 200,
            "case {case:?}: expected the freed name to be creatable again, got \
             {recreate_status} with {recreated:?}"
        );
        assert_eq!(
            str_field(&recreated, "AppImageConfigArn"),
            arn(region, "app-image-config", &name),
            "case {case:?}: the re-created configuration's ARN, got {recreated:?}"
        );
    }
}

/// RULE: an algorithm is deleted only once its registration has left the non-terminal
/// status it is born in; a delete before that is refused, naming the status in the way,
/// and the algorithm SURVIVES — it still reads back, still holds its name against a new
/// create, and is deletable once the status the clock owes has fallen due.
#[tokio::test]
async fn deleting_an_algorithm_is_refused_while_its_registration_is_pending() {
    // Rows differ along what the created algorithm carries, and so along the TERMINAL
    // status its registration settles into: an algorithm with nothing to validate ends
    // `Completed`, one whose create asked for validation jobs ends `Failed`. Both are
    // terminal, so both are deletable once settled.
    let rows: [(&str, Value, &str); 3] = [
        ("plain", json!({}), "Completed"),
        (
            "with-tags",
            json!({ "Tags": [{ "Key": "team", "Value": "ml" }] }),
            "Completed",
        ),
        (
            "with-validation",
            json!({ "ValidationSpecification": validation_specification() }),
            "Failed",
        ),
    ];

    for (case, extra, terminal_status) in rows {
        let emulator = Emulator::new();
        let created = emulator
            .ok("CreateAlgorithm", &algorithm_body(case, &extra))
            .await;
        let algorithm_arn = str_field(&created, "AlgorithmArn");

        // The delete runs while the algorithm is still `Pending` — the status is
        // observable, because the span the clock owes is never zero.
        let (status, refused) = emulator
            .call("DeleteAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a delete of a Pending algorithm, got \
             {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            "Cannot delete Algorithm in Pending status.",
            "case {case:?}: refusal sentence, got {:?}",
            message(&refused)
        );

        // The refused delete removed NOTHING: the algorithm is still described, still
        // `Pending`, and its name is still in use.
        let described = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            str_field(&described, "AlgorithmStatus"),
            "Pending",
            "case {case:?}: the refused delete must leave the status alone, got \
             {described:?}"
        );
        assert_eq!(
            str_field(&described, "AlgorithmArn"),
            algorithm_arn,
            "case {case:?}: the surviving algorithm keeps its ARN, got {described:?}"
        );
        let (duplicate_status, duplicate) = emulator
            .call("CreateAlgorithm", &algorithm_body(case, &extra))
            .await;
        assert_eq!(
            duplicate_status, 400,
            "case {case:?}: the surviving algorithm must still hold its name, got \
             {duplicate_status} with {duplicate:?}"
        );
        assert_eq!(
            message(&duplicate),
            format!("Algorithm already exists: {algorithm_arn}"),
            "case {case:?}: duplicate-name refusal, got {:?}",
            message(&duplicate)
        );

        // Once the transition the clock owed has fallen due, the algorithm reports its
        // terminal status and the same delete is accepted.
        tokio::time::sleep(PAST_ALGORITHM_SETTLED).await;
        let settled = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            str_field(&settled, "AlgorithmStatus"),
            terminal_status,
            "case {case:?}: expected the clock's terminal status, got {settled:?}"
        );
        emulator
            .ok("DeleteAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        let (gone_status, gone) = emulator
            .call("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            gone_status, 400,
            "case {case:?}: expected the deleted algorithm to be gone, got {gone_status} \
             with {gone:?}"
        );
        assert_eq!(
            message(&gone),
            format!("Algorithm {algorithm_arn} does not exist."),
            "case {case:?}: refusal sentence after the accepted delete, got {:?}",
            message(&gone)
        );
    }
}

/// RULE: an AI recommendation job is deleted only once its run has finished; a delete
/// while it is still queued, or still stopping, is refused — naming the state in the way
/// and the stop that ends it — and the job SURVIVES, so the stop still answers its ARN
/// and a read still reaches its terminal status.
///
/// The state this refusal names is the one the service holding the job uses (`Pending`
/// for a job whose run has not started, `Stopping` for one with a stop in flight), which
/// is a different vocabulary from the `AIRecommendationJobStatus` a read reports.
#[tokio::test]
async fn deleting_an_ai_recommendation_job_is_refused_while_it_is_running() {
    // Rows differ along HOW the job reaches a terminal status: the caller stops it (so
    // the delete is also refused in the transient `Stopping` state, and the job ends
    // `Stopped`), or it is left to run out (and ends `Failed`, nothing here having
    // analysed a model).
    let rows: [(&str, bool, &str); 2] = [
        ("stopped-by-the-caller", true, "Stopped"),
        ("left-to-run-out", false, "Failed"),
    ];

    for (case, stop_it, terminal_status) in rows {
        let emulator = Emulator::new();
        seed_workload_config(&emulator).await;
        let created = emulator
            .ok("CreateAIRecommendationJob", &recommendation_job_body(case))
            .await;
        let job_arn = str_field(&created, "AIRecommendationJobArn");

        // The delete runs while the job is still `InProgress`.
        let (status, refused) = emulator
            .call(
                "DeleteAIRecommendationJob",
                &json!({ "AIRecommendationJobName": case }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a delete of a running job, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            "Cannot delete job in 'Pending' state. Stop the job first.",
            "case {case:?}: refusal sentence, got {:?}",
            message(&refused)
        );

        // The refused delete released nothing: the job is still read back, running.
        let (read_status, described) = emulator
            .call(
                "DescribeAIRecommendationJob",
                &json!({ "AIRecommendationJobName": case }),
            )
            .await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: the refused delete must leave the job readable, got \
             {read_status} with {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIRecommendationJobStatus"),
            "InProgress",
            "case {case:?}: the refused delete must leave the status alone, got \
             {described:?}"
        );

        if stop_it {
            // The stop the refusal pointed at is served, because the job still exists.
            let stopped = emulator
                .ok(
                    "StopAIRecommendationJob",
                    &json!({ "AIRecommendationJobName": case }),
                )
                .await;
            assert_eq!(
                str_field(&stopped, "AIRecommendationJobArn"),
                job_arn,
                "case {case:?}: the stop must answer the ARN the job carried, got \
                 {stopped:?}"
            );

            // `Stopping` is transient too, so the delete is still refused — now naming
            // that state.
            let (stopping_status, stopping) = emulator
                .call(
                    "DeleteAIRecommendationJob",
                    &json!({ "AIRecommendationJobName": case }),
                )
                .await;
            assert_eq!(
                (stopping_status, error_code(&stopping).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException while stopping, got \
                 {stopping_status} with {stopping:?}"
            );
            assert_eq!(
                message(&stopping),
                "Cannot delete job in 'Stopping' state. Stop the job first.",
                "case {case:?}: refusal sentence while stopping, got {:?}",
                message(&stopping)
            );
        }

        // Once the clock owes the terminal status, a read reports it — which is what lets
        // a client polling for the job to finish terminate.
        tokio::time::sleep(PAST_JOB_SETTLED).await;
        let terminal = emulator
            .ok(
                "DescribeAIRecommendationJob",
                &json!({ "AIRecommendationJobName": case }),
            )
            .await;
        assert_eq!(
            str_field(&terminal, "AIRecommendationJobStatus"),
            terminal_status,
            "case {case:?}: expected the clock's terminal status, got {terminal:?}"
        );

        // And the delete of a finished job is accepted, answering the ARN it carried.
        let released = emulator
            .ok(
                "DeleteAIRecommendationJob",
                &json!({ "AIRecommendationJobName": case }),
            )
            .await;
        assert_eq!(
            str_field(&released, "AIRecommendationJobArn"),
            job_arn,
            "case {case:?}: the delete must answer the ARN the job carried, got \
             {released:?}"
        );
        let (gone_status, gone) = emulator
            .call(
                "DescribeAIRecommendationJob",
                &json!({ "AIRecommendationJobName": case }),
            )
            .await;
        assert_eq!(
            (gone_status, error_code(&gone).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected the released job to be absent, got {gone_status} \
             with {gone:?}"
        );
    }
}
