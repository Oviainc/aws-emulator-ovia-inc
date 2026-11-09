// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the trial component SageMaker records for a JOB submitted
//! with an `ExperimentConfig`: `CreateTrainingJob` / `CreateProcessingJob` /
//! `CreateTransformJob` -> `ListTrialComponents` -> `DescribeTrialComponent`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! job kind whose create carries an `ExperimentConfig` gets a component of its own,
//! sourced at the job's ARN and attached to the trial the configuration names, so
//! `ListTrialComponents(SourceArn=...)` and `ListTrialComponents(TrialName=...)` both
//! report it; a job submitted WITHOUT an `ExperimentConfig` gets none; and a source ARN
//! no job answers to selects nothing.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{primary_container, str_field, Emulator, ACCOUNT};
use crate::training_job_admission::{image_specification, training_body};

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerJobRole";

/// The experiment and trial every row's `ExperimentConfig` names.
const EXPERIMENT: &str = "tracked-experiment";
const TRIAL: &str = "tracked-trial";

/// Register the experiment and the trial the configurations name, so a component has an
/// existing trial to be attached to (`ExperimentConfig$TrialName` names "an existing
/// trial").
async fn seed_trial(emulator: &Emulator, region: &str) {
    for (target, body) in [
        ("CreateExperiment", json!({ "ExperimentName": EXPERIMENT })),
        (
            "CreateTrial",
            json!({ "TrialName": TRIAL, "ExperimentName": EXPERIMENT }),
        ),
    ] {
        let (status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            status, 200,
            "seeding {region}: {target}, got {status} with {created}"
        );
    }
}

/// Register the model a transform job names.
async fn seed_model(emulator: &Emulator, region: &str, name: &str) {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateModel",
            &json!({
                "ModelName": name,
                "PrimaryContainer": primary_container(
                    "000000000000.dkr.ecr.us-east-1.amazonaws.com/ranker:1",
                ),
                "ExecutionRoleArn": ROLE_ARN,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding {region}/{name}: CreateModel, got {status} with {created}"
    );
}

/// A `CreateProcessingJob` body: every required member, plus `extra`.
fn processing_body(name: &str, extra: &Value) -> Value {
    let mut body = json!({
        "ProcessingJobName": name,
        "RoleArn": ROLE_ARN,
        "AppSpecification": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/proc:1" },
        "ProcessingResources": {
            "ClusterConfig": {
                "InstanceCount": 1,
                "InstanceType": "ml.m5.large",
                "VolumeSizeInGB": 30,
            },
        },
    });
    for (member, value) in extra.as_object().expect("a JSON object of extra members") {
        body[member] = value.clone();
    }
    body
}

/// A `CreateTransformJob` body: every required member, plus `extra`.
fn transform_body(name: &str, model: &str, extra: &Value) -> Value {
    let mut body = json!({
        "TransformJobName": name,
        "ModelName": model,
        "TransformInput": {
            "DataSource": {
                "S3DataSource": { "S3DataType": "S3Prefix", "S3Uri": "s3://demo/input/" },
            },
        },
        "TransformOutput": { "S3OutputPath": "s3://demo/output/" },
        "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
    });
    for (member, value) in extra.as_object().expect("a JSON object of extra members") {
        body[member] = value.clone();
    }
    body
}

/// The `TrialComponentName`s `ListTrialComponents` reports for `filter`.
async fn listed(emulator: &Emulator, region: &str, filter: &Value) -> Vec<String> {
    let (status, listed) = emulator.call_in(region, "ListTrialComponents", filter).await;
    assert_eq!(
        status, 200,
        "listing {region} with {filter}: got {status} with {listed}"
    );
    listed
        .get("TrialComponentSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| {
                    summary
                        .get("TrialComponentName")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row: the job kind, the create it is submitted through, the component name
/// SageMaker gives it and the `SourceType` that component reports.
struct TrackedJobRow {
    case: &'static str,
    region: &'static str,
    target: &'static str,
    job_name: &'static str,
    /// The member the create's answer carries the job's ARN in.
    arn_member: &'static str,
    /// The job's own reader, and the members it takes the job's name in and reports its
    /// status in — the status the job's component must report too.
    describe_target: &'static str,
    describe_member: &'static str,
    status_member: &'static str,
    /// The component's generated name, i.e. the job's name plus its kind's suffix.
    component: &'static str,
    source_type: &'static str,
    display_name: Option<&'static str>,
}

/// RULE: a job submitted with an `ExperimentConfig` has a trial component created FOR it,
/// sourced at the job's own ARN and attached to the trial the configuration names, so a
/// source-keyed and a trial-keyed listing both report it and the describe reports its
/// source.
#[tokio::test]
async fn a_job_submitted_with_an_experiment_config_gets_its_own_trial_component() {
    // Rows differ along the job kind — every create whose input carries an
    // `ExperimentConfig` (smithy_query shape=ExperimentConfig) — the Region signed for,
    // and whether the configuration names a `TrialComponentDisplayName`.
    let rows = [
        TrackedJobRow {
            case: "a training job with a display name",
            region: "us-east-1",
            target: "CreateTrainingJob",
            job_name: "tracked-training-job",
            arn_member: "TrainingJobArn",
            describe_target: "DescribeTrainingJob",
            describe_member: "TrainingJobName",
            status_member: "TrainingJobStatus",
            component: "tracked-training-job-aws-training-job",
            source_type: "SageMakerTrainingJob",
            display_name: Some("the-training-run"),
        },
        TrackedJobRow {
            case: "a processing job with no display name",
            region: "eu-west-1",
            target: "CreateProcessingJob",
            job_name: "tracked-processing-job",
            arn_member: "ProcessingJobArn",
            describe_target: "DescribeProcessingJob",
            describe_member: "ProcessingJobName",
            status_member: "ProcessingJobStatus",
            component: "tracked-processing-job-aws-processing-job",
            source_type: "SageMakerProcessingJob",
            display_name: None,
        },
        TrackedJobRow {
            case: "a transform job with no display name",
            region: "ap-southeast-2",
            target: "CreateTransformJob",
            job_name: "tracked-transform-job",
            arn_member: "TransformJobArn",
            describe_target: "DescribeTransformJob",
            describe_member: "TransformJobName",
            status_member: "TransformJobStatus",
            component: "tracked-transform-job-aws-transform-job",
            source_type: "SageMakerTransformJob",
            display_name: None,
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        seed_trial(&emulator, row.region).await;
        let model = "tracked-model";
        seed_model(&emulator, row.region, model).await;

        let mut config = json!({ "ExperimentName": EXPERIMENT, "TrialName": TRIAL });
        if let Some(display_name) = row.display_name {
            config["TrialComponentDisplayName"] = json!(display_name);
        }
        let extra = json!({ "ExperimentConfig": config });
        let body = match row.target {
            "CreateTrainingJob" => {
                let mut body =
                    training_body(row.job_name, image_specification(), "s3://demo/output/");
                body["ExperimentConfig"] = extra["ExperimentConfig"].clone();
                body
            }
            "CreateProcessingJob" => processing_body(row.job_name, &extra),
            _ => transform_body(row.job_name, model, &extra),
        };

        let (status, created) = emulator.call_in(row.region, row.target, &body).await;
        assert_eq!(
            status, 200,
            "case {:?}: {} got {status} with {created}",
            row.case, row.target
        );
        let job_arn = str_field(&created, row.arn_member);

        // The component the service created for the job: selected by the job's ARN.
        assert_eq!(
            listed(&emulator, row.region, &json!({ "SourceArn": job_arn })).await,
            vec![row.component.to_string()],
            "case {:?}: a source-keyed listing must report the job's own component",
            row.case
        );
        // And by the trial the configuration named, which now holds it.
        assert_eq!(
            listed(&emulator, row.region, &json!({ "TrialName": TRIAL })).await,
            vec![row.component.to_string()],
            "case {:?}: the named trial must hold the job's component",
            row.case
        );
        // And by the experiment the trial belongs to.
        assert_eq!(
            listed(&emulator, row.region, &json!({ "ExperimentName": EXPERIMENT })).await,
            vec![row.component.to_string()],
            "case {:?}: the experiment's trials must hold the job's component",
            row.case
        );

        // The describe reports the component's identity, its source and its display name.
        let (status, described) = emulator
            .call_in(
                row.region,
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": row.component }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeTrialComponent, got {status} with {described}",
            row.case
        );
        assert_eq!(
            str_field(&described, "TrialComponentArn"),
            format!(
                "arn:aws:sagemaker:{}:{ACCOUNT}:experiment-trial-component/{}",
                row.region, row.component
            ),
            "case {:?}: TrialComponentArn, got {described:?}",
            row.case
        );
        assert_eq!(
            described
                .pointer("/Source/SourceArn")
                .and_then(Value::as_str),
            Some(job_arn.as_str()),
            "case {:?}: Source.SourceArn must be the job's ARN, got {described:?}",
            row.case
        );
        assert_eq!(
            described
                .pointer("/Source/SourceType")
                .and_then(Value::as_str),
            Some(row.source_type),
            "case {:?}: Source.SourceType, got {described:?}",
            row.case
        );
        assert_eq!(
            described
                .pointer("/Sources/0/SourceArn")
                .and_then(Value::as_str),
            Some(job_arn.as_str()),
            "case {:?}: Sources must carry the same source, got {described:?}",
            row.case
        );
        assert_eq!(
            str_field(&described, "DisplayName"),
            row.display_name.unwrap_or(row.component),
            "case {:?}: DisplayName, got {described:?}",
            row.case
        );

        // The component REPORTS THE JOB'S run: its status is the status that job's own
        // reader reports, in the vocabulary both enums share.
        let job_status = emulator
            .call_in(
                row.region,
                row.describe_target,
                &json!({ row.describe_member: row.job_name }),
            )
            .await
            .1
            .get(row.status_member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            described
                .pointer("/Status/PrimaryStatus")
                .and_then(Value::as_str),
            Some(job_status.as_str()),
            "case {:?}: Status.PrimaryStatus must be the job's own status, got {described:?}",
            row.case
        );

        // A source ARN no job of this account answers to selects nothing, and neither
        // does the component's own ARN: the filter is on the SOURCE.
        for (label, source) in [
            ("another job's ARN", format!("{job_arn}-other")),
            (
                "the component's own ARN",
                str_field(&described, "TrialComponentArn"),
            ),
        ] {
            assert_eq!(
                listed(&emulator, row.region, &json!({ "SourceArn": source })).await,
                Vec::<String>::new(),
                "case {:?}/{label}: an unrelated source ARN must select nothing",
                row.case
            );
        }
    }
}

/// RULE: only an `ExperimentConfig` makes the service create a component for a job, so a
/// job submitted without one has none — and a component `CreateTrialComponent` made
/// carries no source, so no source-keyed listing reports it.
#[tokio::test]
async fn a_job_submitted_without_an_experiment_config_gets_no_trial_component() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    seed_trial(&emulator, region).await;
    seed_model(&emulator, region, "plain-model").await;

    // Rows: the same three creates, each with no `ExperimentConfig` at all.
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "training",
            "CreateTrainingJob",
            "TrainingJobArn",
            training_body("plain-training-job", image_specification(), "s3://demo/out/"),
        ),
        (
            "processing",
            "CreateProcessingJob",
            "ProcessingJobArn",
            processing_body("plain-processing-job", &json!({})),
        ),
        (
            "transform",
            "CreateTransformJob",
            "TransformJobArn",
            transform_body("plain-transform-job", "plain-model", &json!({})),
        ),
    ];

    for (case, target, arn_member, body) in rows {
        let (status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: {target}, got {status} with {created}"
        );
        let job_arn = str_field(&created, arn_member);
        assert_eq!(
            listed(&emulator, region, &json!({ "SourceArn": job_arn })).await,
            Vec::<String>::new(),
            "case {case}: a job with no ExperimentConfig must have no component"
        );
    }

    // A component created through the API carries no source of its own.
    let created = emulator
        .ok(
            "CreateTrialComponent",
            &json!({ "TrialComponentName": "hand-made-component" }),
        )
        .await;
    let component_arn = str_field(&created, "TrialComponentArn");
    let described = emulator
        .ok(
            "DescribeTrialComponent",
            &json!({ "TrialComponentName": "hand-made-component" }),
        )
        .await;
    assert!(
        described.get("Source").is_none(),
        "a component created through the API reports no Source, got {described:?}"
    );
    assert_eq!(
        listed(&emulator, region, &json!({ "SourceArn": component_arn })).await,
        Vec::<String>::new(),
        "a component with no source is selected by no source ARN"
    );
    assert!(
        listed(&emulator, region, &json!({}))
            .await
            .contains(&"hand-made-component".to_string()),
        "the component itself must still be listed"
    );
}
