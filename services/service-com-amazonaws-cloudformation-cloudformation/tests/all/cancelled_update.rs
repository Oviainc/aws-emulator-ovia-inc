// SPDX-License-Identifier: CC0-1.0

//! `CancelUpdateStack`: ending the UPDATE in flight on a stack, over the awsQuery wire.
//!
//! **The rule every case here holds the service to: `CancelUpdateStack` "cancels an update
//! on the specified stack … the stack rolls back the update and reverts to the previous
//! stack configuration", and "you can cancel only stacks that are in the UPDATE_IN_PROGRESS
//! state" — so the call is answered only while an update is in flight, and what it then does
//! is the rollback of that update: the configuration the update replaced goes back onto the
//! stack and whatever the update had already changed in the services that own the stack's
//! resources is put back there, in whichever direction the revert runs. A stack in any other
//! status is refused with a fixed sentence about the call and left exactly where it is, and a
//! caller presenting a `ClientRequestToken` an operation on that stack already claimed gets
//! the modelled `TokenAlreadyExistsException`.**
//!
//! The table spans the class the rule generalises over rather than one request: the three
//! directions a revert runs in (a resource the update CHANGED put back, one it ADDED removed,
//! one it REMOVED re-created), a resource the update left alone, and the caller that asked for
//! the failed state to be kept (`DisableRollback`) — plus, on the refusal side, every status a
//! stack can be in when the call arrives, and a stack nothing in this Region holds.
//!
//! Each update is held open by an `AWS::CloudFormation::WaitCondition` nothing signals:
//! CloudFormation "doesn't proceed with a stack creation or update until resources receive the
//! required number of signals or the timeout period is exceeded", so the update really is
//! `UPDATE_IN_PROGRESS` when the cancel arrives rather than racing the emulator's clock. The
//! resources are `AWS::SSM::Parameter`s, read back through Systems Manager's OWN wire, so what
//! a row asserts about the revert is what that service really holds.
//!
//! The refusal sentences are spelled out here rather than imported from the crate, because the
//! sentence IS the requirement: a reworded message must fail this suite.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::stack_lifecycle::{
    call, create_stack, get_template, member, router_with_engine, xml_text, xml_texts,
};
use crate::update_rollback_recovery::{
    parameter, parameter_value, reason_of, stack_events, status_of, template,
};

/// The fixed sentence `CancelUpdateStack` answers for a stack whose status has no update in
/// flight for it to cancel: it names the operation that cannot be called and nothing of the
/// stack it was called against.
const CANNOT_CANCEL: &str = "CancelUpdateStack cannot be called from current stack status";

/// The `StackStatusReason` a cancelled update and the rollback it owes carry.
const CANCELLED_BY_USER: &str = "Stack update cancelled by user.";

/// The wire code a request presenting a token already claimed for that stack is refused
/// under (`TokenAlreadyExistsException`'s `awsQueryError` code).
const ALREADY_USED: &str = "TokenAlreadyExistsException";

/// The `Timeout` the wait condition these updates declare asks for, in seconds.
///
/// Longer than any case here takes, so every update stays in flight until the cancel ends
/// it: the wait is what holds the update open, and nothing in these cases signals it.
const HELD_TIMEOUT: &str = "600";

/// `resources` plus the wait condition that holds an update open, which nothing signals.
fn template_held_open(resources: Value) -> String {
    let mut declared: Map<String, Value> = match resources {
        Value::Object(members) => members,
        other => panic!("a Resources section must be an object, got {other:?}"),
    };
    declared.insert(
        "Gate".to_string(),
        json!({"Type": "AWS::CloudFormation::WaitConditionHandle"}),
    );
    declared.insert(
        "Wait".to_string(),
        json!({
            "Type": "AWS::CloudFormation::WaitCondition",
            "Properties": {
                "Handle": {"Ref": "Gate"},
                "Timeout": HELD_TIMEOUT,
                "Count": "1",
            },
        }),
    );
    template(Value::Object(declared))
}

/// `UpdateStack` with a template, `DisableRollback` where the row asks for it, and the
/// row's `ClientRequestToken` when it names one.
async fn update_stack(
    router: &MultiServiceRouter,
    stack: &str,
    body: &str,
    disable_rollback: bool,
    token: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", stack), member("TemplateBody", body)];
    if disable_rollback {
        members.push(member("DisableRollback", "true"));
    }
    if let Some(token) = token {
        members.push(member("ClientRequestToken", token));
    }
    call(router, "UpdateStack", &members).await
}

/// `CancelUpdateStack`, with the row's `ClientRequestToken` when it names one.
async fn cancel_update(
    router: &MultiServiceRouter,
    stack: &str,
    token: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    if let Some(token) = token {
        members.push(member("ClientRequestToken", token));
    }
    call(router, "CancelUpdateStack", &members).await
}

/// The template body a stack reports as the one in force.
async fn template_in_force(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (http_status, body) = get_template(router, stack, None).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected GetTemplate to succeed, got {http_status} {body}"
    );
    xml_text(&body, "TemplateBody").unwrap_or_default()
}

/// One row of the cancellation table: what the stack is created from, what the update it
/// cancels submits, and what the cancel must leave behind.
struct CancelCase {
    case: &'static str,
    stack: &'static str,
    created: String,
    updated: String,
    /// `DisableRollback`, exactly as the update carries it.
    disable_rollback: bool,
    /// The status the stack reports as soon as the cancel is answered.
    cancelling: &'static str,
    /// The status it settles in once the clock makes that transition due.
    settles_to: &'static str,
    /// What Parameter Store holds while the update is in flight — the change the cancel has
    /// to undo.
    after_update: Vec<(String, Option<&'static str>)>,
    /// ...and what it holds once the cancel has been carried out.
    after_cancel: Vec<(String, Option<&'static str>)>,
    /// Whether the template in force must be the one the create deployed again.
    reverts_the_template: bool,
}

/// The table: every direction a cancelled update's revert can run in, and the caller that
/// declines the rollback.
fn cancel_cases() -> Vec<CancelCase> {
    vec![
        CancelCase {
            case: "a resource the update changed is put back",
            stack: "cancel-changed",
            created: template(json!({"Param": parameter("/cancel-changed/p", "before")})),
            updated: template_held_open(
                json!({"Param": parameter("/cancel-changed/p", "after")}),
            ),
            disable_rollback: false,
            cancelling: status::UPDATE_ROLLBACK_IN_PROGRESS,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            after_update: vec![("/cancel-changed/p".to_string(), Some("after"))],
            after_cancel: vec![("/cancel-changed/p".to_string(), Some("before"))],
            reverts_the_template: true,
        },
        CancelCase {
            case: "a resource the update added is removed, and the one it left alone is kept",
            stack: "cancel-added",
            created: template(json!({"Kept": parameter("/cancel-added/k", "kept")})),
            updated: template_held_open(json!({
                "Kept": parameter("/cancel-added/k", "kept"),
                "Added": parameter("/cancel-added/a", "added"),
            })),
            disable_rollback: false,
            cancelling: status::UPDATE_ROLLBACK_IN_PROGRESS,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            after_update: vec![
                ("/cancel-added/k".to_string(), Some("kept")),
                ("/cancel-added/a".to_string(), Some("added")),
            ],
            after_cancel: vec![
                ("/cancel-added/k".to_string(), Some("kept")),
                ("/cancel-added/a".to_string(), None),
            ],
            reverts_the_template: true,
        },
        CancelCase {
            case: "a resource the update removed is re-created",
            stack: "cancel-dropped",
            created: template(json!({
                "Kept": parameter("/cancel-dropped/k", "kept"),
                "Dropped": parameter("/cancel-dropped/d", "dropped"),
            })),
            updated: template_held_open(
                json!({"Kept": parameter("/cancel-dropped/k", "kept")}),
            ),
            disable_rollback: false,
            cancelling: status::UPDATE_ROLLBACK_IN_PROGRESS,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            after_update: vec![("/cancel-dropped/d".to_string(), None)],
            after_cancel: vec![
                ("/cancel-dropped/k".to_string(), Some("kept")),
                ("/cancel-dropped/d".to_string(), Some("dropped")),
            ],
            reverts_the_template: true,
        },
        // "DisableRollback: Preserve the state of previously provisioned resources when an
        // operation fails": the caller said what it wants left behind, so the cancelled
        // update is not rolled back and the owning service keeps what it put there.
        CancelCase {
            case: "DisableRollback keeps the cancelled update as it stood",
            stack: "cancel-kept",
            created: template(json!({"Param": parameter("/cancel-kept/p", "before")})),
            updated: template_held_open(json!({"Param": parameter("/cancel-kept/p", "after")})),
            disable_rollback: true,
            cancelling: status::UPDATE_FAILED,
            settles_to: status::UPDATE_FAILED,
            after_update: vec![("/cancel-kept/p".to_string(), Some("after"))],
            after_cancel: vec![("/cancel-kept/p".to_string(), Some("after"))],
            reverts_the_template: false,
        },
    ]
}

#[tokio::test]
async fn cancelling_an_update_reverts_it_to_the_previous_stack_configuration() {
    for case in cancel_cases() {
        let label = case.case;
        let (router, engine) = router_with_engine();

        let (http_status, body) = create_stack(&router, case.stack, &case.created, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {label:?}: expected CreateStack to succeed, got {http_status} {body}"
        );
        settle_the_store(&router, &engine).await;
        let created = status_of(&router, case.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {label:?}: expected the create to complete before the update, got \
             {created:?}"
        );
        let deployed = template_in_force(label, &router, case.stack).await;

        let (http_status, body) = update_stack(
            &router,
            case.stack,
            &case.updated,
            case.disable_rollback,
            None,
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {label:?}: expected UpdateStack to succeed, got {http_status} {body}"
        );
        let in_flight = status_of(&router, case.stack).await;
        assert_eq!(
            in_flight,
            status::UPDATE_IN_PROGRESS,
            "case {label:?}: expected the update to be in flight when the cancel arrives, \
             got {in_flight:?}"
        );
        for (name, value) in &case.after_update {
            let held = parameter_value(&router, name).await;
            assert_eq!(
                held.as_deref(),
                *value,
                "case {label:?}: expected {name:?} to hold {value:?} while the update is in \
                 flight, got {held:?}"
            );
        }

        let (http_status, body) = cancel_update(&router, case.stack, None).await;
        assert_eq!(
            http_status, 200,
            "case {label:?}: expected CancelUpdateStack to succeed, got {http_status} {body}"
        );
        let cancelling = status_of(&router, case.stack).await;
        assert_eq!(
            cancelling, case.cancelling,
            "case {label:?}: expected the cancel to leave the stack {:?}, got {cancelling:?}",
            case.cancelling
        );
        let reason = reason_of(&router, case.stack).await;
        assert_eq!(
            reason, CANCELLED_BY_USER,
            "case {label:?}: expected the cancelled update's reason to be \
             {CANCELLED_BY_USER:?}, got {reason:?}"
        );

        settle_the_store(&router, &engine).await;
        let settled = status_of(&router, case.stack).await;
        assert_eq!(
            settled, case.settles_to,
            "case {label:?}: expected the cancelled update to settle in {:?}, got {settled:?}",
            case.settles_to
        );
        for (name, value) in &case.after_cancel {
            let held = parameter_value(&router, name).await;
            assert_eq!(
                held.as_deref(),
                *value,
                "case {label:?}: expected {name:?} to hold {value:?} once the cancel was \
                 carried out, got {held:?}"
            );
        }

        let events = stack_events(label, &router, case.stack).await;
        let terminal = (case.stack.to_string(), case.settles_to.to_string());
        assert!(
            events.contains(&terminal),
            "case {label:?}: expected the stack's own {:?} event, got {events:?}",
            case.settles_to
        );

        let in_force = template_in_force(label, &router, case.stack).await;
        if case.reverts_the_template {
            assert_eq!(
                in_force, deployed,
                "case {label:?}: expected the previous configuration's template back, got \
                 {in_force:?}"
            );
        } else {
            assert_ne!(
                in_force, deployed,
                "case {label:?}: a caller that asked for the failed state to be kept keeps \
                 the update's own configuration, got {in_force:?}"
            );
        }
    }
}

/// One row of the refusal table: how to get the stack into a status, and the status it is
/// then in when the cancel arrives.
struct RefusedCase {
    case: &'static str,
    status: &'static str,
}

/// Every status the walk below puts one stack through, in the order it reaches them. None of
/// them has an update in flight, so the cancel is refused from each.
fn refused_cases() -> Vec<RefusedCase> {
    vec![
        RefusedCase {
            case: "while the create is in progress",
            status: status::CREATE_IN_PROGRESS,
        },
        RefusedCase {
            case: "after the create completed",
            status: status::CREATE_COMPLETE,
        },
        RefusedCase {
            case: "after an update completed",
            status: status::UPDATE_COMPLETE,
        },
        RefusedCase {
            case: "while the rollback of a cancelled update is in progress",
            status: status::UPDATE_ROLLBACK_IN_PROGRESS,
        },
        RefusedCase {
            case: "after that rollback completed",
            status: status::UPDATE_ROLLBACK_COMPLETE,
        },
    ]
}

/// Refuse the cancel and assert the stack was left exactly where it was.
async fn assert_refused(case: &RefusedCase, router: &MultiServiceRouter, stack: &str) {
    let before = status_of(router, stack).await;
    assert_eq!(
        before, case.status,
        "case {:?}: the walk must reach {:?} before the cancel, got {before:?}",
        case.case, case.status
    );
    let (http_status, body) = cancel_update(router, stack, None).await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected the cancel to be refused with 400, got {http_status} {body}",
        case.case
    );
    let code = xml_text(&body, "Code");
    assert_eq!(
        code.as_deref(),
        Some("ValidationError"),
        "case {:?}: expected <Code>ValidationError</Code>, got {code:?} {body}",
        case.case
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert_eq!(
        message, CANNOT_CANCEL,
        "case {:?}: expected the refusal {CANNOT_CANCEL:?}, got {message:?}",
        case.case
    );
    let after = status_of(router, stack).await;
    assert_eq!(
        after, case.status,
        "case {:?}: a refused cancel must leave the stack where it was, got {after:?}",
        case.case
    );
}

#[tokio::test]
async fn an_update_can_be_cancelled_only_while_it_is_in_flight() {
    let (router, engine) = router_with_engine();
    let stack = "cancel-gated";
    let created = template(json!({"Param": parameter("/cancel-gated/p", "before")}));
    let updated = template(json!({"Param": parameter("/cancel-gated/p", "after")}));
    let held_open = template_held_open(json!({"Param": parameter("/cancel-gated/p", "held")}));
    let rows = refused_cases();

    let (http_status, body) = create_stack(&router, stack, &created, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    assert_refused(&rows[0], &router, stack).await;

    settle_the_store(&router, &engine).await;
    assert_refused(&rows[1], &router, stack).await;

    let (http_status, body) = update_stack(&router, stack, &updated, false, None).await;
    assert_eq!(
        http_status, 200,
        "expected UpdateStack to succeed, got {http_status} {body}"
    );
    settle_the_store(&router, &engine).await;
    assert_refused(&rows[2], &router, stack).await;

    // An update held at the wait, cancelled: the rollback it owes is in flight, and that is
    // not a status a cancel can be made from either.
    let (http_status, body) = update_stack(&router, stack, &held_open, false, None).await;
    assert_eq!(
        http_status, 200,
        "expected the held-open UpdateStack to succeed, got {http_status} {body}"
    );
    let (http_status, body) = cancel_update(&router, stack, None).await;
    assert_eq!(
        http_status, 200,
        "expected CancelUpdateStack to succeed, got {http_status} {body}"
    );
    assert_refused(&rows[3], &router, stack).await;

    settle_the_store(&router, &engine).await;
    assert_refused(&rows[4], &router, stack).await;

    // A stack this Region does not hold is not a status refusal at all: the call is refused
    // about the stack, in the template this operation words that class with.
    for named in ["cancel-absent", "arn:aws:cloudformation:us-east-1:000000000000:stack/ghost/0"]
    {
        let (http_status, body) = cancel_update(&router, named, None).await;
        assert_eq!(
            http_status, 400,
            "case {named:?}: expected an unknown stack to be refused with 400, got \
             {http_status} {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            message,
            format!("Stack: {named} does not exist"),
            "case {named:?}: expected the colon-prefixed not-found template, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_cancel_is_held_to_the_client_request_token_rule() {
    let (router, engine) = router_with_engine();
    let stack = "cancel-tokens";
    let created = template(json!({"Param": parameter("/cancel-tokens/p", "before")}));
    let updated = template_held_open(json!({"Param": parameter("/cancel-tokens/p", "after")}));

    let (http_status, body) = create_stack(&router, stack, &created, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    settle_the_store(&router, &engine).await;

    let (http_status, body) = update_stack(&router, stack, &updated, false, Some("the-update")).await;
    assert_eq!(
        http_status, 200,
        "expected UpdateStack to succeed, got {http_status} {body}"
    );

    // The token the UPDATE claimed names that operation, so the cancel cannot run under it.
    let (http_status, body) = cancel_update(&router, stack, Some("the-update")).await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected the reused token to be refused, got {http_status} {body}",
        "a cancel under the update's own token"
    );
    let code = xml_text(&body, "Code");
    assert_eq!(
        code.as_deref(),
        Some(ALREADY_USED),
        "case {:?}: expected the wire code {ALREADY_USED}, got {code:?} {body}",
        "a cancel under the update's own token"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert!(
        message.contains("the-update"),
        "case {:?}: the refusal must name the token the request reused, got {message:?}",
        "a cancel under the update's own token"
    );
    // ...and the refused cancel changed nothing: the update is still in flight.
    let in_flight = status_of(&router, stack).await;
    assert_eq!(
        in_flight,
        status::UPDATE_IN_PROGRESS,
        "case {:?}: a refused cancel must leave the update in flight, got {in_flight:?}",
        "a cancel under the update's own token"
    );

    // A token new to this stack is accepted, and the rollback the cancel opens runs under it
    // — "all events initiated by a given stack operation are assigned the same client
    // request token".
    let (http_status, body) = cancel_update(&router, stack, Some("the-cancel")).await;
    assert_eq!(
        http_status, 200,
        "case {:?}: expected a cancel under a fresh token to be accepted, got {http_status} \
         {body}",
        "a cancel under a token new to the stack"
    );
    settle_the_store(&router, &engine).await;
    let settled = status_of(&router, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {:?}: expected the cancelled update to be rolled back, got {settled:?}",
        "a cancel under a token new to the stack"
    );
    let (http_status, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStackEvents to succeed, got {http_status} {body}"
    );
    let tokens = xml_texts(&body, "ClientRequestToken");
    assert!(
        tokens.iter().any(|token| token == "the-cancel"),
        "case {:?}: expected the rollback's events to carry the cancel's own token, got \
         {tokens:?}",
        "a cancel under a token new to the stack"
    );
}
