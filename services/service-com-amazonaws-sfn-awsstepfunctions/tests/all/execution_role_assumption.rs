// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ASSUMING a state machine's execution role, driven over both
//! services' own wire protocols through the production `MultiServiceRouter` with Step
//! Functions and IAM mounted on ONE engine — the shape `core/router/src/lib.rs` mounts
//! them in — so the role a state machine names is the role IAM's own handlers hold.
//!
//! The rule under test: Step Functions acts as the state machine's `roleArn`, which it
//! has to ASSUME before it dispatches a `Task` — so `CreateStateMachine` accepts a role
//! the account does not hold (the control plane does not resolve it) while the DATA
//! plane refuses it: an execution whose role this account's IAM answers cannot be
//! assumed by either of this service's principals ends `FAILED` with
//! `States.TaskFailed`, carrying the service's own sentence for a role it cannot act
//! as, and the work the `Task` named never happens.
//!
//! The rows differ along what the rule generalises over: what IAM ANSWERS about the
//! role (no such role, a role whose trust policy names another service principal, one
//! that explicitly denies the assume, one that grants it to the global principal, one
//! that grants it to the REGIONAL principal, and one that grants it through a wildcard)
//! and whether the ARN names a role of this account at all — a role of ANOTHER account
//! is one this emulator's IAM cannot be asked about, which is evidence of nothing and
//! leaves the execution to run.
//!
//! What the role may then DO once it is assumed is the next question, and
//! `tests/all/task_execution_role.rs` holds it; the harness (`Chain`,
//! `trust_document`) is that suite's, and `tests/all.rs` is this binary's crate root,
//! so it is reached through `crate::task_execution_role`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::integration::TASK_FAILED_ERROR;
use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_FAILED;

use crate::execution_data_encryption::xml_element;
use crate::state_machine_lifecycle::{string, REGION};
use crate::task_execution_role::{trust_document, Chain};

/// A child definition that finishes at once: the work the parent's `Task` names, so a
/// row can show whether that work happened at all.
const CHILD: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// The integration the parent's `Task` names, which starts the child.
const FIRE_AND_FORGET: &str = "arn:aws:states:::states:startExecution";

/// A role ARN of this account that IAM holds nothing under.
const NOT_HELD: &str = "arn:aws:iam::000000000000:role/NotHeldHere";

/// A role ARN of ANOTHER account, whose IAM this emulator is not the one to ask.
const ANOTHER_ACCOUNT: &str = "arn:aws:iam::210987654321:role/Elsewhere";

/// A policy allowing every Step Functions action, so a row that gets past the assume is
/// not then refused on what the role may do.
fn allow_everything() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "states:*", "Resource": "*" }]
    })
}

/// A trust policy with one statement: `effect` for `principal` over `action`.
fn trust(effect: &str, principal: &str, action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": action,
        }]
    })
}

/// The parent definition: one `Task` that starts `child_arn`.
fn parent_definition(child_arn: &str) -> String {
    json!({
        "StartAt": "CallChild",
        "States": {
            "CallChild": {
                "Type": "Task",
                "Resource": FIRE_AND_FORGET,
                "Parameters": { "StateMachineArn": child_arn },
                "End": true,
            }
        }
    })
    .to_string()
}

/// A role carrying `trust` as its trust policy and [`allow_everything`] inline, by its
/// own ARN.
///
/// [`Chain::role`] creates one trusting `states.amazonaws.com`; the rows below need the
/// OTHER trust documents too, so the role is created here with the document the row
/// names.
async fn role_trusting(chain: &Chain, name: &str, trust: &Value) -> String {
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust.to_string()),
            ],
        )
        .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
    chain
        .iam(
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", allow_everything().to_string()),
            ],
        )
        .await;
    arn
}

/// How many executions `ListExecutions` reports for `machine_arn`.
async fn started_count(chain: &Chain, machine_arn: &str) -> usize {
    let listed = chain
        .sfn(
            "ListExecutions",
            &json!({ "stateMachineArn": machine_arn, "maxResults": 10 }),
        )
        .await;
    listed
        .get("executions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// One row: the role the parent runs as, and whether this service can assume it.
struct Row {
    case: &'static str,
    /// The role ARN, built once the chain exists.
    role_arn: String,
    /// Whether the execution must reach its `Task`'s work.
    assumable: bool,
}

/// A state machine whose role cannot be assumed is created, and its executions fail on
/// the data plane; one whose role can be assumed runs its `Task`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_execution_whose_role_cannot_be_assumed_fails_at_its_task() {
    let chain = Chain::new();
    let child_arn = chain.machine("assume-child", CHILD, NOT_HELD).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "a role this account's IAM holds nothing under",
            role_arn: NOT_HELD.to_string(),
            assumable: false,
        },
        Row {
            case: "a role whose trust policy names another service principal",
            role_arn: role_trusting(
                &chain,
                "LambdaTrustedRole",
                &trust("Allow", "lambda.amazonaws.com", "sts:AssumeRole"),
            )
            .await,
            assumable: false,
        },
        Row {
            case: "a role whose trust policy explicitly denies the assume",
            role_arn: role_trusting(
                &chain,
                "DeniedTrustRole",
                &trust("Deny", "states.amazonaws.com", "sts:AssumeRole"),
            )
            .await,
            assumable: false,
        },
        Row {
            case: "a role trusting this service",
            role_arn: role_trusting(&chain, "TrustedRole", &trust_document()).await,
            assumable: true,
        },
        Row {
            // The service assumes the role as either of its principals, so a role
            // trusting the Region's own is one it can act as.
            case: "a role trusting this service's regional principal",
            role_arn: role_trusting(
                &chain,
                "RegionalTrustRole",
                &trust(
                    "Allow",
                    &format!("states.{REGION}.amazonaws.com"),
                    "sts:AssumeRole",
                ),
            )
            .await,
            assumable: true,
        },
        Row {
            case: "a role trusting this service through a wildcard action",
            role_arn: role_trusting(
                &chain,
                "WildcardTrustRole",
                &trust("Allow", "states.amazonaws.com", "sts:*"),
            )
            .await,
            assumable: true,
        },
        Row {
            // Another account's roles are not this emulator's IAM to answer for, so
            // nothing is known and nothing is refused.
            case: "a role of another account",
            role_arn: ANOTHER_ACCOUNT.to_string(),
            assumable: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // Every row's state machine is CREATED: the control plane does not resolve the
        // role, whatever the data plane then makes of it.
        let parent = chain
            .machine(
                &format!("assume-parent-{index}"),
                &parent_definition(&child_arn),
                &row.role_arn,
            )
            .await;
        let before = started_count(&chain, &child_arn).await;
        let (execution_arn, described) = chain.ran(&parent, "only", case).await;

        if row.assumable {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the execution to run its task, got {described:?}"
            );
            assert_eq!(
                started_count(&chain, &child_arn).await,
                before + 1,
                "case {case:?}: expected the child execution the task started"
            );
            continue;
        }

        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(TASK_FAILED_ERROR)),
            "case {case:?}: expected FAILED with {TASK_FAILED_ERROR}, got {described:?}"
        );
        // The cause is the service's own sentence for an execution role it cannot act
        // as: both of the principals the assume was attempted as, and the role itself,
        // so an operator knows which role reference to act on.
        let cause = string(&described, "cause");
        for fragment in [
            "is not authorized to assume the provided role",
            "states.amazonaws.com",
            &format!("states.{REGION}.amazonaws.com"),
            row.role_arn.as_str(),
        ] {
            assert!(
                cause.contains(fragment),
                "case {case:?}: expected the cause to carry {fragment:?}, got {cause:?}"
            );
        }
        // The task reported that failure as its own, and the work it named never
        // happened: no child execution was started.
        let events = chain.history(&execution_arn).await;
        let failed: Vec<&Value> = events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED))
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one {EVENT_TASK_FAILED}, got {events:?}"
        );
        assert_eq!(
            started_count(&chain, &child_arn).await,
            before,
            "case {case:?}: expected no child execution for a role that cannot be assumed"
        );
    }
}
