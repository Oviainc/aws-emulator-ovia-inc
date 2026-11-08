// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `StopBuildBatch`, driven over the wire the router serves
//! (`POST /` with `X-Amz-Target: CodeBuild_20161006.StopBuildBatch` on awsJson1_1)
//! through the harness in `project_build_lifecycle`.
//!
//! RULE under test: `StopBuildBatch` ends a batch build that is still running — the
//! record it writes reports `STOPPED`, is complete and carries the instant it ended,
//! and that record is what every later read reports — while a batch the clock already
//! finished keeps the outcome it reached, and an id naming no batch build is refused
//! with the modelled `ResourceNotFoundException`. A stopped batch is complete, so it
//! deletes cleanly.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{
    batch_enabled, call, create_project, emulator, error_type, PAST_THE_SETTLE_SPAN, REGION,
};

/// Start one batch build of `project` and hand back its id.
async fn start_build_batch(router: &MultiServiceRouter, project: &str) -> String {
    let (status, body) = call(router, "StartBuildBatch", json!({"projectName": project})).await;
    assert_eq!(
        status, 200,
        "StartBuildBatch {project:?}: expected 200, got {status} with {body}"
    );
    body["buildBatch"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("StartBuildBatch {project:?} must report an id, got {body}"))
        .to_string()
}

/// The batch build `id` reads as, through `BatchGetBuildBatches`.
async fn read_back(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "BatchGetBuildBatches", json!({"ids": [id]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuildBatches {id}: expected 200, got {status} with {body}"
    );
    body["buildBatches"][0].clone()
}

/// Which batch build the stop call names, and so what it must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A batch build still inside its settle span: the stop ends it.
    Running,
    /// The same batch build stopped twice — the second call is the one asserted.
    Repeated,
    /// A batch build whose clock-owed transition has already fallen due: it keeps the
    /// outcome it reached.
    AlreadyFinished,
    /// A well-formed id naming no batch build at all.
    Unknown,
}

/// RULE: stopping a running batch build moves it to `STOPPED` and completes it, in
/// the store every later read is computed from; one that has already finished is left
/// with the outcome it reached; an id naming no batch build is refused with the
/// modelled not-found error naming that id.
#[tokio::test]
async fn stop_build_batch_ends_a_running_batch_and_leaves_a_finished_one_alone() {
    let cases: &[(Target, &str)] = &[
        (Target::Running, "stop-batch-running"),
        (Target::Repeated, "stop-batch-repeat"),
        (Target::AlreadyFinished, "stop-batch-finished"),
        (Target::Unknown, "stop-batch-unknown"),
    ];

    for (target, project) in cases {
        // A fresh emulator per row: a row's state must not depend on the one before.
        let router = emulator();
        create_project(&router, project, batch_enabled()).await;
        let started = start_build_batch(&router, project).await;
        // A second batch build, so every row also proves the one the call did not
        // name is untouched by it.
        let untouched = start_build_batch(&router, project).await;

        if *target == Target::AlreadyFinished {
            tokio::time::sleep(PAST_THE_SETTLE_SPAN).await;
        }

        let requested = match target {
            Target::Unknown => format!("{project}:00000000-0000-8000-8000-000000000001"),
            _ => started.clone(),
        };

        // The first of the two stops, for the row that asserts the second.
        let first_answer = if *target == Target::Repeated {
            let (status, body) =
                call(&router, "StopBuildBatch", json!({"id": requested.clone()})).await;
            assert_eq!(
                status, 200,
                "case {target:?}: the first stop: expected 200, got {status} with {body}"
            );
            Some(body)
        } else {
            None
        };

        let (status, body) = call(&router, "StopBuildBatch", json!({"id": requested.clone()})).await;

        match target {
            Target::Running | Target::Repeated => {
                assert_eq!(
                    status, 200,
                    "case {target:?}: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body["buildBatch"]["buildBatchStatus"],
                    Value::String("STOPPED".to_string()),
                    "case {target:?}: a stopped batch build reports STOPPED, got {body}"
                );
                assert_eq!(
                    body["buildBatch"]["complete"],
                    Value::Bool(true),
                    "case {target:?}: a stopped batch build is complete, got {body}"
                );
                assert!(
                    !body["buildBatch"]["endTime"].is_null(),
                    "case {target:?}: a stopped batch build carries the instant it ended, \
                     got {body}"
                );
                assert_eq!(
                    body["buildBatch"]["currentPhase"],
                    Value::String("STOPPED".to_string()),
                    "case {target:?}: a stopped batch build sits in the STOPPED phase, \
                     got {body}"
                );
                if let Some(first) = &first_answer {
                    assert_eq!(
                        body["buildBatch"], first["buildBatch"],
                        "case {target:?}: stopping an already stopped batch build changes \
                         nothing, got {body}"
                    );
                }
            }
            Target::AlreadyFinished => {
                assert_eq!(
                    status, 200,
                    "case {target:?}: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    body["buildBatch"]["complete"],
                    Value::Bool(true),
                    "case {target:?}: the batch build was already complete, got {body}"
                );
                assert_ne!(
                    body["buildBatch"]["buildBatchStatus"],
                    Value::String("STOPPED".to_string()),
                    "case {target:?}: a batch build that already finished keeps the \
                     outcome it reached, got {body}"
                );
            }
            Target::Unknown => {
                assert_eq!(
                    status, 400,
                    "case {target:?}: expected 400, got {status} with {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "ResourceNotFoundException",
                    "case {target:?}: expected ResourceNotFoundException, got {body}"
                );
                assert_eq!(
                    body["message"],
                    Value::String(format!("BuildBatch {requested} does not exist")),
                    "case {target:?}: the refusal names the id the request gave, got {body}"
                );
            }
        }

        // What a later read reports is the record the stop wrote, not a per-call answer.
        let read = read_back(&router, &started).await;
        if matches!(target, Target::Running | Target::Repeated) {
            assert_eq!(
                read["buildBatchStatus"],
                Value::String("STOPPED".to_string()),
                "case {target:?}: the stored batch build reads as STOPPED, got {read}"
            );
            assert_eq!(
                read["complete"],
                Value::Bool(true),
                "case {target:?}: the stored batch build reads as complete, got {read}"
            );
        }
        let other = read_back(&router, &untouched).await;
        assert_eq!(
            other["id"],
            Value::String(untouched.clone()),
            "case {target:?}: the batch build the call did not name is still readable, \
             got {other}"
        );
        assert_ne!(
            other["buildBatchStatus"],
            Value::String("STOPPED".to_string()),
            "case {target:?}: the batch build the call did not name was not stopped, \
             got {other}"
        );
    }
}

/// RULE: a batch build is deletable once it has finished, and a stop is one way it
/// finishes — so the delete that is refused while the batch runs succeeds right after
/// `StopBuildBatch`, with no wait for any clock-owed transition.
#[tokio::test]
async fn a_stopped_batch_build_deletes_cleanly() {
    let router = emulator();
    create_project(&router, "stop-then-delete", batch_enabled()).await;
    let id = start_build_batch(&router, "stop-then-delete").await;

    // While it runs, the delete is refused — the state the stop below changes.
    let (status, refused) = call(&router, "DeleteBuildBatch", json!({"id": id.clone()})).await;
    assert_eq!(
        status, 400,
        "DeleteBuildBatch while the batch runs: expected 400, got {status} with {refused}"
    );

    let (status, stopped) = call(&router, "StopBuildBatch", json!({"id": id.clone()})).await;
    assert_eq!(
        status, 200,
        "StopBuildBatch: expected 200, got {status} with {stopped}"
    );

    let (status, deleted) = call(&router, "DeleteBuildBatch", json!({"id": id.clone()})).await;
    assert_eq!(
        status, 200,
        "DeleteBuildBatch after the stop: expected 200, got {status} with {deleted}"
    );

    let (status, after) = call(&router, "BatchGetBuildBatches", json!({"ids": [id.clone()]})).await;
    assert_eq!(
        status, 200,
        "BatchGetBuildBatches after the delete: expected 200, got {status} with {after}"
    );
    assert_eq!(
        after["buildBatchesNotFound"],
        json!([state::build_batch_arn(REGION, &id)]),
        "the deleted batch build reads as not found by its ARN, got {after}"
    );
}
