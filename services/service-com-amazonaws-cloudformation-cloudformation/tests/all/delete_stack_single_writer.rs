// SPDX-License-Identifier: CC0-1.0

//! `DeleteStack` as the SINGLE WRITER of a stack, end to end over the awsQuery wire.
//!
//! Two rules, each with its own table.
//!
//! 1. **CloudFormation runs one operation at a time against a stack, and the operation a
//!    deletion cannot run beside is an UPDATE: a `DeleteStack` that arrives while a stack's
//!    update is in flight is refused, with the stack left exactly as it was, and the same
//!    call is accepted once that update has concluded.** The update is what is changing and
//!    replacing the very resources the deletion would dispose of, so a delete taken while it
//!    runs would race it and leave resources neither operation accounts for.
//!
//! 2. **`RetainResources` can only name resources that are still THERE** — retaining one is
//!    leaving it exactly where it is — so a list naming a resource whose own deletion has
//!    already completed, which is every resource of a stack whose deletion completed, is
//!    refused and nothing is deleted.
//!
//! The first table's rows are the statuses an update holds a stack in while it runs (the
//! update itself and the rollback of a cancelled one) crossed with what the delete carries
//! (no token, and a token new to the stack), and it closes on the control that the stack
//! takes the identical delete once the rollback has concluded. The second table's rows are
//! the ways a named resource can be absent: a stack whose deletion completed addressed by its
//! id and by its name, and a logical id a live stack never declared — with the control that
//! the same delete WITHOUT the member is still the no-op success a second deletion has always
//! been.
//!
//! The update is held open by an `AWS::CloudFormation::WaitCondition` nothing signals, so it
//! really is in flight when the delete arrives rather than racing the emulator's clock. The
//! refusal sentences are spelled out here rather than imported from the crate, because the
//! sentence IS the requirement.

use serde_json::json;
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{
    call, create_stack, member, router, router_with_engine, wait_for_the_transition, xml_text,
};
use crate::update_rollback_recovery::{
    parameter, status_of, template, template_with_an_unsignalled_gate,
};

/// This service's one wire code for a request it refuses on input grounds.
const VALIDATION_ERROR: &str = "ValidationError";

/// The sentence a `RetainResources` list naming a resource that is not there to be retained
/// is refused with: entirely fixed text, naming nothing of the request.
const RETAINED_MUST_EXIST: &str = "The specified resources to retain must be in a valid \
     state. Do not specify resources that are in the DELETE_COMPLETE state.";

/// A one-resource stack, read back and deleted through the service that owns the resource.
const ONE_TOPIC: &str = r#"{"Resources":{
    "Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"single-writer-topic"}}
}}"#;

/// `DeleteStack` with whatever `RetainResources` and `ClientRequestToken` the row names.
async fn delete_stack(
    router: &MultiServiceRouter,
    stack: &str,
    retained: &[&str],
    token: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    for (position, logical_id) in retained.iter().enumerate() {
        members.push(member(
            &format!("RetainResources.member.{}", position + 1),
            logical_id,
        ));
    }
    if let Some(token) = token {
        members.push(member("ClientRequestToken", token));
    }
    call(router, "DeleteStack", &members).await
}

/// The `StackId` one stack reports, which is how a deleted stack stays addressable.
async fn stack_id_of(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (http_status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: DescribeStacks({stack}) must answer 200, got {http_status} {body}"
    );
    xml_text(&body, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a StackId, got {body}"))
}

/// Assert one refused call: the status, the code and the whole sentence.
fn assert_refused(case: &str, answered: (u16, String), expected: &str) {
    let (http_status, body) = answered;
    assert_eq!(
        http_status, 400,
        "case {case:?}: expected the refusal, got {http_status} {body}"
    );
    let code = xml_text(&body, "Code");
    assert_eq!(
        code.as_deref(),
        Some(VALIDATION_ERROR),
        "case {case:?}: expected {VALIDATION_ERROR}, got {code:?} in {body}"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}

#[tokio::test]
async fn a_delete_is_refused_while_an_update_is_in_flight() {
    let (router, engine) = router_with_engine();
    let stack = "delete-during-update";
    let created = template(json!({"Param": parameter("/delete-during-update/p", "before")}));
    let held_open = template_with_an_unsignalled_gate(
        json!({"Param": parameter("/delete-during-update/p", "after")}),
    );

    let (http_status, body) = create_stack(&router, stack, &created, &[]).await;
    assert_eq!(
        http_status, 200,
        "CreateStack must answer 200, got {http_status} {body}"
    );
    settle_the_store(&router, &engine).await;
    let stack_id = stack_id_of("the created stack", &router, stack).await;

    let (http_status, body) = call(
        &router,
        "UpdateStack",
        &[member("StackName", stack), member("TemplateBody", &held_open)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "UpdateStack must answer 200, got {http_status} {body}"
    );

    // The rows: each status an update holds the stack in while it runs, crossed with what
    // the delete carries. `UPDATE_COMPLETE_CLEANUP_IN_PROGRESS` is the third such status and
    // is not reachable from a client call here: it belongs to an update that has already
    // applied every change.
    let tokens: [(&str, Option<&str>); 2] =
        [("no token", None), ("a token new to the stack", Some("a-fresh-delete"))];
    for (held, carried) in [
        (status::UPDATE_IN_PROGRESS, "the update itself"),
        (
            status::UPDATE_ROLLBACK_IN_PROGRESS,
            "the rollback of the cancelled update",
        ),
    ] {
        if held == status::UPDATE_ROLLBACK_IN_PROGRESS {
            let (http_status, body) =
                call(&router, "CancelUpdateStack", &[member("StackName", stack)]).await;
            assert_eq!(
                http_status, 200,
                "CancelUpdateStack must answer 200, got {http_status} {body}"
            );
        }
        let in_flight = status_of(&router, stack).await;
        assert_eq!(
            in_flight, held,
            "case {carried:?}: the stack must be {held} when the delete arrives, got \
             {in_flight:?}"
        );
        for (token_case, token) in tokens {
            let case = format!("{carried}, {token_case}");
            assert_refused(
                &case,
                delete_stack(&router, stack, &[], token).await,
                &format!("Stack:{stack_id} is in {held} state and can not be deleted."),
            );
            // The refused delete changed nothing: the operation in flight is still the one
            // the stack is running.
            let after = status_of(&router, stack).await;
            assert_eq!(
                after, held,
                "case {case:?}: a refused delete must leave the stack where it was, got \
                 {after:?}"
            );
        }
    }

    // ...and the identical call is accepted once that update has concluded: the refusal was
    // about the operation in flight, not about this stack.
    settle_the_store(&router, &engine).await;
    let settled = status_of(&router, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "the cancelled update must settle before the control row, got {settled:?}"
    );
    let (http_status, body) = delete_stack(&router, stack, &[], Some("a-fresh-delete")).await;
    assert_eq!(
        http_status, 200,
        "case \"once the update concluded\": expected the delete to be accepted, got \
         {http_status} {body}"
    );
    let deleting = status_of(&router, &stack_id).await;
    assert_eq!(
        deleting,
        status::DELETE_IN_PROGRESS,
        "case \"once the update concluded\": expected the deletion to have started, got \
         {deleting:?}"
    );
}

#[tokio::test]
async fn retaining_a_resource_that_is_no_longer_there_is_refused() {
    let router = router();

    // A stack whose deletion COMPLETED: every resource it held is disposed of, so there is
    // nothing left for a later delete to retain.
    let (http_status, body) = create_stack(&router, "retain-deleted", ONE_TOPIC, &[]).await;
    assert_eq!(
        http_status, 200,
        "CreateStack must answer 200, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let deleted_id = stack_id_of("the stack to delete", &router, "retain-deleted").await;
    let (http_status, body) = delete_stack(&router, "retain-deleted", &[], None).await;
    assert_eq!(
        http_status, 200,
        "the first DeleteStack must be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let gone = status_of(&router, &deleted_id).await;
    assert_eq!(
        gone,
        status::DELETE_COMPLETE,
        "the first deletion must complete before the rows, got {gone:?}"
    );

    // ...and a LIVE stack, for the row that names a logical id it never declared.
    let (http_status, body) = create_stack(&router, "retain-live", ONE_TOPIC, &[]).await;
    assert_eq!(
        http_status, 200,
        "CreateStack must answer 200, got {http_status} {body}"
    );
    wait_for_the_transition().await;

    // One row per way a named resource can be absent.
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "a stack whose deletion completed, addressed by its id",
            &deleted_id,
            &["Topic"],
        ),
        (
            "a stack whose deletion completed, addressed by its name",
            "retain-deleted",
            &["Topic"],
        ),
        (
            "a logical id the live stack never declared",
            "retain-live",
            &["Absent"],
        ),
        (
            "one declared resource beside one the stack never had",
            "retain-live",
            &["Topic", "Absent"],
        ),
    ];
    for (case, named, retained) in rows {
        assert_refused(
            case,
            delete_stack(&router, named, retained, None).await,
            RETAINED_MUST_EXIST,
        );
    }

    // The live stack is untouched by its refused calls, and the deleted stack still answers
    // the same delete WITHOUT the member as the no-op success a second deletion has always
    // been: what was refused is the LIST, not the deletion.
    let live = status_of(&router, "retain-live").await;
    assert_eq!(
        live,
        status::CREATE_COMPLETE,
        "case \"the live stack\": a refused delete must leave it where it was, got {live:?}"
    );
    for (case, named) in [
        ("the deleted stack by its id", deleted_id.as_str()),
        ("the deleted stack by its name", "retain-deleted"),
    ] {
        let (http_status, body) = delete_stack(&router, named, &[], None).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected the no-op success, got {http_status} {body}"
        );
    }
}
