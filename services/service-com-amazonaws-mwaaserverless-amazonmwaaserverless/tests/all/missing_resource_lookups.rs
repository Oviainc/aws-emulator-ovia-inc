// SPDX-License-Identifier: CC0-1.0

//! Every MWAA Serverless lookup that cannot resolve the entity its operation needs is
//! refused the SAME way: the modelled `ResourceNotFoundException` shape, answered with
//! **HTTP 400** — the status the recorded service answers these lookups with, not the
//! 404 the shape's `@httpError` names — and with a message that names the entity THAT
//! OPERATION had to resolve, so a client can tell "workflow absent" from "no version
//! to run" from "run absent".
//!
//! The table below spans the class that rule generalises over: which operation is
//! called, which reference is absent (workflow / version / run), and the form, length
//! and region of the absent identifier. The fixture's own request is one row of it.

use crate::workflow_lifecycle::{role_arn, Harness};

/// The status the recorded service answers a lookup miss with (fixture
/// `mwaa-serverless-resource-not-found-errors`, every step).
///
/// `pub(crate)` so another suite asserting a lookup miss uses the same status rather
/// than restating it.
pub(crate) const LOOKUP_MISS_STATUS: u16 = 400;

/// The wire error code the recorded service answers a lookup miss with — the modelled
/// shape, which is also what `smithy_query(query="operation", …)` lists for each of
/// these operations.
const LOOKUP_MISS_TYPE: &str = "ResourceNotFoundException";

/// The entity-level message for an absent workflow. Recorded example from fixture
/// `mwaa-serverless-resource-not-found-errors` (GetWorkflow, DeleteWorkflow).
///
/// `pub(crate)` so another suite whose operation resolves the WORKFLOW asserts the
/// same entity-level message rather than restating it.
pub(crate) const WORKFLOW_MISSING: &str = "Resource not found";

/// The entity-level message for the latest version a run would have executed.
/// Recorded example from the same fixture (StartWorkflowRun, which names no version).
const LATEST_VERSION_MISSING: &str = "Unable to find latest version";

/// The entity-level message for a `WorkflowVersion` the workflow does not hold. Not
/// in the recording (its request named no version); it is the version-level message
/// for a named version, kept distinct from the workflow- and run-level ones.
const NAMED_VERSION_MISSING: &str = "Unable to find the requested version";

/// The entity-level message for an absent run. Recorded example from the same
/// fixture (GetWorkflowRun).
const RUN_MISSING: &str = "Workflow run not found";

/// The `ResourceType` each entity reports. `ResourceId` and `ResourceType` are
/// **required** members of `ResourceNotFoundException`
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so a refusal
/// must carry both.
const RESOURCE_TYPES: &[&str] = &["workflow", "workflow-version", "workflow-run"];

/// One row of the table: a call whose reference resolves to nothing.
struct Case {
    name: String,
    operation: &'static str,
    body: serde_json::Value,
    /// The entity-level message the operation must report.
    expected_message: &'static str,
}

/// A `WorkflowArn` of the modelled form
/// (`^arn:aws…:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$`,
/// `smithy_query(query="shape", shape="WorkflowArn")`) that no store ever held.
fn ghost_arn(region: &str, account: &str, name_and_suffix: &str) -> String {
    format!("arn:aws:airflow-serverless:{region}:{account}:workflow/{name_and_suffix}")
}

/// Pattern-valid ARNs for workflows that were never created, differing in region,
/// account, and the form and length of the resource name.
///
/// `pub(crate)` so every suite that needs an ARN no store ever held draws from one set.
pub(crate) fn ghost_arns() -> Vec<String> {
    // 244 name characters + '-' + 10 suffix characters = the 255-character maximum the
    // `WorkflowArn` resource segment allows.
    let longest = format!("{}-0123456789", "n".repeat(244));
    vec![
        ghost_arn("us-west-2", "000000000000", "a-0123456789"),
        ghost_arn("eu-central-1", "123456789012", "absent.wf_name-ABCDEFGHIJ"),
        ghost_arn("ap-south-1", "999999999999", &longest),
    ]
}

/// `WorkflowVersion` values of the modelled form (`^[a-z0-9]{32}$`, length 32..32)
/// that no workflow in this test ever had.
fn ghost_versions() -> Vec<String> {
    vec![
        "0".repeat(32),
        "f".repeat(32),
        "9e07a1b3c5d7f90123456789abcdef01".to_string(),
    ]
}

/// `RunId` values of the modelled form (`^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`) that no
/// workflow in this test ever held.
fn ghost_run_ids() -> Vec<&'static str> {
    vec!["r", "run-0000000000000000", "a.b-c_d", "RUN.9_absent-xyz"]
}

/// Every row: the four operations that resolve a reference, over an absent workflow,
/// an absent version of a stored workflow and an absent run of a stored workflow.
fn cases(stored_arn: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    for arn in ghost_arns() {
        cases.push(Case {
            name: format!("GetWorkflow/absent-workflow/{arn}"),
            operation: "GetWorkflow",
            body: serde_json::json!({"WorkflowArn": arn}),
            expected_message: WORKFLOW_MISSING,
        });
        cases.push(Case {
            name: format!("DeleteWorkflow/absent-workflow/{arn}"),
            operation: "DeleteWorkflow",
            body: serde_json::json!({"WorkflowArn": arn}),
            expected_message: WORKFLOW_MISSING,
        });
        // StartWorkflowRun has to resolve the version it would run, so for an absent
        // workflow it reports the LATEST version it could not find.
        cases.push(Case {
            name: format!("StartWorkflowRun/absent-workflow/{arn}"),
            operation: "StartWorkflowRun",
            body: serde_json::json!({"WorkflowArn": arn}),
            expected_message: LATEST_VERSION_MISSING,
        });
        // GetWorkflowRun resolves the RUN, so an absent workflow holds no run either.
        cases.push(Case {
            name: format!("GetWorkflowRun/absent-workflow/{arn}"),
            operation: "GetWorkflowRun",
            body: serde_json::json!({"WorkflowArn": arn, "RunId": "run-0123456789abcdef"}),
            expected_message: RUN_MISSING,
        });
    }

    for version in ghost_versions() {
        for operation in ["GetWorkflow", "DeleteWorkflow", "StartWorkflowRun"] {
            cases.push(Case {
                name: format!("{operation}/absent-version/{version}"),
                operation,
                body: serde_json::json!({
                    "WorkflowArn": stored_arn,
                    "WorkflowVersion": version,
                }),
                expected_message: NAMED_VERSION_MISSING,
            });
        }
    }

    for run_id in ghost_run_ids() {
        cases.push(Case {
            name: format!("GetWorkflowRun/absent-run/{run_id}"),
            operation: "GetWorkflowRun",
            body: serde_json::json!({"WorkflowArn": stored_arn, "RunId": run_id}),
            expected_message: RUN_MISSING,
        });
    }

    cases
}

/// Create one workflow so the version- and run-level rows run against a workflow the
/// store really holds, and report its ARN.
async fn stored_workflow(harness: &Harness) -> String {
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "lookupProbe",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/probe.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string()
}

#[tokio::test]
async fn an_unresolvable_reference_is_a_400_validation_failure_naming_its_entity() {
    // The four entity-level messages must stay tellable apart: that is the whole
    // point of reporting the entity the operation resolved.
    let messages = [
        WORKFLOW_MISSING,
        LATEST_VERSION_MISSING,
        NAMED_VERSION_MISSING,
        RUN_MISSING,
    ];
    for (index, message) in messages.iter().enumerate() {
        for other in &messages[index + 1..] {
            assert_ne!(
                message, other,
                "case entity messages: {message:?} and {other:?} must differ so a client \
                 can tell which entity was absent"
            );
        }
    }

    let harness = Harness::new("us-west-2");
    let stored_arn = stored_workflow(&harness).await;

    for case in cases(&stored_arn) {
        let Case {
            name,
            operation,
            body,
            expected_message,
        } = case;
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {name:?}: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {answered:?}"
        );
        assert!(
            answered["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains(LOOKUP_MISS_TYPE)),
            "case {name:?}: expected a {LOOKUP_MISS_TYPE}, got {:?}",
            answered["__type"]
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(expected_message),
            "case {name:?}: expected the message naming the entity that did not resolve, \
             got {:?}",
            answered["Message"]
        );
        assert!(
            answered["ResourceId"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "case {name:?}: ResourceId is required and must name what was looked up, \
             got {:?}",
            answered["ResourceId"]
        );
        assert!(
            answered["ResourceType"]
                .as_str()
                .is_some_and(|kind| RESOURCE_TYPES.contains(&kind)),
            "case {name:?}: ResourceType is required and must name the entity, got {:?}",
            answered["ResourceType"]
        );
    }

    // A refused lookup persists nothing: no workflow, no run, no idempotency replay.
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(
        status, 200,
        "expected ListWorkflows to succeed, got {status} {listed:?}"
    );
    assert_eq!(
        listed["Workflows"].as_array().map(Vec::len),
        Some(1),
        "a refused lookup must not create or remove a workflow, got {:?}",
        listed["Workflows"]
    );
    assert_eq!(
        harness.stored_run_count(&stored_arn),
        0,
        "a refused StartWorkflowRun must not persist a run, got {}",
        harness.stored_run_count(&stored_arn)
    );
}

#[tokio::test]
async fn a_resolvable_reference_still_succeeds_after_the_refusals() {
    let harness = Harness::new("eu-west-1");
    let stored_arn = stored_workflow(&harness).await;

    // A refused call must not poison the resolvable path: the same ARN still reads,
    // starts a run and deletes.
    let (status, refused) = harness
        .call(
            "GetWorkflow",
            serde_json::json!({
                "WorkflowArn": stored_arn,
                "WorkflowVersion": "0".repeat(32),
            }),
        )
        .await;
    assert_eq!(
        status, LOOKUP_MISS_STATUS,
        "case absent version: expected HTTP {LOOKUP_MISS_STATUS}, got {status} {refused:?}"
    );

    let (status, fetched) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": stored_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case stored workflow: expected GetWorkflow to succeed, got {status} {fetched:?}"
    );

    let (status, started) = harness
        .call(
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": stored_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case stored workflow: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    let (status, run) = harness
        .call(
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": stored_arn, "RunId": run_id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case stored run: expected GetWorkflowRun to succeed, got {status} {run:?}"
    );

    let (status, deleted) = harness
        .call(
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": stored_arn}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case stored workflow: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );

    // And once it is gone, the very same ARN is refused like any other absent one.
    for (operation, body, expected) in [
        (
            "GetWorkflow",
            serde_json::json!({"WorkflowArn": stored_arn}),
            WORKFLOW_MISSING,
        ),
        (
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": stored_arn}),
            WORKFLOW_MISSING,
        ),
        (
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": stored_arn}),
            LATEST_VERSION_MISSING,
        ),
        (
            "GetWorkflowRun",
            serde_json::json!({"WorkflowArn": stored_arn, "RunId": run_id}),
            RUN_MISSING,
        ),
    ] {
        let (status, answered) = harness.call(operation, body).await;
        assert_eq!(
            status, LOOKUP_MISS_STATUS,
            "case {operation}/after-delete: expected HTTP {LOOKUP_MISS_STATUS}, got \
             {status} {answered:?}"
        );
        assert_eq!(
            answered["Message"].as_str(),
            Some(expected),
            "case {operation}/after-delete: expected the entity-level message, got {:?}",
            answered["Message"]
        );
    }
}
