// SPDX-License-Identifier: CC0-1.0

//! WHO answered a manual approval, as the two readers of an action's run report it.
//!
//! The rule under test: `ActionExecution$lastUpdatedBy` and
//! `ActionExecutionDetail$updatedBy` are "the ARN of the user who changed the pipeline
//! execution details", so an action a CALLER's own request moved reports that caller's
//! identity — the principal `sts:GetCallerIdentity` answers the same request with — and
//! an action only the execution plane moved reports none. The identity is the same
//! whichever way the approval was answered, and whichever reader is asked.
//!
//! The harness comes from `tests/all/execution_engine.rs`, so the pipeline runs its real
//! actions through the emulated S3 service and every assertion goes through the router.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, creation_run, declaration, source_stage, wait_for_terminal, Emulator, WAIT,
};

/// The bucket the source object lives in.
const SOURCE_BUCKET: &str = "reviewer-source";

/// The source object's key.
const SOURCE_KEY: &str = "source.zip";

/// The stage the approval action waits in, and the action's name.
const GATE_STAGE: &str = "Gate";
/// See [`GATE_STAGE`].
const GATE_ACTION: &str = "Approve";

/// The ARN of the principal every request of this suite is signed for: the IAM user
/// whose long-term key this emulator publishes, which is the identity
/// `sts:GetCallerIdentity` answers with (`service_common::caller_principal`).
const CALLER_ARN: &str = "arn:aws:iam::000000000000:user/emulator";

/// The approval action's `latestExecution`, as `GetPipelineState` reports it.
async fn approval_state(emulator: &Emulator, pipeline: &str) -> Value {
    let state = emulator
        .ok("GetPipelineState", &json!({"name": pipeline}))
        .await;
    state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stage| stage["stageName"] == json!(GATE_STAGE))
        .filter_map(|stage| stage["actionStates"].as_array())
        .flatten()
        .find(|action| action["actionName"] == json!(GATE_ACTION))
        .map(|action| action["latestExecution"].clone())
        .unwrap_or(Value::Null)
}

/// The approval action's run, as `ListActionExecutions` reports it.
async fn approval_detail(emulator: &Emulator, pipeline: &str, execution: &str) -> Value {
    let page = emulator
        .ok(
            "ListActionExecutions",
            &json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": execution}}),
        )
        .await;
    page["actionExecutionDetails"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|detail| detail["actionName"] == json!(GATE_ACTION))
        .cloned()
        .unwrap_or_else(|| panic!("the approval action's run is reported, got {page}"))
}

/// Poll until the approval action publishes the token it waits on.
async fn approval_token(emulator: &Emulator, pipeline: &str, case: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let latest = approval_state(emulator, pipeline).await;
        if let Some(token) = latest["token"].as_str() {
            return token.to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the approval action never published a token, last {latest}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// RULE: the identity that answered an approval is reported for it, whichever
/// `ApprovalStatus` the answer carried and whichever reader is asked — while an action
/// the execution plane moved on its own reports none.
#[tokio::test]
async fn an_approval_records_the_identity_that_answered_it() {
    // (case, the ApprovalStatus answered with, the status the action then carries, the
    //  status the execution then reaches)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("approved", "Approved", "Succeeded", "Succeeded"),
        ("rejected", "Rejected", "Failed", "Failed"),
    ];

    for (case, answer_status, action_status, execution_status) in rows {
        let emulator = Emulator::new();
        let pipeline = "reviewer-pipe";
        emulator.bucket(&artifact_bucket(pipeline)).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator
            .put_object(SOURCE_BUCKET, SOURCE_KEY, b"reviewed bytes")
            .await;
        emulator
            .ok(
                "CreatePipeline",
                &declaration(
                    pipeline,
                    json!([
                        source_stage(SOURCE_BUCKET, SOURCE_KEY, "SourceArtifact"),
                        {
                            "name": GATE_STAGE,
                            "actions": [{
                                "name": GATE_ACTION,
                                "actionTypeId": {
                                    "category": "Approval",
                                    "owner": "AWS",
                                    "provider": "Manual",
                                    "version": "1",
                                },
                            }],
                        },
                    ]),
                    None,
                ),
            )
            .await;
        let execution = creation_run(&emulator, pipeline).await;
        let token = approval_token(&emulator, pipeline, case).await;

        // Before anyone answers it, the waiting approval names nobody: the execution
        // plane reached the action, and that is not a caller changing it.
        let waiting = approval_state(&emulator, pipeline).await;
        assert!(
            waiting["lastUpdatedBy"].is_null(),
            "case {case:?}: an approval nobody has answered names nobody, got {waiting}"
        );

        emulator
            .ok(
                "PutApprovalResult",
                &json!({
                    "pipelineName": pipeline,
                    "stageName": GATE_STAGE,
                    "actionName": GATE_ACTION,
                    "result": {"summary": "the reviewer's note", "status": answer_status},
                    "token": token,
                }),
            )
            .await;

        let answered = approval_state(&emulator, pipeline).await;
        assert_eq!(
            answered["lastUpdatedBy"], json!(CALLER_ARN),
            "case {case:?}: the approval names the identity that answered it, \
             got {answered}"
        );
        assert_eq!(
            answered["status"], json!(action_status),
            "case {case:?}: unexpected action status, got {answered}"
        );

        // The same fact through the other reader, which reports it as `updatedBy`.
        let detail = approval_detail(&emulator, pipeline, &execution).await;
        assert_eq!(
            detail["updatedBy"], json!(CALLER_ARN),
            "case {case:?}: the action execution names the identity that changed it, \
             got {detail}"
        );

        // An action of the same execution that no caller touched still names nobody.
        let page = emulator
            .ok(
                "ListActionExecutions",
                &json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": execution}}),
            )
            .await;
        let source = page["actionExecutionDetails"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|detail| detail["actionName"] == json!("Src"))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the source action is reported, got {page}"));
        assert!(
            source["updatedBy"].is_null(),
            "case {case:?}: an action only the execution plane moved names nobody, \
             got {source}"
        );

        let settled = wait_for_terminal(&emulator, pipeline, &execution).await;
        assert_eq!(
            settled["pipelineExecution"]["status"], json!(execution_status),
            "case {case:?}: unexpected execution status, got {settled}"
        );
    }
}
