// SPDX-License-Identifier: CC0-1.0

//! How CodePipeline WORDS `PipelineNotFoundException`, end to end over the wire the
//! service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! RULE: every operation that resolves a pipeline NAME refuses an absent one with the
//! same modelled error — HTTP 400, `__type` `PipelineNotFoundException`
//! (`smithy_query(query="operation", operation="GetPipeline,StartPipelineExecution,
//! RollbackStage")` reports `errorKind=client`, `httpError=400` for each) — but the
//! MESSAGE belongs to the PLANE of the API the call came in on, not to the service as a
//! whole: the pipeline-declaration calls, the approval answer, a single execution read,
//! the execution listings, the execution-control calls and the stage rollback each carry
//! their own fixed sentence, into which only the pipeline name the request asked for and
//! the caller's account are interpolated.
//!
//! So this suite is a table of every operation that raises the error against the plane
//! it belongs to, crossed with a set of absent names that differ in alphabet and in
//! length (both ends of `PipelineName`'s modelled `@length(1, 100)`). Three further
//! properties are asserted with it: the status and the code are shared by every row, the
//! six planes' sentences are genuinely distinct, and a name the store DOES hold is never
//! refused with this error — so what is pinned is the name lookup and not a blanket
//! refusal.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::pipeline_lifecycle::{build_stage, source_stage, Emulator, ACCOUNT_ID, ROLE_ARN};

/// An execution id of the modelled form (`PipelineExecutionId` is
/// `@pattern("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")`) that
/// nothing ever minted: the rows that need one are refused for the NAME before the id is
/// ever resolved.
const UNMINTED_EXECUTION_ID: &str = "11111111-2222-4333-8444-555555555555";

/// An approval token of the modelled form (`ApprovalToken` carries the same pattern).
const UNMINTED_APPROVAL_TOKEN: &str = "22222222-3333-4444-8555-666666666666";

/// The name of the one pipeline the store holds, so a refusal below is the NAME's lookup
/// failing and not an empty store.
const STORED: &str = "stored";

/// How a row builds the request that names `name` as the pipeline it addresses.
type Request = fn(&str) -> Value;

/// The planes CodePipeline words this refusal for, one variant each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Plane {
    /// The calls that read or write the pipeline DECLARATION and the things recorded
    /// against it — its stage transitions and its action revisions.
    Definition,
    /// The call that answers a manual approval action.
    Approval,
    /// A single execution read.
    ExecutionRead,
    /// The execution listings.
    ExecutionListing,
    /// The calls that start, stop or retry work.
    ExecutionControl,
    /// The stage rollback, whose sentence names no account at all.
    StageRollback,
}

/// The sentence AWS answers with, for `plane`, over the account the lookup was scoped
/// to and the name the request asked for.
fn expected_message(plane: Plane, name: &str) -> String {
    match plane {
        Plane::Definition => {
            format!("Account '{ACCOUNT_ID}' does not have a pipeline with name '{name}'")
        }
        Plane::Approval => format!(
            "{name} was not found in account {ACCOUNT_ID}. Make sure you are using the correct \
             name for the pipeline, and then try again."
        ),
        Plane::ExecutionRead => format!(
            "The account with id '{ACCOUNT_ID}' does not include a pipeline with the name \
             '{name}'"
        ),
        Plane::ExecutionListing => format!(
            "The account with AWS account ID '{ACCOUNT_ID}' does not include a pipeline named \
             '{name}'"
        ),
        Plane::ExecutionControl => {
            format!("Pipeline with name {name} does not exist in account {ACCOUNT_ID}.")
        }
        Plane::StageRollback => format!("Pipeline '{name}' does not exist."),
    }
}

/// Every operation that resolves a pipeline name, with the plane whose sentence it
/// carries and the request that names `name` as its pipeline.
///
/// The operations that model the error but resolve no name are NOT rows: `PutWebhook`
/// stores a definition over a pipeline name and resolves nothing (see `webhooks.rs`),
/// while `OverrideStageCondition` and `ListDeployActionExecutionTargets` are still
/// scaffolded and answer 501.
fn rows() -> Vec<(&'static str, Plane, Request)> {
    vec![
        ("GetPipeline", Plane::Definition, |name| {
            json!({"name": name})
        }),
        ("GetPipelineState", Plane::Definition, |name| {
            json!({"name": name})
        }),
        ("UpdatePipeline", Plane::Definition, |name| {
            json!({"pipeline": declaration(name)})
        }),
        ("EnableStageTransition", Plane::Definition, |name| {
            json!({
                "pipelineName": name,
                "stageName": "Source",
                "transitionType": "Inbound",
            })
        }),
        ("DisableStageTransition", Plane::Definition, |name| {
            json!({
                "pipelineName": name,
                "stageName": "Source",
                "transitionType": "Inbound",
                "reason": "maintenance window",
            })
        }),
        ("PutActionRevision", Plane::Definition, |name| {
            json!({
                "pipelineName": name,
                "stageName": "Source",
                "actionName": "Src",
                "actionRevision": {
                    "revisionId": "rev-1",
                    "revisionChangeId": "change-1",
                    "created": 1_700_000_000,
                },
            })
        }),
        ("PutApprovalResult", Plane::Approval, |name| {
            json!({
                "pipelineName": name,
                "stageName": "Source",
                "actionName": "Src",
                "result": {"summary": "looks good", "status": "Approved"},
                "token": UNMINTED_APPROVAL_TOKEN,
            })
        }),
        ("GetPipelineExecution", Plane::ExecutionRead, |name| {
            json!({
                "pipelineName": name,
                "pipelineExecutionId": UNMINTED_EXECUTION_ID,
            })
        }),
        ("ListActionExecutions", Plane::ExecutionListing, |name| {
            json!({"pipelineName": name})
        }),
        ("ListPipelineExecutions", Plane::ExecutionListing, |name| {
            json!({"pipelineName": name})
        }),
        ("ListRuleExecutions", Plane::ExecutionListing, |name| {
            json!({"pipelineName": name})
        }),
        ("StartPipelineExecution", Plane::ExecutionControl, |name| {
            json!({"name": name})
        }),
        ("StopPipelineExecution", Plane::ExecutionControl, |name| {
            json!({
                "pipelineName": name,
                "pipelineExecutionId": UNMINTED_EXECUTION_ID,
            })
        }),
        ("RetryStageExecution", Plane::ExecutionControl, |name| {
            json!({
                "pipelineName": name,
                "stageName": "Source",
                "pipelineExecutionId": UNMINTED_EXECUTION_ID,
                "retryMode": "FAILED_ACTIONS",
            })
        }),
        ("RollbackStage", Plane::StageRollback, |name| {
            json!({
                "pipelineName": name,
                "stageName": "Source",
                "targetPipelineExecutionId": UNMINTED_EXECUTION_ID,
            })
        }),
    ]
}

/// A declaration under `name` that every structural rule accepts, for the row that has
/// to send a whole pipeline.
fn declaration(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), build_stage("Build", 1)],
    })
}

/// The `(status, code, message)` of an answer: the `__type` is `None` for a success, and
/// the awsJson `__type` may be namespace-prefixed, so only the shape name is kept.
async fn answer(
    emulator: &Emulator,
    operation: &str,
    body: &Value,
) -> (u16, Option<String>, String) {
    let (status, response) = emulator.call(operation, body).await;
    let code = response
        .get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit('#').next().unwrap_or_default().to_string());
    let message = response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// The absent names every row is driven over: the rule generalises over the NAME, so the
/// cases differ in alphabet and in length rather than only in spelling. The punctuated
/// name carries every character besides the alphanumerics that `PipelineName`'s pattern
/// allows, and the shortest and longest are the bounds of its `@length(1, 100)`
/// (`smithy_query(query="shape", shape="PipelineName")`).
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain absent name", "ghost-pipeline".to_string()),
        ("a punctuated absent name", "a.b@c-d_e".to_string()),
        ("the shortest absent name", "z".to_string()),
        ("the longest absent name", "q".repeat(100)),
    ]
}

#[tokio::test]
async fn an_absent_pipeline_is_refused_with_its_own_planes_wording() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration(STORED)}))
        .await;

    for (name_case, name) in absent_names() {
        for (operation, plane, build) in rows() {
            let (status, code, message) = answer(&emulator, operation, &build(&name)).await;
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("PipelineNotFoundException")),
                "case {name_case:?} via {operation}: the status and the code are shared by every \
                 plane, got {status} {code:?} {message:?}"
            );
            assert_eq!(
                message,
                expected_message(plane, &name),
                "case {name_case:?} via {operation}: expected the {plane:?} plane's sentence, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_plane_words_the_refusal_differently_from_the_others() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration(STORED)}))
        .await;

    // One name, every plane: the sentences a client reads must tell the planes apart, so
    // no two planes may answer the same text — and the operations WITHIN a plane may not
    // disagree about it.
    let name = "ghost-pipeline";
    let mut by_plane: BTreeMap<Plane, BTreeSet<String>> = BTreeMap::new();
    for (operation, plane, build) in rows() {
        let (status, code, message) = answer(&emulator, operation, &build(name)).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("PipelineNotFoundException")),
            "case {operation:?}: expected the modelled not-found refusal, got {status} {code:?} \
             {message:?}"
        );
        by_plane.entry(plane).or_default().insert(message);
    }

    for (plane, messages) in &by_plane {
        assert_eq!(
            messages.len(),
            1,
            "case {plane:?}: the operations of one plane word the refusal identically, got \
             {messages:?}"
        );
    }

    let distinct: BTreeSet<&String> = by_plane.values().flatten().collect();
    assert_eq!(
        distinct.len(),
        by_plane.len(),
        "each plane carries its own sentence, got {by_plane:?}"
    );
}

#[tokio::test]
async fn a_pipeline_the_store_holds_is_never_refused_as_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration(STORED)}))
        .await;

    // The same table over the name the store DOES hold. Several rows refuse it for
    // something else (an execution id nothing minted, an action that is no approval) and
    // some succeed; what may never happen is this error.
    for (operation, plane, build) in rows() {
        let (status, code, message) = answer(&emulator, operation, &build(STORED)).await;
        assert_ne!(
            code.as_deref(),
            Some("PipelineNotFoundException"),
            "case {operation:?} ({plane:?}): a stored name resolves, got {status} {code:?} \
             {message:?}"
        );
    }
}
