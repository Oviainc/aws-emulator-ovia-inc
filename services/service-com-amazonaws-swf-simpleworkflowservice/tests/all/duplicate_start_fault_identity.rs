// SPDX-License-Identifier: CC0-1.0

//! What `StartWorkflowExecution` answers when the `workflowId` it asks for is already
//! held by an open run, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: the `WorkflowExecutionAlreadyStartedFault` a
//! colliding start is refused with IDENTIFIES the incumbent — besides the code it
//! carries a `workflowExecution` holding the requested `workflowId` and the `runId` of
//! the run that still holds it, taken from the service's own state — so the caller can
//! address that run without a visibility query. The identity is the one the first start
//! returned, and it resolves: describing it reads the open run back.
//!
//! Rows vary what the rule generalises over: the shape and size of the `workflowId`
//! (plain, punctuated, non-ASCII, at the modelled 256-character maximum), how the
//! colliding start addresses its type and defaults, which domain the collision happens
//! in, and how many times the same id is re-started. Controls keep the fault from being
//! a constant: the same id in another domain starts cleanly, and once the incumbent is
//! terminated the id starts again with a NEW runId.

use serde_json::{json, Value};

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, seeded, string_at, Emulator,
};

/// The workflow type every start below uses, registered with every default so a start
/// needs no overrides.
const TYPE_NAME: &str = "orders";
const TYPE_VERSION: &str = "1.0";

/// `WorkflowId`'s modelled maximum length (string, @length 1..256).
const MAX_WORKFLOW_ID_LEN: usize = 256;

/// One `workflowId` a collision is provoked on.
struct IdCase {
    case: &'static str,
    workflow_id: String,
}

fn id_cases() -> Vec<IdCase> {
    vec![
        IdCase {
            case: "plain id",
            workflow_id: "wf-1".to_string(),
        },
        IdCase {
            case: "punctuated id",
            workflow_id: "orders-2026-01-01#42".to_string(),
        },
        IdCase {
            case: "non-ASCII id",
            workflow_id: "注文-ワークフロー".to_string(),
        },
        IdCase {
            case: "id at the modelled maximum",
            workflow_id: "w".repeat(MAX_WORKFLOW_ID_LEN),
        },
    ]
}

/// Start `workflow_id` in `domain`, requiring a 200, and return the minted `runId`.
async fn start(emulator: &Emulator, case: &str, domain: &str, workflow_id: &str) -> String {
    let answer = emulator
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
            }),
        )
        .await;
    string_at(&answer, "/runId")
}

/// Start `workflow_id` in `domain` expecting the already-started fault, and return the
/// `workflowExecution` the fault carries.
async fn refused_start(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    workflow_id: &str,
    extra: Value,
) -> Value {
    let mut body = json!({
        "domain": domain,
        "workflowId": workflow_id,
        "workflowType": { "name": TYPE_NAME, "version": TYPE_VERSION },
    });
    crate::workflow_execution_lifecycle::merge(&mut body, extra);
    let (status, refused) = emulator.call("StartWorkflowExecution", &body).await;
    let code = error_code(&refused);
    assert_eq!(
        (status, code.as_str()),
        (400, "WorkflowExecutionAlreadyStartedFault"),
        "case {case:?}: a start colliding with an open run is 400 \
         WorkflowExecutionAlreadyStartedFault, got {status} {code} with {refused:?}"
    );
    refused
        .get("workflowExecution")
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the fault identifies the run that holds the id, \
                 got {refused:?}"
            )
        })
}

#[tokio::test]
async fn a_colliding_start_is_refused_with_the_open_run_it_collided_with() {
    let emulator = Emulator::new();
    let domain = "collisions";
    let elsewhere = "collisions-elsewhere";
    for name in [domain, elsewhere] {
        seeded(
            &emulator,
            name,
            name,
            (TYPE_NAME, TYPE_VERSION),
            full_defaults("tasks", "TERMINATE"),
        )
        .await;
    }

    for case in id_cases() {
        let workflow_id = case.workflow_id.as_str();
        let open_run_id = start(&emulator, case.case, domain, workflow_id).await;

        // The same id, asked for in several ways: the identity reported is the stored
        // run's, never anything taken from the colliding request.
        let variants = [
            ("bare retry", json!({})),
            (
                "retry overriding the defaults",
                json!({
                    "taskList": { "name": "other-tasks" },
                    "executionStartToCloseTimeout": "120",
                    "taskStartToCloseTimeout": "30",
                    "childPolicy": "ABANDON",
                    "tagList": ["retry"],
                }),
            ),
            ("retry carrying input", json!({ "input": "{\"n\":1}" })),
        ];
        for (variant, extra) in variants {
            let label = format!("{} / {variant}", case.case);
            let identity = refused_start(&emulator, &label, domain, workflow_id, extra).await;
            assert_eq!(
                identity,
                json!({ "workflowId": workflow_id, "runId": open_run_id }),
                "case {label:?}: the fault carries the requested workflowId and the \
                 open run's runId, got {identity:?}"
            );
        }

        // The identity the fault handed back resolves: it addresses the OPEN run.
        let described = emulator
            .ok(
                case.case,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": workflow_id, "runId": open_run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {:?}: the run the fault named is the open one, got {described:?}",
            case.case
        );

        // Control: the collision is per domain, so the same id starts cleanly in another
        // domain, with a runId of its own.
        let other_run_id = start(&emulator, case.case, elsewhere, workflow_id).await;
        assert_ne!(
            other_run_id, open_run_id,
            "case {:?}: a run started in another domain is a different run",
            case.case
        );

        // Control: once the incumbent is closed the id is free again, and the next start
        // gets a NEW runId — so the reported identity tracks state rather than the id.
        emulator
            .ok(
                case.case,
                "TerminateWorkflowExecution",
                &json!({ "domain": domain, "workflowId": workflow_id }),
            )
            .await;
        let restarted = start(&emulator, case.case, domain, workflow_id).await;
        assert_ne!(
            restarted, open_run_id,
            "case {:?}: a restart after a terminate mints a fresh runId",
            case.case
        );
        let identity = refused_start(
            &emulator,
            &format!("{} / after restart", case.case),
            domain,
            workflow_id,
            json!({}),
        )
        .await;
        assert_eq!(
            identity,
            json!({ "workflowId": workflow_id, "runId": restarted }),
            "case {:?}: the fault names the run that holds the id NOW, got {identity:?}",
            case.case
        );
    }
}
