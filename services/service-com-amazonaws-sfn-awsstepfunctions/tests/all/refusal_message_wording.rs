// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEXT of two of this service's refusals, driven over its
//! own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`, so what is asserted is what a signed AWS SDK request
//! observes in `$.message`.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than on the one request that exposed it:
//!
//! 1. an ARN whose RESOURCE TYPE is not one the operation reading it accepts is the
//!    modelled `InvalidArn` (client, HTTP 400) whose diagnosis says that the type is
//!    not valid in this context and names THAT type, taken from the ARN the request
//!    carried — the same sentence whichever operation read the ARN, whichever
//!    resource type that operation accepts, and whether or not any such resource
//!    exists; an ARN whose type the operation DOES accept is never refused that way;
//! 2. each operation words a refusal as the service words it there: the execution
//!    HISTORY answers a `nextToken` it did not issue with `InvalidToken` whose
//!    message is the condition alone, while the paged LIST operations of this API
//!    answer the same condition with the detail they quote after it. A token the
//!    history itself issued keeps working, so what is refused is the token and not
//!    the pagination.
//!
//! The harness (`Emulator`, `create_machine`, `start_execution`, `string`), the
//! `failure` reader and `create_express` are the ones `state_machine_lifecycle`,
//! `error_messages` and `sync_execution` already build; `tests/all.rs` is this
//! binary's crate root, so each is reached through `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    invalid_token_message, ACTIVITY_RESOURCE, EXECUTION_RESOURCE, EXPRESS_EXECUTION_RESOURCE,
    MAP_RUN_RESOURCE, STATE_MACHINE_RESOURCE,
};

use crate::error_messages::failure;
use crate::state_machine_lifecycle::{
    create_machine, start_execution, string, Emulator, ACCOUNT, PAST_THE_EXECUTION_DELAY, REGION,
};
use crate::sync_execution::create_express;

/// A definition whose two `Pass` states leave a history of more than one page at the
/// smallest page size, so a token the history itself issued can be walked.
const TWO_PASS_DEFINITION: &str = r#"{"StartAt":"First","States":{
    "First":{"Type":"Pass","Result":{"step":1},"Next":"Second"},
    "Second":{"Type":"Pass","Result":{"step":2},"End":true}}}"#;

/// A resource type no ARN of this service carries at all.
///
/// The rule reads the type off the REQUEST, so a type the service never mints is
/// reported exactly like one it does — which is what shows the sentence is not built
/// from a catalog of known types.
const UNKNOWN_RESOURCE: &str = "notAResourceType";

/// One resource type, and the resource field an ARN of that type carries.
///
/// The types are the crate's own ARN-form constants
/// (`state::{STATE_MACHINE_RESOURCE, EXECUTION_RESOURCE, EXPRESS_EXECUTION_RESOURCE,
/// ACTIVITY_RESOURCE, MAP_RUN_RESOURCE}`, each documented against the ARN form it
/// belongs to), plus [`UNKNOWN_RESOURCE`]; the remainder of each row is that form's
/// own, so the resource type is the only thing wrong with the ARN.
struct ResourceForm {
    kind: &'static str,
    resource: String,
}

/// Every resource form an ARN member below is given.
fn resource_forms() -> Vec<ResourceForm> {
    let map_run_id = "0e6e0d0e-0000-4000-8000-00000000abcd";
    vec![
        ResourceForm {
            kind: STATE_MACHINE_RESOURCE,
            resource: format!("{STATE_MACHINE_RESOURCE}:probe"),
        },
        ResourceForm {
            kind: EXECUTION_RESOURCE,
            resource: format!("{EXECUTION_RESOURCE}:probe:run-1"),
        },
        ResourceForm {
            kind: EXPRESS_EXECUTION_RESOURCE,
            resource: format!("{EXPRESS_EXECUTION_RESOURCE}:probe:run-1:{map_run_id}"),
        },
        ResourceForm {
            kind: ACTIVITY_RESOURCE,
            resource: format!("{ACTIVITY_RESOURCE}:probe-activity"),
        },
        ResourceForm {
            kind: MAP_RUN_RESOURCE,
            resource: format!("{MAP_RUN_RESOURCE}:probe/mapLabel:{map_run_id}"),
        },
        ResourceForm {
            kind: UNKNOWN_RESOURCE,
            resource: format!("{UNKNOWN_RESOURCE}:probe"),
        },
    ]
}

/// One ARN member of one operation, and the resource types that member accepts.
struct ArnMember {
    operation: &'static str,
    member: &'static str,
    accepts: &'static [&'static str],
}

/// Every ARN member an implemented operation reads, with the resource type(s) it
/// accepts — the tagging members accept two ("the ARN for the Step Functions state
/// machine or activity"), every other member one.
fn arn_members() -> Vec<ArnMember> {
    const EXECUTIONS: &[&str] = &[EXECUTION_RESOURCE];
    const STATE_MACHINES: &[&str] = &[STATE_MACHINE_RESOURCE];
    const ACTIVITIES: &[&str] = &[ACTIVITY_RESOURCE];
    const MAP_RUNS: &[&str] = &[MAP_RUN_RESOURCE];
    const TAGGABLE: &[&str] = &[STATE_MACHINE_RESOURCE, ACTIVITY_RESOURCE];
    vec![
        ArnMember {
            operation: "GetExecutionHistory",
            member: "executionArn",
            accepts: EXECUTIONS,
        },
        ArnMember {
            operation: "DescribeExecution",
            member: "executionArn",
            accepts: EXECUTIONS,
        },
        ArnMember {
            operation: "DescribeStateMachineForExecution",
            member: "executionArn",
            accepts: EXECUTIONS,
        },
        ArnMember {
            operation: "StopExecution",
            member: "executionArn",
            accepts: EXECUTIONS,
        },
        ArnMember {
            operation: "ListMapRuns",
            member: "executionArn",
            accepts: EXECUTIONS,
        },
        ArnMember {
            operation: "DescribeStateMachine",
            member: "stateMachineArn",
            accepts: STATE_MACHINES,
        },
        ArnMember {
            operation: "ListExecutions",
            member: "stateMachineArn",
            accepts: STATE_MACHINES,
        },
        ArnMember {
            operation: "DescribeActivity",
            member: "activityArn",
            accepts: ACTIVITIES,
        },
        ArnMember {
            operation: "DeleteActivity",
            member: "activityArn",
            accepts: ACTIVITIES,
        },
        ArnMember {
            operation: "ListTagsForResource",
            member: "resourceArn",
            accepts: TAGGABLE,
        },
        ArnMember {
            operation: "DescribeMapRun",
            member: "mapRunArn",
            accepts: MAP_RUNS,
        },
        ArnMember {
            operation: "ListExecutions",
            member: "mapRunArn",
            accepts: MAP_RUNS,
        },
    ]
}

/// What the modelled `InvalidArn` says about an ARN of resource type `kind` handed to
/// an operation that does not accept it.
fn expected_arn_message(kind: &str) -> String {
    format!("Invalid Arn: 'Resource type not valid in this context: {kind}'")
}

// ---------------------------------------------------------------------------
// Rule 1: the resource type the request's ARN named
// ---------------------------------------------------------------------------

/// Every ARN member of every implemented operation reports a resource type it does
/// not accept with `InvalidArn` and the one diagnosis, naming the type the REQUEST
/// carried; a type the member accepts reaches the lookup instead.
#[tokio::test]
async fn an_arn_of_the_wrong_resource_type_names_that_type_and_nothing_else() {
    let emulator = Emulator::new();
    let prefix = format!("arn:aws:states:{REGION}:{ACCOUNT}");

    for form in resource_forms() {
        let arn = format!("{prefix}:{}", form.resource);
        for member in arn_members() {
            let case = format!(
                "a {} ARN on {}.{}",
                form.kind, member.operation, member.member
            );
            let body = json!({ member.member: arn.clone() });
            if member.accepts.contains(&form.kind) {
                // The type IS accepted here, so the ARN is RESOLVED: the answer is
                // whatever this store holds under it — nothing, which is either that
                // operation's own resource-absent error or an empty report — and
                // never the resource-type refusal.
                let (status, answered) = emulator.call(member.operation, &body).await;
                let message = answered
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    !message.contains("Resource type not valid in this context"),
                    "case {case:?}: an accepted resource type must reach the lookup, got \
                     {status} with body {answered:?}"
                );
                continue;
            }
            let (status, code, message) = failure(&emulator, &case, member.operation, &body).await;
            let expected = expected_arn_message(form.kind);
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidArn"),
                "case {case:?}: expected 400 InvalidArn, got {status} {code} with message \
                 {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}

/// The ARN a SYNCHRONOUS Express execution is minted with carries the `express`
/// resource type, so the history API — which no Express execution has one for —
/// refuses the service's OWN value with that type named.
#[tokio::test]
async fn an_express_execution_arn_is_refused_with_its_own_resource_type() {
    let emulator = Emulator::new();
    let express = create_express(&emulator, "sync-history", TWO_PASS_DEFINITION).await;
    let answered = emulator
        .ok(
            "StartSyncExecution",
            &json!({ "stateMachineArn": express, "name": "sync-run" }),
        )
        .await;
    let execution = string(&answered, "executionArn");
    assert_eq!(
        execution.split(':').nth(5),
        Some(EXPRESS_EXECUTION_RESOURCE),
        "a synchronous start must mint an {EXPRESS_EXECUTION_RESOURCE} ARN, got {execution:?}"
    );

    // Every operation that reads an execution ARN refuses it the same way: the ARN
    // names a resource type none of them accepts.
    for operation in [
        "GetExecutionHistory",
        "DescribeExecution",
        "DescribeStateMachineForExecution",
        "StopExecution",
        "ListMapRuns",
    ] {
        let case = format!("the minted Express execution ARN on {operation}");
        let (status, code, message) = failure(
            &emulator,
            &case,
            operation,
            &json!({ "executionArn": execution.clone() }),
        )
        .await;
        let expected = expected_arn_message(EXPRESS_EXECUTION_RESOURCE);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidArn"),
            "case {case:?}: expected 400 InvalidArn, got {status} {code} with message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the pagination token each operation did not issue
// ---------------------------------------------------------------------------

/// Pagination tokens no operation of this service ever issued.
///
/// Rows differ along the FORM of the value — text, a signed or fractional number, a
/// padded number, a number too large to be an offset, an opaque cursor of another
/// API's shape, and a value at the modelled `PageToken` maximum (1024) — so what is
/// asserted is the rule and not one forged string.
fn forged_tokens() -> Vec<(&'static str, String)> {
    vec![
        ("text that is not a token", "not-a-token".to_string()),
        ("a negative number", "-1".to_string()),
        ("a fractional number", "1.5".to_string()),
        ("a padded number", " 7".to_string()),
        (
            "a number no history could be that long",
            "99999999999999999999999999".to_string(),
        ),
        (
            "an opaque cursor of another API's shape",
            "eyJvZmZzZXQiOiAxfQ==".to_string(),
        ),
        ("a token at the modelled maximum", "z".repeat(1024)),
    ]
}

/// A `nextToken` the history did not issue is `InvalidToken` whose message is the
/// condition alone, and a token the history DID issue walks the rest of it.
#[tokio::test]
async fn the_history_refuses_a_token_it_did_not_issue_with_the_condition_alone() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "token-history", TWO_PASS_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "run-1", "{}").await;
    std::thread::sleep(PAST_THE_EXECUTION_DELAY);

    // The control: the history pages, and the token IT issued is accepted.
    let first = emulator
        .ok(
            "GetExecutionHistory",
            &json!({ "executionArn": execution, "maxResults": 1 }),
        )
        .await;
    let issued = string(&first, "nextToken");
    let second = emulator
        .ok(
            "GetExecutionHistory",
            &json!({ "executionArn": execution, "maxResults": 1, "nextToken": issued }),
        )
        .await;
    let events = second
        .get("events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected an events list in {second:?}"));
    assert_eq!(
        events.len(),
        1,
        "the token the history issued must serve the next page, got {second:?}"
    );

    for (case, token) in forged_tokens() {
        let (status, code, message) = failure(
            &emulator,
            case,
            "GetExecutionHistory",
            &json!({ "executionArn": execution, "nextToken": token }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidToken"),
            "case {case:?}: expected 400 InvalidToken, got {status} {code} with message \
             {message:?}"
        );
        assert_eq!(
            message, "Invalid Token",
            "case {case:?}: expected the condition alone, got {message:?}"
        );
    }
}

/// The paged LIST operations answer the same condition with the detail they quote
/// after it, which is what makes the history's bare sentence that operation's own
/// wording rather than a service-wide one.
#[tokio::test]
async fn the_paged_lists_keep_the_detail_their_token_refusal_quotes() {
    let emulator = Emulator::new();
    let machine = create_machine(&emulator, "token-lists", TWO_PASS_DEFINITION).await;
    let execution = start_execution(&emulator, &machine, "run-1", "{}").await;
    let expected = invalid_token_message();
    assert_eq!(
        expected, "Invalid Token: 'Invalid token'",
        "the list refusal quotes the condition itself, got {expected:?}"
    );

    // (operation, the members that operation requires beside the token)
    let lists: Vec<(&str, Value)> = vec![
        ("ListStateMachines", json!({})),
        ("ListActivities", json!({})),
        ("ListExecutions", json!({ "stateMachineArn": machine.clone() })),
        ("ListMapRuns", json!({ "executionArn": execution.clone() })),
        (
            "ListStateMachineVersions",
            json!({ "stateMachineArn": machine.clone() }),
        ),
        (
            "ListStateMachineAliases",
            json!({ "stateMachineArn": machine.clone() }),
        ),
    ];

    for (operation, required) in lists {
        for (case, token) in forged_tokens() {
            let mut body = required.clone();
            body.as_object_mut()
                .expect("the request is an object")
                .insert("nextToken".to_string(), Value::String(token));
            let case = format!("{case} on {operation}");
            let (status, code, message) = failure(&emulator, &case, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidToken"),
                "case {case:?}: expected 400 InvalidToken, got {status} {code} with message \
                 {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
        }
    }
}
