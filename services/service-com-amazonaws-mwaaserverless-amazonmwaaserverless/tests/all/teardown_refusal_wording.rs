// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE refusal a workflow whose teardown has started gives
//! every write asked of it, in both the forms the model allows.
//!
//! RULE under test: a write asked of a workflow in `DELETING` is refused and nothing is
//! written, and the refusal names the workflow, the status its teardown put it in and
//! the clause the calling operation completes — one wording, whichever error shape
//! carries it. An operation that models `ConflictException` refuses with it ("the
//! resource is in a state that prevents the requested operation",
//! `smithy_query(query="shape", shape="ConflictException")`: `@httpError(409)`, with
//! required `Message`, `ResourceId` and `ResourceType`); one that does not —
//! `TagResource` and `UntagResource` model no conflict
//! (`smithy_query(query="operation", operation="TagResource,UntagResource")`) — refuses
//! with the `ValidationException` it does model, carrying the SAME wording.
//!
//! These suites drive that single refusal through an operation of each kind, over a
//! table spanning what the rule generalises over: which writer is called, how much the
//! request carries (the required members alone, and every optional member the operation
//! models), how many versions and runs the workflow already holds, and which regional
//! endpoint serves the call. The wording and the `ResourceId`/`ResourceType` the refusal
//! reports are asserted exactly, so the two conflict-modelling writers cannot drift
//! apart from each other or from the validation form.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    WORKFLOW_RESOURCE_TYPE, WORKFLOW_STATUS_DELETING,
};

use crate::workflow_deletion_teardown::{with_workflow, workflow_with_versions};
use crate::workflow_lifecycle::{role_arn, Harness};

/// The HTTP status the model gives `ConflictException` (`@httpError(409)`).
const CONFLICT_STATUS: u16 = 409;

/// The HTTP status the model gives `ValidationException` (`@httpError(400)`).
const VALIDATION_STATUS: u16 = 400;

/// The wording every form of the refusal carries for `workflow_arn`, where `action`
/// completes the clause.
fn refusal_message(workflow_arn: &str, action: &str) -> String {
    format!("Workflow {workflow_arn} is {WORKFLOW_STATUS_DELETING} and {action}")
}

/// One row: a label, the regional endpoint the call is served at, how many versions and
/// runs the workflow holds before the refused call, the operation, its request body
/// (`{arn}` standing for the workflow this row created) and the clause that operation
/// completes.
struct Case {
    label: &'static str,
    region: &'static str,
    versions: usize,
    runs: usize,
    operation: &'static str,
    body: fn(&str) -> serde_json::Value,
    action: &'static str,
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "update carrying only the required members",
            region: "us-west-2",
            versions: 1,
            runs: 0,
            operation: "UpdateWorkflow",
            body: |arn| {
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/late.yaml"},
                    "RoleArn": role_arn(),
                })
            },
            action: "cannot be updated",
        },
        Case {
            label: "update carrying every optional member it models",
            region: "eu-west-1",
            versions: 3,
            runs: 2,
            operation: "UpdateWorkflow",
            body: |arn| {
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/full.yaml"},
                    "RoleArn": role_arn(),
                    "Description": "an update that never lands",
                    "Code": {"S3Location": {"Bucket": "code", "ObjectKey": "src.zip"}},
                    // `EngineVersion` is an intEnum whose only declared value is 1
                    // (`smithy_query(query="shape", shape="EngineVersion")`).
                    "EngineVersion": 1,
                    "LoggingConfiguration": {"LogGroupName": "/aws/mwaa-serverless/late"},
                    "NetworkConfiguration": {
                        "SubnetIds": ["subnet-0123456789abcdef0"],
                        "SecurityGroupIds": ["sg-0123456789abcdef0"],
                    },
                    "TriggerMode": "SCHEDULED",
                })
            },
            action: "cannot be updated",
        },
        Case {
            label: "a run asked with only the required member",
            region: "ap-southeast-2",
            versions: 1,
            runs: 0,
            operation: "StartWorkflowRun",
            body: |arn| serde_json::json!({"WorkflowArn": arn}),
            action: "cannot start a run",
        },
        Case {
            label: "a run asked with a fresh client token and override parameters",
            region: "us-east-1",
            versions: 2,
            runs: 1,
            operation: "StartWorkflowRun",
            body: |arn| {
                serde_json::json!({
                    "WorkflowArn": arn,
                    "ClientToken": "token-that-starts-nothing",
                    "OverrideParameters": {"retries": 2, "label": "late"},
                })
            },
            action: "cannot start a run",
        },
    ]
}

/// Create a workflow holding `versions` versions and `runs` runs, delete it, and hold
/// the removal deadline out of reach so the call under test meets the `DELETING` phase
/// rather than an already removed record. Answers its ARN.
async fn workflow_being_deleted(
    harness: &Harness,
    label: &str,
    name: &str,
    versions: usize,
    runs: usize,
) -> String {
    let arn = workflow_with_versions(harness, name, versions).await;
    for index in 0..runs {
        let (status, started) = harness
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected run {index} to start, got {status} {started:?}"
        );
    }
    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    with_workflow(harness, &arn, |workflow| {
        workflow.deletion_due_at = Some(SystemTime::now() + Duration::from_secs(3_600));
    });
    let (status, fetched) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: a workflow being deleted is still readable, got {status} {fetched:?}"
    );
    assert_eq!(
        fetched["WorkflowStatus"].as_str(),
        Some(WORKFLOW_STATUS_DELETING),
        "case {label:?}: expected the workflow to be {WORKFLOW_STATUS_DELETING}, got {:?}",
        fetched["WorkflowStatus"]
    );
    arn
}

#[tokio::test]
async fn every_conflict_modelling_writer_refuses_a_tearing_down_workflow_the_same_way() {
    for case in cases() {
        // A fresh emulator per row, so one row's resources cannot answer another's call.
        let harness = Harness::new(case.region);
        let label = case.label;
        let arn =
            workflow_being_deleted(&harness, label, "tearingDown", case.versions, case.runs).await;
        let versions_before = harness.stored_versions(&arn);
        let runs_before = harness.stored_run_count(&arn);

        let (status, refused) = harness.call(case.operation, (case.body)(&arn)).await;
        assert_eq!(
            status, CONFLICT_STATUS,
            "case {label:?}: a workflow being deleted is in a state that prevents the \
             operation, so {} must answer {CONFLICT_STATUS}, got {status} {refused:?}",
            case.operation
        );
        assert!(
            refused["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ConflictException")),
            "case {label:?}: expected a ConflictException, got {:?}",
            refused["__type"]
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(refusal_message(&arn, case.action).as_str()),
            "case {label:?}: expected the shared refusal wording, got {:?}",
            refused["Message"]
        );
        assert_eq!(
            refused["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: the refusal must name the workflow whose state blocks it, got {:?}",
            refused["ResourceId"]
        );
        assert_eq!(
            refused["ResourceType"].as_str(),
            Some(WORKFLOW_RESOURCE_TYPE),
            "case {label:?}: expected the workflow resource type, got {:?}",
            refused["ResourceType"]
        );

        // Nothing was written: the versions and runs a client reads back are the ones
        // the record carried before the refused call.
        assert_eq!(
            harness.stored_versions(&arn),
            versions_before,
            "case {label:?}: a refused write must append no version, expected \
             {versions_before:?}"
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            runs_before,
            "case {label:?}: a refused write must admit no run, expected {runs_before}, got {}",
            harness.stored_run_count(&arn)
        );
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowStatus"].as_str(),
            Some(WORKFLOW_STATUS_DELETING),
            "case {label:?}: the refused write must leave the teardown running, got {:?}",
            fetched["WorkflowStatus"]
        );
    }
}

#[tokio::test]
async fn both_error_shapes_carry_the_same_refusal_wording() {
    // One workflow, four writers: the two that model `ConflictException` and the two
    // that do not. Every answer must carry the same wording up to its own clause, which
    // is what having ONE builder for the refusal guarantees.
    let harness = Harness::new("eu-central-1");
    let arn = workflow_being_deleted(&harness, "both shapes", "oneWording", 2, 1).await;

    let rows: Vec<(&str, serde_json::Value, u16, &str, &str)> = vec![
        (
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/late.yaml"},
                "RoleArn": role_arn(),
            }),
            CONFLICT_STATUS,
            "ConflictException",
            "cannot be updated",
        ),
        (
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": arn}),
            CONFLICT_STATUS,
            "ConflictException",
            "cannot start a run",
        ),
        (
            "TagResource",
            serde_json::json!({"ResourceArn": arn, "Tags": {"env": "prod"}}),
            VALIDATION_STATUS,
            "ValidationException",
            "cannot be tagged",
        ),
        (
            "UntagResource",
            serde_json::json!({"ResourceArn": arn, "TagKeys": ["team"]}),
            VALIDATION_STATUS,
            "ValidationException",
            "cannot be untagged",
        ),
    ];
    let prefix = refusal_message(&arn, "");
    for (operation, body, expected_status, expected_type, action) in rows {
        let (status, refused) = harness.call(operation, body).await;
        assert_eq!(
            status, expected_status,
            "case {operation:?}: expected HTTP {expected_status}, got {status} {refused:?}"
        );
        assert!(
            refused["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains(expected_type)),
            "case {operation:?}: expected a {expected_type}, got {:?}",
            refused["__type"]
        );
        assert_eq!(
            refused["Message"].as_str(),
            Some(refusal_message(&arn, action).as_str()),
            "case {operation:?}: expected the shared refusal wording, got {:?}",
            refused["Message"]
        );
        let message = refused["Message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(&prefix),
            "case {operation:?}: every form of the refusal shares one wording up to its \
             clause, expected a message starting {prefix:?}, got {message:?}"
        );
    }
}
