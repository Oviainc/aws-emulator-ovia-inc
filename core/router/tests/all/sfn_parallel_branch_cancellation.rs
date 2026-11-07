// SPDX-License-Identifier: CC0-1.0

//! What a `Parallel` state's SIBLING branches do once one branch has failed — driven
//! across Step Functions, IAM and Amazon SQS, mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them.
//!
//! RULE: "If any branch fails, the entire Parallel state is considered to have failed
//! and all the branches are stopped." The state fails at the instant that branch did,
//! and from that instant on a sibling branch runs nothing: a state due after it is not
//! entered, and the WORK of a state the failure caught in flight — the integration call
//! it was about to make — is not made. Work a sibling had already FINISHED before that
//! instant stands, because the failure cannot reach back for it.
//!
//! Two crates have to agree for that to hold — Step Functions decides whether the call
//! is made, Amazon SQS would hold the message it made — so the suite lives here rather
//! than in either of them, and it reads the queue through SQS's own wire: whether the
//! message is there is the whole question.
//!
//! The rows differ along what the rule generalises over: whether the sibling's send is
//! due at the very instant the other branch failed or later, and whether it had already
//! completed before it.
//!
//! What the rows do NOT cover, and why: the walk reaches a `Parallel`'s branches in
//! DECLARATION order, so the instant a branch failed at is known from that branch on —
//! a sibling declared BEFORE the failing one has already run when the failure is seen,
//! and a send of ITS due after that instant is still made. Covering that half needs the
//! branches to advance together in the run's own time rather than one after another,
//! which is a change to how the interpreter schedules work rather than to this rule.
//!
//! The four-service wiring, the role, the queue and the execution reader are the ones
//! `sfn_task_target_authorization` owns; `tests/all.rs` is one crate root, so they are
//! reached through `crate::sfn_task_target_authorization`.

use serde_json::{json, Value};

use crate::sfn_task_target_authorization::{
    allow, emulator, logs_only, queue, ran, role, sfn, string, waiting,
};

/// One row: the case, the two branches of the `Parallel` state, and whether the send
/// the sending branch declares must have happened.
struct Row {
    case: &'static str,
    branches: Value,
    delivered: usize,
}

/// A `Parallel` state whose branches are `branches`.
fn definition(branches: &Value) -> String {
    json!({
        "StartAt": "Both",
        "States": {
            "Both": {
                "Type": "Parallel",
                "Branches": branches,
                "End": true,
            }
        }
    })
    .to_string()
}

/// A branch that fails straight away, with the error it reports.
fn fails_now() -> Value {
    json!({
        "StartAt": "Boom",
        "States": { "Boom": { "Type": "Fail", "Error": "Boom", "Cause": "on purpose" } }
    })
}

/// A branch that waits `seconds` and then fails, so its failure comes due later than
/// the work of a sibling that does not wait.
fn fails_later(seconds: u64) -> Value {
    json!({
        "StartAt": "Hold",
        "States": {
            "Hold": { "Type": "Wait", "Seconds": seconds, "Next": "Boom" },
            "Boom": { "Type": "Fail", "Error": "Boom", "Cause": "on purpose" }
        }
    })
}

/// A branch that sends one message to `url`, after waiting `seconds` first when it is
/// not zero — which is what puts its send later in the run than a sibling's failure.
fn sends(url: &str, seconds: u64) -> Value {
    let send = json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::sqs:sendMessage",
        "Parameters": { "QueueUrl": url, "MessageBody": "should-not-arrive-if-sibling-failed" },
        "End": true,
    });
    match seconds {
        0 => json!({ "StartAt": "Send", "States": { "Send": send } }),
        seconds => json!({
            "StartAt": "Hold",
            "States": {
                "Hold": { "Type": "Wait", "Seconds": seconds, "Next": "Send" },
                "Send": send
            }
        }),
    }
}

/// A failed branch stops its siblings: a send the failure caught before it completed
/// does not reach the queue, and one that had already completed stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_branch_stops_the_send_of_its_sibling() {
    let router = emulator();
    let (queue_url, queue_arn) = queue(&router, "parallel-sibling").await;
    let role_arn = role(
        &router,
        "ParallelSiblingRole",
        &json!({
            "Version": "2012-10-17",
            "Statement": [logs_only(), allow("sqs:SendMessage", &queue_arn)],
        }),
    )
    .await;

    let rows = [
        Row {
            case: "a send due at the very instant the other branch failed",
            branches: json!([fails_now(), sends(&queue_url, 0)]),
            delivered: 0,
        },
        Row {
            case: "a send due after the instant the other branch failed",
            branches: json!([fails_now(), sends(&queue_url, 30)]),
            delivered: 0,
        },
        Row {
            // The failure cannot reach back for work that had already finished: this
            // send completed two seconds before the other branch failed, so the
            // message is on the queue exactly as it would be on the real service.
            case: "a send that had already completed before the other branch failed",
            branches: json!([fails_later(2), sends(&queue_url, 0)]),
            delivered: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let created = sfn(
            &router,
            "CreateStateMachine",
            &json!({
                "name": format!("parallel-sibling-{index}"),
                "definition": definition(&row.branches),
                "roleArn": role_arn,
            }),
        )
        .await;
        let machine_arn = string(&created, "stateMachineArn");
        let (_execution_arn, described) = ran(&router, &machine_arn, case).await;
        // The state failed with the branch's own error whatever became of the sibling.
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("Boom"),
            "case {case:?}: expected the failing branch's own error, got {described:?}"
        );
        let delivered = waiting(&router, &queue_url).await;
        assert_eq!(
            delivered, row.delivered,
            "case {case:?}: expected {} message(s) on the queue, got {delivered}",
            row.delivered
        );
    }
}
