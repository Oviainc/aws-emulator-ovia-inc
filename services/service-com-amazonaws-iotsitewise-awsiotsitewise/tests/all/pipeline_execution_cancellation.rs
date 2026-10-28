// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelPipelineExecution`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is about
//! the wire behaviour a real client sees.
//!
//! The rule these suites pin, over a SET of executions rather than one: a cancel
//! RECORDS the cancellation on a pipeline execution whose run has not reached a
//! terminal state and leaves it reporting the cancellation in flight and then
//! complete, as the clock makes each due, carrying the reason the request
//! supplied; a repeat cancel of such an execution records no second cancellation
//! and reports the state it stands in; an execution whose run reached a terminal
//! state cannot be cancelled and is the modelled conflict; and a workspace,
//! pipeline or execution this service does not hold is the modelled not-found
//! refusal.
//!
//! The HTTP harness (`call`, `router`, `error_code`, …) and the pipeline helpers
//! (`create_workspace`, `create_task`, `create_pipeline`, `start_plain`,
//! `describe_execution`, `Node`) are the ones the asset and pipeline-execution
//! suites already define; this suite reuses them rather than growing a second.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ACTIVATION_DELAY,
};

use crate::asset_lifecycle::{call, error_code, router, router_with_engine};
use crate::hierarchies_etags_and_refusals::error_message;
use crate::pipeline_executions::{
    create_pipeline, create_task, create_workspace, describe_execution, start_plain, Node,
};
use crate::pipeline_lifecycle::{expected_arn, pipeline_uri};

/// The workspace every suite below works in.
const WORKSPACE: &str = "cancel-workspace";

/// A second workspace, so a cancel can be shown to reach an execution only inside
/// the workspace it was started in.
const OTHER_WORKSPACE: &str = "cancel-workspace-two";

/// The pipeline every suite that needs only one works with.
const PIPELINE: &str = "nightly-rollup";

/// A second pipeline of the same workspace, for the same reason
/// [`OTHER_WORKSPACE`] exists.
const OTHER_PIPELINE: &str = "hourly-rollup";

/// The task every pipeline below runs on every compute node.
const TASK: &str = "transform";

/// Long enough that the transition a recorded cancellation owes has fallen due,
/// so an execution the suite cancelled reports the state it settles into.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// `POST /workspaces/{workspaceName}/pipelines/{pipelineName}/executions/{pipelineExecutionId}/cancel`,
/// as `OPERATIONS.md` reports the route.
fn cancel_uri(workspace: &str, pipeline: &str, execution_id: &str) -> String {
    format!("/workspaces/{workspace}/pipelines/{pipeline}/executions/{execution_id}/cancel")
}

/// `CancelPipelineExecution`, carrying `reason` only when one is supplied, as a
/// client that names no reason omits the member rather than sending null.
async fn cancel(
    router: &MultiServiceRouter,
    workspace: &str,
    pipeline: &str,
    execution_id: &str,
    reason: Option<&str>,
) -> (u16, Value) {
    let body = reason.map(|reason| serde_json::json!({ "reason": reason }));
    call(
        router,
        "POST",
        &cancel_uri(workspace, pipeline, execution_id),
        body,
    )
    .await
}

/// `CancelPipelineExecution` in [`WORKSPACE`] on [`PIPELINE`], asserting it was
/// accepted, and the `state` it reported.
async fn cancelled(router: &MultiServiceRouter, execution_id: &str, reason: Option<&str>) -> String {
    let (status, body) = cancel(router, WORKSPACE, PIPELINE, execution_id, reason).await;
    assert_eq!(
        status, 202,
        "cancel {execution_id}: expected 202, got {status} {body}"
    );
    body["state"]
        .as_str()
        .unwrap_or_else(|| panic!("cancel {execution_id}: no state in {body}"))
        .to_string()
}

/// `DescribePipelineExecution` in [`WORKSPACE`] on [`PIPELINE`], asserting it was
/// answered.
async fn described(router: &MultiServiceRouter, execution_id: &str) -> Value {
    let (status, body) = describe_execution(router, WORKSPACE, PIPELINE, execution_id, "").await;
    assert_eq!(
        status, 200,
        "describe {execution_id}: expected 200, got {status} {body}"
    );
    body
}

/// The workspace, task and pipeline every suite below starts executions of, with
/// `nodes` as the pipeline's `computations`.
async fn fixture(router: &MultiServiceRouter, pipeline: &str, nodes: &[Node]) {
    let (status, body) = create_pipeline(router, WORKSPACE, pipeline, &[], nodes).await;
    assert_eq!(
        status, 200,
        "create pipeline {pipeline}: got {status} {body}"
    );
}

/// The shared fixture: the workspace and the task the pipelines below run.
async fn workspace_and_task(router: &MultiServiceRouter) {
    create_workspace(router, WORKSPACE).await;
    create_task(router, WORKSPACE, TASK, &[], &[]).await;
}

/// Overwrite the `PipelineExecutionState` a stored execution's own RUN has
/// reached, so the suite can address an execution standing in EVERY state the
/// lifecycle admits — including the terminal ones no call of this emulator
/// reaches, because a compute node's container image runs nowhere here.
pub(crate) fn set_run_state(engine: &engine::InMemoryEngine, execution_id: &str, run_state: &str) {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    store
        .pipeline_executions
        .get_mut(execution_id)
        .unwrap_or_else(|| panic!("the store holds {execution_id}"))
        .run_state = run_state.to_string();
}

/// Put a recorded cancellation on a stored execution, or take it off, so a row
/// can stand in a state a cancel itself leaves behind.
pub(crate) fn set_cancel_requested_at(
    engine: &engine::InMemoryEngine,
    execution_id: &str,
    at: Option<SystemTime>,
) {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let mut store = state.lock();
    store
        .pipeline_executions
        .get_mut(execution_id)
        .unwrap_or_else(|| panic!("the store holds {execution_id}"))
        .cancel_requested_at = at;
}

/// The instant a cancel was recorded on a stored execution, as the store holds it
/// — the only way to observe that a request recorded nothing.
fn cancel_recorded_at(
    engine: &engine::InMemoryEngine,
    execution_id: &str,
) -> Option<SystemTime> {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    store
        .pipeline_executions
        .get(execution_id)
        .unwrap_or_else(|| panic!("the store holds {execution_id}"))
        .cancel_requested_at
}

/// One row of the cancel table: the case name, the pipeline it starts an
/// execution of, that pipeline's compute nodes, and the `reason` the cancel
/// carries.
struct CancelCase {
    label: &'static str,
    pipeline: &'static str,
    nodes: Vec<Node>,
    reason: Option<String>,
}

#[tokio::test]
async fn a_cancel_takes_an_execution_that_has_not_finished_through_cancelling_to_cancelled() {
    let router = router();
    workspace_and_task(&router).await;

    // The rows differ along what the rule generalises over: the `reason` the
    // cancel carries — absent, one character, a sentence and the longest string
    // the member admits (`CancelPipelineExecutionRequestReasonString` is
    // `length 1..1024`, from `smithy_query(query="shape",
    // shape="CancelPipelineExecutionRequestReasonString")`) — and the shape of
    // the pipeline the execution runs (no compute nodes, one, independent nodes,
    // a chain), so what a cancel does cannot depend on either.
    let cases: Vec<CancelCase> = vec![
        CancelCase {
            label: "no reason, no compute nodes",
            pipeline: "no-nodes",
            nodes: Vec::new(),
            reason: None,
        },
        CancelCase {
            label: "the shortest reason the member admits",
            pipeline: "one-node",
            nodes: vec![Node::new("extract", TASK)],
            reason: Some("x".to_string()),
        },
        CancelCase {
            label: "a sentence",
            pipeline: "two-independent-nodes",
            nodes: vec![Node::new("extract", TASK), Node::new("report", TASK)],
            reason: Some("the upstream feed was withdrawn".to_string()),
        },
        CancelCase {
            label: "the longest reason the member admits",
            pipeline: "a-chain",
            nodes: vec![
                Node::new("extract", TASK),
                Node::after("load", TASK, &["extract"]),
            ],
            reason: Some("r".repeat(1024)),
        },
    ];

    let mut cancelling: Vec<(&str, &str, String, Option<String>)> = Vec::new();
    for case in &cases {
        fixture(&router, case.pipeline, &case.nodes).await;
        let id = start_plain(&router, WORKSPACE, case.pipeline).await;

        // The cancel follows the start immediately, so the execution has not
        // finished when it arrives — which is exactly what an answer of
        // CANCELLING states, since an execution in a terminal state would have
        // been refused.
        let (status, body) = cancel(
            &router,
            WORKSPACE,
            case.pipeline,
            &id,
            case.reason.as_deref(),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?}: a cancellable execution is accepted, got {status} {body}",
            case.label
        );
        assert_eq!(
            body["state"].as_str(),
            Some("CANCELLING"),
            "case {:?}: a recorded cancel leaves the execution cancelling, got {body}",
            case.label
        );

        let (read_status, read) =
            describe_execution(&router, WORKSPACE, case.pipeline, &id, "").await;
        assert_eq!(
            read_status, 200,
            "case {:?}: the execution reads back, got {read_status} {read}",
            case.label
        );
        assert_eq!(
            read["status"]["state"].as_str(),
            Some("CANCELLING"),
            "case {:?}: a read agrees with the cancel's own answer, got {read}",
            case.label
        );
        assert!(
            read["endTime"].is_null(),
            "case {:?}: a cancellation in flight has ended nothing, got {read}",
            case.label
        );
        assert!(
            read["status"]["stateDetails"].is_null(),
            "case {:?}: the details are a terminated execution's, got {read}",
            case.label
        );
        cancelling.push((case.label, case.pipeline, id, case.reason.clone()));
    }

    // The span the clock owes the cancellation is NON-ZERO, which is what the
    // CANCELLING reads above observed; once it has elapsed every one of them is
    // cancelled, and stays cancelled however often it is read or cancelled again.
    tokio::time::sleep(settled()).await;
    for (label, pipeline, id, reason) in &cancelling {
        let (_, read) = describe_execution(&router, WORKSPACE, pipeline, id, "").await;
        assert_eq!(
            read["status"]["state"].as_str(),
            Some("CANCELLED"),
            "case {label:?}: the cancellation falls due, got {read}"
        );
        assert!(
            !read["endTime"].is_null(),
            "case {label:?}: a CANCELLED execution reached a state it stays in, got {read}"
        );
        // "For a cancelled execution, this is the reason you supplied when
        // calling CancelPipelineExecution" — and an execution cancelled without
        // one has nothing to report there.
        assert_eq!(
            read["status"]["stateDetails"]["message"].as_str(),
            reason.as_deref(),
            "case {label:?}: the details report the reason the cancel carried, got {read}"
        );
        // "Classification of the failure. Present when the execution failed": a
        // cancelled execution did not fail.
        assert!(
            read["status"]["stateDetails"]["code"].is_null(),
            "case {label:?}: a cancelled execution carries no failure code, got {read}"
        );

        // Idempotent: cancelling an already cancelled execution reports the state
        // it stands in and moves nothing — including the reason the first cancel
        // recorded, which a retry carrying another one does not overwrite.
        let (status, body) = cancel(
            &router,
            WORKSPACE,
            pipeline,
            id,
            Some("a reason the retry carries"),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {label:?}: a repeat cancel is accepted, got {status} {body}"
        );
        assert_eq!(
            body["state"].as_str(),
            Some("CANCELLED"),
            "case {label:?}: a repeat cancel reports the current state, got {body}"
        );
        assert_eq!(
            read,
            describe_execution(&router, WORKSPACE, pipeline, id, "")
                .await
                .1,
            "case {label:?}: a repeat cancel changes nothing about the execution"
        );
    }
}

/// One row of the state table: the state the stored execution stands in and what
/// a cancel of an execution in that state must answer.
#[derive(Debug)]
struct StateCase {
    /// The case name.
    label: &'static str,
    /// The state the execution's own run has reached.
    run_state: &'static str,
    /// How long ago a cancel was recorded on it, if one was.
    cancel_ago: Option<Duration>,
    /// The state a read of it reports before the cancel.
    state: &'static str,
    /// The state a cancel answers with, or `None` when the cancel is refused.
    cancelled_to: Option<&'static str>,
}

#[tokio::test]
async fn a_cancel_is_accepted_or_refused_by_the_state_the_execution_stands_in() {
    let (router, engine) = router_with_engine();
    workspace_and_task(&router).await;
    fixture(&router, PIPELINE, &[Node::new("extract", TASK)]).await;
    let now = SystemTime::now();

    // Rows span EVERY state of the lifecycle. Source of the catalogue:
    // `smithy_query(query="shape", shape="PipelineExecutionState")` —
    // NOT_STARTED | RUNNING | SUCCEEDED | FAILED | CANCELLING | CANCELLED: the
    // two an execution is still going through, the two terminal ones its run
    // earns, and the two a cancel itself leaves behind.
    let cases: Vec<StateCase> = vec![
        StateCase {
            label: "not started",
            run_state: "NOT_STARTED",
            cancel_ago: None,
            state: "NOT_STARTED",
            cancelled_to: Some("CANCELLING"),
        },
        StateCase {
            label: "running",
            run_state: "RUNNING",
            cancel_ago: None,
            state: "RUNNING",
            cancelled_to: Some("CANCELLING"),
        },
        StateCase {
            label: "succeeded",
            run_state: "SUCCEEDED",
            cancel_ago: None,
            state: "SUCCEEDED",
            cancelled_to: None,
        },
        StateCase {
            label: "failed",
            run_state: "FAILED",
            cancel_ago: None,
            state: "FAILED",
            cancelled_to: None,
        },
        StateCase {
            label: "cancelling",
            run_state: "RUNNING",
            cancel_ago: Some(Duration::ZERO),
            state: "CANCELLING",
            cancelled_to: Some("CANCELLING"),
        },
        StateCase {
            label: "cancelled",
            run_state: "RUNNING",
            cancel_ago: Some(2 * ACTIVATION_DELAY),
            state: "CANCELLED",
            cancelled_to: Some("CANCELLED"),
        },
    ];

    for case in &cases {
        let id = start_plain(&router, WORKSPACE, PIPELINE).await;
        set_run_state(&engine, &id, case.run_state);
        let recorded_at = case.cancel_ago.map(|ago| now - ago);
        set_cancel_requested_at(&engine, &id, recorded_at);

        let before = described(&router, &id).await;
        assert_eq!(
            before["status"]["state"].as_str(),
            Some(case.state),
            "case {:?}: the row stands in {}, got {before}",
            case.label,
            case.state
        );

        let (status, body) = cancel(&router, WORKSPACE, PIPELINE, &id, None).await;
        match case.cancelled_to {
            Some(expected) => {
                assert_eq!(
                    status, 202,
                    "case {:?}: a cancellable execution is accepted, got {status} {body}",
                    case.label
                );
                assert_eq!(
                    body["state"].as_str(),
                    Some(expected),
                    "case {:?}: expected {expected}, got {body}",
                    case.label
                );
                match recorded_at {
                    // An execution a cancel was already recorded on is not
                    // cancelled a second time: the recorded instant is exactly
                    // the one it already carried, so the state it is on its way
                    // to cannot be pushed back by a retry.
                    Some(_) => assert_eq!(
                        cancel_recorded_at(&engine, &id),
                        recorded_at,
                        "case {:?}: a repeat cancel records no second cancellation",
                        case.label
                    ),
                    None => assert!(
                        cancel_recorded_at(&engine, &id).is_some(),
                        "case {:?}: the cancellation is recorded on the stored execution",
                        case.label
                    ),
                }
            }
            None => {
                // A terminal state its run reached: the cancel cannot apply,
                // which this service answers as the modelled conflict rather
                // than a success that cancelled nothing.
                assert_eq!(
                    status, 409,
                    "case {:?}: a terminal execution cannot be cancelled, got {status} {body}",
                    case.label
                );
                assert_eq!(
                    error_code(&body),
                    "ConflictingOperationException",
                    "case {:?}: unexpected error code in {body}",
                    case.label
                );
                assert_eq!(
                    error_message(&body),
                    format!(
                        "The pipeline execution {id} is in the {} state; only an execution \
                         that has not reached a terminal state can be cancelled.",
                        case.state
                    ),
                    "case {:?}: unexpected message in {body}",
                    case.label
                );
                assert_eq!(
                    body["resourceId"].as_str(),
                    Some(id.as_str()),
                    "case {:?}: the conflict names the execution that conflicts, got {body}",
                    case.label
                );
                assert_eq!(
                    body["resourceArn"].as_str(),
                    Some(expected_arn(WORKSPACE, PIPELINE).as_str()),
                    "case {:?}: the conflict names the pipeline's ARN, got {body}",
                    case.label
                );
                assert_eq!(
                    cancel_recorded_at(&engine, &id),
                    None,
                    "case {:?}: a refused cancel records nothing",
                    case.label
                );
            }
        }

        // Whatever the answer was, a read of the execution agrees with it.
        let after = described(&router, &id).await;
        let expected = case.cancelled_to.unwrap_or(case.state);
        assert_eq!(
            after["status"]["state"].as_str(),
            Some(expected),
            "case {:?}: a read after the cancel reports {expected}, got {after}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_cancel_reaches_an_execution_only_inside_the_pipeline_it_was_started_in() {
    let router = router();
    workspace_and_task(&router).await;
    fixture(&router, PIPELINE, &[Node::new("extract", TASK)]).await;
    fixture(&router, OTHER_PIPELINE, &[Node::new("extract", TASK)]).await;
    create_workspace(&router, OTHER_WORKSPACE).await;
    let id = start_plain(&router, WORKSPACE, PIPELINE).await;

    // An id the store holds no execution under, still inside the shape every `ID`
    // member models (36 lowercase hex characters and hyphens, not the all-zero
    // one), so the refusal under test is the not-found one rather than the
    // boundary's validation.
    let unknown = "1eadbeef-0000-4000-8000-00000000abcd".to_string();

    // Rows span the ways a cancel addresses no execution of this service: a
    // workspace it does not hold (including a name differing from a held one only
    // in case, since a workspace is named exactly), a pipeline the named
    // workspace does not hold, an id no execution carries, the id of an execution
    // of ANOTHER pipeline of the same workspace, and a workspace that holds no
    // pipeline of that name at all.
    let cases: Vec<(&str, String, String, String, String)> = vec![
        (
            "a workspace this service does not hold",
            "cancel-workspace-nine".to_string(),
            PIPELINE.to_string(),
            id.clone(),
            "Workspace with ID cancel-workspace-nine not found".to_string(),
        ),
        (
            "a workspace whose case differs",
            WORKSPACE.to_uppercase(),
            PIPELINE.to_string(),
            id.clone(),
            format!("Workspace with ID {} not found", WORKSPACE.to_uppercase()),
        ),
        (
            "a pipeline this workspace does not hold",
            WORKSPACE.to_string(),
            "weekly-rollup".to_string(),
            id.clone(),
            "Pipeline with ID weekly-rollup not found".to_string(),
        ),
        (
            "an id no execution carries",
            WORKSPACE.to_string(),
            PIPELINE.to_string(),
            unknown.clone(),
            format!("Pipeline Execution with ID {unknown} not found"),
        ),
        (
            "an execution of another pipeline",
            WORKSPACE.to_string(),
            OTHER_PIPELINE.to_string(),
            id.clone(),
            format!("Pipeline Execution with ID {id} not found"),
        ),
        (
            "an execution of a pipeline of another workspace",
            OTHER_WORKSPACE.to_string(),
            PIPELINE.to_string(),
            id.clone(),
            format!("Pipeline with ID {PIPELINE} not found"),
        ),
    ];

    for (case, workspace, pipeline, addressed, expected) in &cases {
        let (status, body) = cancel(&router, workspace, pipeline, addressed, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            error_message(&body),
            expected.as_str(),
            "case {case:?}: unexpected message in {body}"
        );
    }

    // None of those refusals touched the execution they named: it is still the
    // one `StartPipelineExecution` recorded, in the state it was left in.
    let read = described(&router, &id).await;
    assert_eq!(
        read["status"]["state"].as_str(),
        Some("NOT_STARTED"),
        "a refused cancel cancels nothing, got {read}"
    );
    assert!(
        read["status"]["stateDetails"].is_null(),
        "a refused cancel records no reason, got {read}"
    );
}

#[tokio::test]
async fn a_pipeline_is_deletable_once_the_cancellation_of_its_execution_falls_due() {
    let router = router();
    workspace_and_task(&router).await;
    fixture(&router, PIPELINE, &[Node::new("extract", TASK)]).await;
    let id = start_plain(&router, WORKSPACE, PIPELINE).await;

    // An execution that has not finished blocks the delete, whether it is waiting
    // to run or has a cancellation in flight: both are states an execution has
    // not finished in.
    for stage in ["before the cancel", "while the cancellation is in flight"] {
        if stage == "while the cancellation is in flight" {
            let state = cancelled(&router, &id, Some("the pipeline is being retired")).await;
            assert_eq!(
                state, "CANCELLING",
                "case {stage:?}: the cancel is recorded, got {state}"
            );
        }
        let (status, body) = call(&router, "DELETE", &pipeline_uri(WORKSPACE, PIPELINE), None).await;
        assert_eq!(
            status, 409,
            "case {stage:?}: an unfinished execution blocks the delete, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictingOperationException",
            "case {stage:?}: unexpected error code in {body}"
        );
    }

    // Once the cancellation has fallen due the execution is in a state it stays
    // in, so nothing of the pipeline is still going and the delete is accepted.
    tokio::time::sleep(settled()).await;
    let read = described(&router, &id).await;
    assert_eq!(
        read["status"]["state"].as_str(),
        Some("CANCELLED"),
        "the cancellation falls due, got {read}"
    );
    let (status, body) = call(&router, "DELETE", &pipeline_uri(WORKSPACE, PIPELINE), None).await;
    assert_eq!(
        status, 200,
        "a pipeline whose only execution was cancelled is deletable, got {status} {body}"
    );
}
