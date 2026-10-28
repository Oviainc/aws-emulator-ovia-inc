// SPDX-License-Identifier: CC0-1.0

//! How CodePipeline WORDS the refusal of an approval that has already been answered,
//! end to end over the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.PutApprovalResult`).
//!
//! RULE: `PutApprovalResult` refuses a second response to an approval action that has
//! already received one with HTTP 400 and `ApprovalAlreadyCompletedException`, whose
//! message is INVARIANT text — the same sentence pair for every such refusal, with
//! nothing taken from the request or from state interpolated into it.
//!
//! The rows differ along everything the rule generalises over: the pipeline, stage and
//! action names (their alphabet and their length, up to the longest an identifier may
//! be), the decision the first call submitted (`Approved` and `Rejected`, so the action
//! settles once at `Succeeded` and once at `Failed`), the summary both calls carry
//! (including the empty and the longest `ApprovalSummary`), and whether the execution
//! had settled by the time the duplicate arrived. Every row must read back the SAME
//! message, and none of its own values may appear in it.
//!
//! The sibling suite `execution_engine.rs` pins the surrounding behaviour (the token is
//! published only once the action is reached, and the first response releases the
//! stage); this suite pins the wording and that the refused second call changes nothing.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_engine::{
    artifact_bucket, creation_run, source_stage, wait_for_terminal, Emulator, WAIT,
};
use crate::pipeline_lifecycle::ROLE_ARN;

/// The message `ApprovalAlreadyCompletedException` carries. Fixed text: the service
/// words this class of refusal the same way for every approval, so it is spelled out
/// here rather than assembled from a row's values.
const ALREADY_COMPLETED_MESSAGE: &str =
    "The approval request that you responded to is already complete or has expired. \
     Review the latest revisions in the pipeline, and then try again.";

/// The longest identifier `PipelineName` / `StageName` / `ActionName` accept
/// (`smithy_query(query="operation", operation="PutApprovalResult")` reports each as
/// `len 1..100, pattern ^[A-Za-z0-9.@\-_]+$`).
const MAX_NAME_LENGTH: usize = 100;

/// The longest `ApprovalSummary` a caller can send (`smithy_query(query="shape",
/// shape="ApprovalSummary")` reports `@length(min: 0, max: 512)`).
const MAX_SUMMARY_LENGTH: usize = 512;

/// One row of the table: the names the approval is addressed by, the decision and
/// summary the FIRST call submits, the summary and decision the duplicate submits, and
/// whether the duplicate waits for the execution to settle first.
struct Row {
    case: &'static str,
    pipeline: String,
    stage: String,
    action: String,
    first_status: &'static str,
    first_summary: String,
    second_status: &'static str,
    second_summary: String,
    settle_first: bool,
}

/// The source bucket every row reads its revision from.
const SOURCE_BUCKET: &str = "approval-source";

/// The declaration one row drives: an S3 source stage and a manual-approval stage.
fn approval_declaration(row: &Row, bucket: &str) -> Value {
    json!({
        "pipeline": {
            "name": row.pipeline,
            "roleArn": ROLE_ARN,
            "artifactStore": {"type": "S3", "location": bucket},
            "stages": [
                source_stage(SOURCE_BUCKET, "source.zip", "SourceArtifact"),
                {
                    "name": row.stage,
                    "actions": [{
                        "name": row.action,
                        "actionTypeId": {
                            "category": "Approval",
                            "owner": "AWS",
                            "provider": "Manual",
                            "version": "1",
                        },
                    }],
                },
            ],
        }
    })
}

/// Poll `GetPipelineState` until the named approval action publishes its token.
async fn approval_token(emulator: &Emulator, row: &Row) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        let state = emulator
            .ok("GetPipelineState", &json!({"name": row.pipeline}))
            .await;
        let token = state["stageStates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|stage| stage["stageName"] == json!(row.stage))
            .filter_map(|stage| stage["actionStates"].as_array())
            .flatten()
            .find(|action| action["actionName"] == json!(row.action))
            .and_then(|action| action["latestExecution"]["token"].as_str())
            .map(str::to_string);
        if let Some(token) = token {
            return token;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: the approval action never published a token; last state was {state}",
            row.case
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What `GetPipelineState` reports for the named approval action: its status and the
/// summary of the response it accepted.
async fn reported_action(emulator: &Emulator, row: &Row) -> Value {
    let state = emulator
        .ok("GetPipelineState", &json!({"name": row.pipeline}))
        .await;
    state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stage| stage["stageName"] == json!(row.stage))
        .filter_map(|stage| stage["actionStates"].as_array())
        .flatten()
        .find(|action| action["actionName"] == json!(row.action))
        .cloned()
        .unwrap_or_else(|| panic!("case {:?}: the approval action must be reported", row.case))
}

#[tokio::test]
async fn a_second_response_to_one_approval_is_refused_with_one_invariant_message() {
    let rows = vec![
        Row {
            case: "an approved action, plain names",
            pipeline: "appr-plain".to_string(),
            stage: "Gate".to_string(),
            action: "Approve".to_string(),
            first_status: "Approved",
            first_summary: "looks good".to_string(),
            second_status: "Approved",
            second_summary: "duplicate attempt".to_string(),
            settle_first: false,
        },
        Row {
            // Every character besides the alphanumerics that the three name patterns
            // allow, and a rejection — so the action settles at Failed rather than
            // Succeeded, and the refusal still reads the same.
            case: "a rejected action, punctuated names, empty summary",
            pipeline: "a.b@c-d_e".to_string(),
            stage: "Review_Gate.1".to_string(),
            action: "Sign.Off@2".to_string(),
            first_status: "Rejected",
            first_summary: "not yet".to_string(),
            second_status: "Rejected",
            second_summary: String::new(),
            settle_first: false,
        },
        Row {
            // The longest names and the longest summary, with the duplicate arriving
            // only after the execution has reached a terminal status.
            case: "the longest names and summary, after the execution settles",
            pipeline: "p".repeat(MAX_NAME_LENGTH),
            stage: "s".repeat(MAX_NAME_LENGTH),
            action: "a".repeat(MAX_NAME_LENGTH),
            first_status: "Approved",
            first_summary: "f".repeat(MAX_SUMMARY_LENGTH),
            second_status: "Rejected",
            second_summary: "s".repeat(MAX_SUMMARY_LENGTH),
            settle_first: true,
        },
    ];

    let mut messages: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let emulator = Emulator::new();
        let bucket = artifact_bucket("approval-messages");
        emulator.bucket(&bucket).await;
        emulator.bucket(SOURCE_BUCKET).await;
        emulator
            .put_object(SOURCE_BUCKET, "source.zip", b"approval bytes")
            .await;
        emulator
            .ok("CreatePipeline", &approval_declaration(row, &bucket))
            .await;
        // The execution under test is the one the pipeline's creation started: creating
        // a pipeline runs it once, and that run is what reaches the approval action.
        let execution = creation_run(&emulator, &row.pipeline).await;

        let token = approval_token(&emulator, row).await;
        let answered = emulator
            .ok(
                "PutApprovalResult",
                &json!({
                    "pipelineName": row.pipeline,
                    "stageName": row.stage,
                    "actionName": row.action,
                    "result": {"summary": row.first_summary, "status": row.first_status},
                    "token": token,
                }),
            )
            .await;
        assert!(
            answered["approvedAt"].is_number() || answered["approvedAt"].is_string(),
            "case {:?}: the accepted response reports approvedAt, got {answered}",
            row.case
        );
        if row.settle_first {
            wait_for_terminal(&emulator, &row.pipeline, &execution).await;
        }
        let before = reported_action(&emulator, row).await;

        // The duplicate: the SAME token, a different decision and summary.
        let (status, body) = emulator
            .cp(
                "PutApprovalResult",
                &json!({
                    "pipelineName": row.pipeline,
                    "stageName": row.stage,
                    "actionName": row.action,
                    "result": {"summary": row.second_summary, "status": row.second_status},
                    "token": token,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {body}",
            row.case
        );
        // The awsJson `__type` may carry the model's namespace, so only the shape name
        // is compared.
        let code = body["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected an error body, got {body}", row.case))
            .rsplit('#')
            .next()
            .unwrap_or_default();
        assert_eq!(
            code, "ApprovalAlreadyCompletedException",
            "case {:?}: expected the already-completed code, got {body}",
            row.case
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default();
        assert_eq!(
            message, ALREADY_COMPLETED_MESSAGE,
            "case {:?}: expected the invariant already-completed message, got {message:?}",
            row.case
        );
        for value in [
            row.pipeline.as_str(),
            row.stage.as_str(),
            row.action.as_str(),
            token.as_str(),
            row.first_summary.as_str(),
            row.second_summary.as_str(),
        ] {
            assert!(
                value.is_empty() || !message.contains(value),
                "case {:?}: the message must interpolate nothing, but {value:?} is in \
                 {message:?}",
                row.case
            );
        }
        messages.insert(message.to_string());

        // The refused call changed nothing: the action keeps the first response.
        let after = reported_action(&emulator, row).await;
        assert_eq!(
            after["latestExecution"]["summary"].as_str(),
            Some(row.first_summary.as_str()),
            "case {:?}: the action keeps the first response's summary, got {after}",
            row.case
        );
        let expected_status = if row.first_status == "Approved" {
            "Succeeded"
        } else {
            "Failed"
        };
        assert_eq!(
            after["latestExecution"]["status"].as_str(),
            Some(expected_status),
            "case {:?}: the action stays {expected_status}, got {after}",
            row.case
        );
        assert_eq!(
            after["latestExecution"]["token"],
            before["latestExecution"]["token"],
            "case {:?}: the refused call republishes no token, got {after}",
            row.case
        );
    }

    assert_eq!(
        messages.len(),
        1,
        "every row must read the SAME message, got {messages:?}"
    );
}
