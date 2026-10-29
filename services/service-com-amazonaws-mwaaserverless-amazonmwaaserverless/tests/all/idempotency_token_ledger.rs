// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE `@idempotencyToken` rule every operation of this service
//! that admits a `ClientToken` follows: the token is remembered per operation and per
//! regional endpoint, repeating it with the same request replays what that first call
//! produced, and presenting it with a different request is the modelled
//! `ConflictException` naming the token, the operation and the resource the first call
//! produced.
//!
//! The rows below are the operations that admit the token — `CreateWorkflow` and
//! `StartWorkflowRun`, the two `@idempotencyToken` members of this service
//! (`smithy_query(query="digest")`) — so one table drives the shared path through an
//! operation of each, and the key scoping is asserted across both of them at once.

use crate::workflow_lifecycle::{role_arn, Harness};

/// One row of the table: an operation that admits a `ClientToken`, the `ResourceType` its
/// mismatch refusal reports, and the response member that identifies what the token
/// produced.
struct Row {
    case: &'static str,
    /// The `x-amz-target` operation name, which is also what its ledger key and its
    /// mismatch message are expected to name.
    operation: &'static str,
    /// The `ResourceType` a mismatch refusal of this operation reports.
    resource_type: &'static str,
    /// The response member that tells one produced resource from another.
    identity_member: &'static str,
}

/// The operations the rule covers. They differ in the resource the token produces, in the
/// `ResourceType` their refusal reports and in the request shape — not merely in a name.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "CreateWorkflow mints a workflow",
            operation: "CreateWorkflow",
            resource_type: "Workflow",
            identity_member: "WorkflowArn",
        },
        Row {
            case: "StartWorkflowRun starts a run",
            operation: "StartWorkflowRun",
            resource_type: "WorkflowRun",
            identity_member: "RunId",
        },
    ]
}

/// The workflow a row's requests are made against, and the two requests of that row:
/// one the token is first used for, and one that differs from it in a member the
/// request fingerprint covers.
struct Requests {
    /// The workflow the token's first use touches — the `ResourceId` a mismatch refusal
    /// is expected to report for either operation.
    workflow_arn: String,
    first: serde_json::Value,
    differing: serde_json::Value,
}

/// Create the workflow a row needs — without a `ClientToken`, so nothing but the row's
/// own token is ever in the ledger.
async fn workflow_for(harness: &Harness, region: &str, name: &str) -> String {
    let (status, created) = harness
        .call_in(
            region,
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/ledger.yaml"},
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

/// This row's two requests under `token`, against a workflow of `region`'s endpoint.
async fn requests_for(harness: &Harness, row: &Row, region: &str, token: &str) -> Requests {
    match row.operation {
        "CreateWorkflow" => {
            let first = serde_json::json!({
                "Name": "ledgered",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/ledger.yaml"},
                "RoleArn": role_arn(),
                "ClientToken": token,
            });
            let mut differing = first.clone();
            differing["Description"] = serde_json::json!("a request the token was not used for");
            Requests {
                // Filled in by the caller once the first call has answered: a create's
                // workflow does not exist before it.
                workflow_arn: String::new(),
                first,
                differing,
            }
        }
        "StartWorkflowRun" => {
            let arn = workflow_for(harness, region, "started").await;
            let first = serde_json::json!({"WorkflowArn": arn, "ClientToken": token});
            let mut differing = first.clone();
            differing["OverrideParameters"] = serde_json::json!({"retries": 3});
            Requests {
                workflow_arn: arn,
                first,
                differing,
            }
        }
        other => panic!("case {other:?}: the table has no requests for this operation"),
    }
}

/// How many resources this row's operation has produced under `workflow_arn` at
/// `region`'s endpoint: the workflows the endpoint holds, or the runs the store holds for
/// that workflow.
async fn produced_count(harness: &Harness, row: &Row, region: &str, workflow_arn: &str) -> usize {
    match row.operation {
        "CreateWorkflow" => {
            let (status, listed) = harness
                .call_in(region, "ListWorkflows", serde_json::json!({"MaxResults": 100}))
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected ListWorkflows to succeed, got {status} {listed:?}",
                row.case
            );
            listed["Workflows"].as_array().map_or(0, Vec::len)
        }
        "StartWorkflowRun" => harness.stored_run_count(workflow_arn),
        other => panic!("case {other:?}: the table has no resource count for this operation"),
    }
}

#[tokio::test]
async fn every_tokened_operation_replays_its_first_answer_and_refuses_a_changed_request() {
    // Tokens differing in form and length, since the rule is about the ledger and not
    // about the token's spelling. Each is inside `IdempotencyTokenString` (len 1..128,
    // pattern `^[\x21-\x7E]+$`).
    let tokens = [
        "T",
        "token.1",
        "00000000-0000-4000-8000-000000000000",
        "~!@#$%^&*()_+",
    ];

    for row in rows() {
        for token in tokens {
            // A fresh emulator per case, so one case's ledger cannot answer another's
            // call and the produced-resource count below is this case's alone.
            let harness = Harness::new("us-west-2");
            let region = harness.region();
            let mut requests = requests_for(&harness, &row, region, token).await;

            let (status, first) = harness
                .call(row.operation, requests.first.clone())
                .await;
            assert_eq!(
                status, 200,
                "case {:?} token {token:?}: expected the first {} to succeed, got {status} \
                 {first:?}",
                row.case, row.operation
            );
            let identity = first[row.identity_member].clone();
            assert!(
                identity.is_string(),
                "case {:?} token {token:?}: expected the response to identify what it \
                 produced in {}, got {identity:?}",
                row.case,
                row.identity_member
            );
            if requests.workflow_arn.is_empty() {
                requests.workflow_arn = first["WorkflowArn"]
                    .as_str()
                    .expect("a create answers with the workflow it minted")
                    .to_string();
            }

            // The SAME request under the same token: the first answer replayed, not a
            // second resource.
            let (status, replay) = harness.call(row.operation, requests.first.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?} token {token:?}: expected the token replay to succeed, got {status} \
                 {replay:?}",
                row.case
            );
            assert_eq!(
                replay[row.identity_member], identity,
                "case {:?} token {token:?}: a repeated ClientToken must replay the resource the \
                 first call produced, got {:?}",
                row.case, replay[row.identity_member]
            );

            // A DIFFERENT request under the same token: the modelled conflict, naming the
            // token, the operation it was first used for, and the identity of the
            // workflow that first use touched.
            let (status, conflict) = harness
                .call(row.operation, requests.differing.clone())
                .await;
            assert_eq!(
                status, 409,
                "case {:?} token {token:?}: expected ConflictException (409) for the reused \
                 token with a changed request, got {status} {conflict:?}",
                row.case
            );
            assert!(
                conflict["__type"]
                    .as_str()
                    .is_some_and(|kind| kind.contains("ConflictException")),
                "case {:?} token {token:?}: expected a ConflictException, got {:?}",
                row.case,
                conflict["__type"]
            );
            assert_eq!(
                conflict["Message"].as_str(),
                Some(
                    format!(
                        "ClientToken {token} was already used for a {} request with different \
                         parameters",
                        row.operation
                    )
                    .as_str()
                ),
                "case {:?} token {token:?}: expected the conflict to name the token and the \
                 operation, got {:?}",
                row.case,
                conflict["Message"]
            );
            assert_eq!(
                conflict["ResourceId"].as_str(),
                Some(requests.workflow_arn.as_str()),
                "case {:?} token {token:?}: expected the conflict to name the workflow the \
                 token's first use touched, got {:?}",
                row.case,
                conflict["ResourceId"]
            );
            assert_eq!(
                conflict["ResourceType"].as_str(),
                Some(row.resource_type),
                "case {:?} token {token:?}: expected the conflict to report the resource type \
                 this operation produces, got {:?}",
                row.case,
                conflict["ResourceType"]
            );

            // Neither the replay nor the refusal produced a second resource.
            let produced = produced_count(&harness, &row, region, &requests.workflow_arn).await;
            assert_eq!(
                produced, 1,
                "case {:?} token {token:?}: expected exactly one resource per ClientToken, got \
                 {produced}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn one_token_is_remembered_per_operation_and_per_endpoint() {
    let harness = Harness::new("eu-west-1");
    let other_region = "ap-northeast-1";
    let token = "shared-token";

    // ONE token, presented to BOTH operations at the same endpoint. Each operation keys
    // the ledger by itself, so the start neither replays the create's answer nor
    // conflicts with it even though the two requests digest differently.
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": "sharedToken",
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/ledger.yaml"},
                "RoleArn": role_arn(),
                "ClientToken": token,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();

    let start = serde_json::json!({"WorkflowArn": arn, "ClientToken": token});
    let (status, started) = harness.call("StartWorkflowRun", start.clone()).await;
    assert_eq!(
        status, 200,
        "a token recorded for one operation must not refuse another operation, got {status} \
         {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("a started run has a RunId")
        .to_string();

    // And each entry still replays its own answer, so the two entries really are both
    // remembered under the one token.
    let (status, replayed_run) = harness.call("StartWorkflowRun", start.clone()).await;
    assert_eq!(
        status, 200,
        "expected the StartWorkflowRun replay to succeed, got {status} {replayed_run:?}"
    );
    assert_eq!(
        replayed_run["RunId"].as_str(),
        Some(run_id.as_str()),
        "expected the token to replay the run it started, got {:?}",
        replayed_run["RunId"]
    );

    // The same token at ANOTHER endpoint says nothing about what this one produced: the
    // run is started again, under that endpoint's own workflow.
    let other_arn = workflow_for(&harness, other_region, "sharedToken").await;
    let (status, elsewhere) = harness
        .call_in(
            other_region,
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": other_arn, "ClientToken": token}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {other_region:?}: expected StartWorkflowRun to succeed, got {status} {elsewhere:?}"
    );
    assert_ne!(
        elsewhere["RunId"].as_str(),
        Some(run_id.as_str()),
        "case {other_region:?}: a token of another endpoint must not replay, got the already \
         started {run_id:?}"
    );
    assert_eq!(
        harness.stored_run_count(&arn),
        1,
        "the first endpoint's workflow must still hold exactly one run, got {}",
        harness.stored_run_count(&arn)
    );
}
