// SPDX-License-Identifier: CC0-1.0

//! How the operation-scoped StackSets refusals are WORDED, over the awsQuery wire.
//!
//! The rule every case here holds the service to: **an operation-scoped StackSet API
//! (`DescribeStackSetOperation`, `ListStackSetOperationResults`, `StopStackSetOperation`)
//! answers `OperationNotFoundException` (HTTP 404) with ONE fixed sentence for every
//! `OperationId` the StackSet it resolved holds no operation under — the sentence names the
//! KIND of record that did not resolve and interpolates nothing from the request, so two
//! calls differing only in the id they named read identically — while a `StackSetName` that
//! resolves to no StackSet of this administrator account and Region is still refused
//! `StackSetNotFoundException` (HTTP 404) naming what the request named.**
//!
//! The table spans what the rule generalises over: every operation-scoped API, over ids
//! differing in form, length and provenance — the uuid shape a client generates, one
//! differing from it in every digit, the shortest id `OperationId` admits and one at its
//! longest (`OperationId` targets `ClientRequestToken`, `@length(1..128)` with
//! `@pattern("^[a-zA-Z0-9][-a-zA-Z0-9]*$")`:
//! `smithy_query(query="shape", shape="ClientRequestToken")`), an id that IS an operation of
//! another StackSet, and the StackSet's own name — plus the positive controls that each API
//! does resolve the operation the StackSet really holds.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out), so
//! what is asserted is what a client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{call, member, router, wait_for_the_transition, xml_text};

/// The template the StackSets here are created from: one topic, as a StackSet needs something
/// for an instance's stack to deploy.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"operation-wording"}}}}"#;

/// The sentence every operation-scoped StackSet API answers when the StackSet holds no
/// operation under the `OperationId` the request named: entirely fixed text, spelled out here
/// rather than read from the crate, because the sentence IS the requirement and a reworded one
/// must fail this suite.
const OPERATION_NOT_FOUND: &str = "Operation not found";

/// The three APIs that address ONE operation of a StackSet, each of which models
/// `OperationNotFoundException`
/// (`smithy_query(query="operation", operation="DescribeStackSetOperation")` and its two
/// siblings).
const OPERATION_SCOPED: [&str; 3] = [
    "DescribeStackSetOperation",
    "ListStackSetOperationResults",
    "StopStackSetOperation",
];

async fn create_stack_set(router: &MultiServiceRouter, stack_set_name: &str) -> (u16, String) {
    call(
        router,
        "CreateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await
}

/// Start a StackSet operation and answer the `OperationId` it was recorded under: an update
/// with the template already in force is the one call a StackSet holding no instance can be
/// asked for.
async fn start_an_operation(router: &MultiServiceRouter, stack_set_name: &str) -> String {
    let (status, body) = call(
        router,
        "UpdateStackSet",
        &[
            member("StackSetName", stack_set_name),
            member("UsePreviousTemplate", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {stack_set_name:?}: expected UpdateStackSet to start an operation, got {status} \
         {body}"
    );
    xml_text(&body, "OperationId")
        .unwrap_or_else(|| panic!("case {stack_set_name:?}: no OperationId in {body}"))
}

/// One operation-scoped call, by the API's name.
async fn call_operation_scoped(
    router: &MultiServiceRouter,
    action: &str,
    stack_set_name: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        action,
        &[
            member("StackSetName", stack_set_name),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// The `<Code>` and `<Message>` of a refusal envelope.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// Every `OperationId` the StackSet under test holds no operation under, spanning the forms
/// and lengths the member admits and both ways an id can be real elsewhere: an operation of
/// another StackSet, and the name of the StackSet itself.
fn unresolved_ids(stack_set_name: &str, elsewhere_operation: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "a uuid-shaped id nothing started",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "another uuid-shaped id, differing in every digit",
            "9f3c1b2a-5d6e-4f70-8a91-b2c3d4e5f607".to_string(),
        ),
        // The bounds `ClientRequestToken` states: one character, and 128 of them.
        ("the shortest id the member admits", "x".to_string()),
        (
            "an id at the longest the member admits",
            format!("o{}", "p".repeat(127)),
        ),
        (
            "an operation of ANOTHER StackSet",
            elsewhere_operation.to_string(),
        ),
        (
            "the StackSet's own name, which addresses no operation",
            stack_set_name.to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_operation_id_that_resolves_nothing_is_refused_with_one_fixed_sentence() {
    let router = router();
    let stack_set_name = "fleet-operation-wording";
    let elsewhere = "fleet-operation-wording-elsewhere";
    for name in [stack_set_name, elsewhere] {
        let (status, body) = create_stack_set(&router, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateStackSet to succeed, got {status} {body}"
        );
    }
    // An operation of EACH StackSet, so the id of one is a real id that the other holds
    // nothing under. Each settles before the next call, since a StackSet serves one
    // operation at a time.
    let elsewhere_operation = start_an_operation(&router, elsewhere).await;
    wait_for_the_transition().await;
    let operation_id = start_an_operation(&router, stack_set_name).await;
    wait_for_the_transition().await;

    // One fixed sentence, whichever id the request named and whichever API read it: the
    // messages are collected as well as asserted, so an answer that interpolated the id
    // would also fail the "one sentence" check below.
    let mut answered: Vec<String> = Vec::new();
    for (case, unresolved) in unresolved_ids(stack_set_name, &elsewhere_operation) {
        for action in OPERATION_SCOPED {
            let (status, body) =
                call_operation_scoped(&router, action, stack_set_name, &unresolved).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {action}: expected the call refused with 404, got {status} \
                 {body}"
            );
            let (code, message) = refusal(&body);
            assert_eq!(
                code.as_deref(),
                Some("OperationNotFoundException"),
                "case {case:?} on {action}: expected the modelled code, got {code:?} in {body}"
            );
            assert_eq!(
                message.as_deref(),
                Some(OPERATION_NOT_FOUND),
                "case {case:?} on {action}: expected {OPERATION_NOT_FOUND:?}, got {message:?}"
            );
            // Nothing of the request is in the sentence, so the id the row named cannot be
            // part of it — checked on the ids long enough for the containment to mean
            // something.
            if unresolved.len() >= 8 {
                let reported = message.clone().unwrap_or_default();
                assert!(
                    !reported.contains(unresolved.as_str()),
                    "case {case:?} on {action}: expected no part of the request in the \
                     message, got {reported:?}"
                );
            }
            answered.push(message.unwrap_or_default());
        }
    }
    answered.sort();
    answered.dedup();
    assert_eq!(
        answered,
        vec![OPERATION_NOT_FOUND.to_string()],
        "case {:?}: expected every refusal to read identically, got {answered:?}",
        "one sentence across every id and every API"
    );

    // The positive controls: each API DOES resolve the operation the StackSet holds, so the
    // refusals above are about the id and not about the API. The stop answers the modelled
    // `InvalidOperationException` (400) rather than 404, which is the answer for an operation
    // that resolved and has already concluded.
    let (status, body) =
        call_operation_scoped(&router, "DescribeStackSetOperation", stack_set_name, &operation_id)
            .await;
    assert_eq!(
        status, 200,
        "case \"the operation the StackSet holds\" on DescribeStackSetOperation: expected 200, \
         got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "OperationId").as_deref(),
        Some(operation_id.as_str()),
        "case \"the operation the StackSet holds\": expected the operation reported back, got \
         {body}"
    );
    let (status, body) = call_operation_scoped(
        &router,
        "ListStackSetOperationResults",
        stack_set_name,
        &operation_id,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the operation the StackSet holds\" on ListStackSetOperationResults: expected \
         200, got {status} {body}"
    );
    let (status, body) =
        call_operation_scoped(&router, "StopStackSetOperation", stack_set_name, &operation_id)
            .await;
    assert_eq!(
        status, 400,
        "case \"the operation the StackSet holds\" on StopStackSetOperation: expected the \
         concluded operation to be refused with 400, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("InvalidOperationException"),
        "case \"the operation the StackSet holds\" on StopStackSetOperation: expected the \
         operation to have resolved, got {body}"
    );
}

/// The StackSet-level refusal is untouched by the operation-level one: it names the StackSet
/// the request named, on every operation-scoped API.
#[tokio::test]
async fn a_stack_set_that_resolves_nothing_is_still_refused_naming_it() {
    let router = router();
    let stack_set_name = "fleet-operation-wording-stackset-level";
    let (status, body) = create_stack_set(&router, stack_set_name).await;
    assert_eq!(
        status, 200,
        "expected CreateStackSet to succeed, got {status} {body}"
    );
    let operation_id = start_an_operation(&router, stack_set_name).await;
    wait_for_the_transition().await;

    // A StackSet that resolves nothing, with an id that IS an operation of the one that does:
    // the StackSet is what the request failed to resolve, so that is what the refusal names.
    for (case, unresolved_stack_set) in [
        ("a StackSet name nothing holds", "fleet-never-created"),
        (
            "a StackSet name differing only in case",
            "FLEET-OPERATION-WORDING-STACKSET-LEVEL",
        ),
    ] {
        for action in OPERATION_SCOPED {
            let (status, body) =
                call_operation_scoped(&router, action, unresolved_stack_set, &operation_id).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {action}: expected the call refused with 404, got {status} \
                 {body}"
            );
            let (code, message) = refusal(&body);
            assert_eq!(
                code.as_deref(),
                Some("StackSetNotFoundException"),
                "case {case:?} on {action}: expected the modelled code, got {code:?} in {body}"
            );
            let expected = format!("StackSet {unresolved_stack_set} not found");
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {case:?} on {action}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
