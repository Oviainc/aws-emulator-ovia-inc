// SPDX-License-Identifier: CC0-1.0

//! The SQUARE-BRACKETED stack-not-found refusal, over the awsQuery wire.
//!
//! **The rule every case here holds the service to: CloudFormation words the "that stack is
//! not there" class with a fixed template belonging to the OPERATION that refused, and the
//! operations that read a stack's LOGS (its event stream, its drift results) and the ones that
//! address its CHANGE SETS word it `Stack [<StackName>] does not exist` — HTTP 400, error code
//! `ValidationError`, the name spelled exactly as the request sent it as the sentence's only
//! variable part. The template belongs to the operation, not to the service: the reads of a
//! stack's own record and of its resources keep the sentence that introduces the name with
//! "with id", so no single service-wide sentence satisfies both sides.**
//!
//! `stack_refusal_wording.rs` holds the whole family — every template this emulator has
//! evidence of, one operation per template. This suite is the bracketed side of it in depth:
//! each of its operations crossed with six ways of naming a stack that is not addressable
//! (a name nothing created, the name of a stack that was created and deleted, a name that
//! differs from a live stack's only in case, a well-formed stack id no stack carries, a live
//! stack's own id addressed from another Region, and the shortest name the member's pattern
//! admits), plus the optional members each operation takes — so the refusal is shown to
//! precede filtering and paging rather than to depend on them.
//!
//! Every call drives the real request path (form-encoded body in, XML envelope out) through
//! the one router `harness.rs` builds, so what is asserted is what a client sees.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, call_in, deploy, describe_stack, member, router, wait_for_the_transition, xml_text,
    ACCOUNT_ID, REGION,
};

/// The template the live stacks here are deployed from: one topic, so the control rows have a
/// stack that really has a resource to report events and drift results for.
const TEMPLATE: &str = r#"{"Resources":{
     "Topic":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"not-found-wording"}}}}"#;

/// A Region other than the one these requests are signed for, for the row that addresses a
/// live stack's id from outside the Region that holds it.
const OTHER_REGION: &str = "us-west-2";

/// One row of the operation table: the action, and the members it needs beside `StackName`.
struct Call {
    case: &'static str,
    action: &'static str,
    extra: Vec<(String, String)>,
}

/// The operations whose template for the class is the square-bracketed one, each spelled both
/// with only `StackName` and with the optional members it takes — the refusal is decided on
/// the stack, so neither a filter nor an explicit change-set type changes it.
fn bracketing_calls() -> Vec<Call> {
    vec![
        Call {
            case: "the event stream of a stack",
            action: "DescribeStackEvents",
            extra: vec![],
        },
        Call {
            case: "the drift results of a stack",
            action: "DescribeStackResourceDrifts",
            extra: vec![],
        },
        Call {
            // `StackResourceDriftStatusFilters` is optional: naming one does not turn the
            // refusal into an empty, filtered page.
            case: "the drift results of a stack, filtered by drift status",
            action: "DescribeStackResourceDrifts",
            extra: vec![member(
                "StackResourceDriftStatusFilters.member.1",
                "IN_SYNC",
            )],
        },
        Call {
            case: "the change sets of a stack",
            action: "ListChangeSets",
            extra: vec![],
        },
        Call {
            // "By default, CloudFormation specifies UPDATE", so a request that names no
            // `ChangeSetType` plans against a stack that has to exist, exactly as an explicit
            // UPDATE does.
            case: "a change set planning an update, with the type left to its default",
            action: "CreateChangeSet",
            extra: vec![
                member("ChangeSetName", "wording-set-default"),
                member("TemplateBody", TEMPLATE),
            ],
        },
        Call {
            case: "a change set planning an update, with the type stated",
            action: "CreateChangeSet",
            extra: vec![
                member("ChangeSetName", "wording-set-stated"),
                member("ChangeSetType", "UPDATE"),
                member("TemplateBody", TEMPLATE),
            ],
        },
    ]
}

/// `Stack [<name>] does not exist` — the sentence this suite is about, spelled out here
/// rather than imported from the crate, because the sentence IS the requirement.
fn bracketed(name: &str) -> String {
    format!("Stack [{name}] does not exist")
}

/// The HTTP status, `<Code>` and `<Message>` of one call's envelope, with the body for the
/// assertion messages.
async fn answer(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(String, String)],
) -> (u16, Option<String>, Option<String>, String) {
    let (status, body) = call_in(router, region, action, members).await;
    let code = xml_text(&body, "Code");
    let message = xml_text(&body, "Message");
    (status, code, message, body)
}

#[tokio::test]
async fn a_stack_log_or_change_set_read_brackets_the_name_it_could_not_resolve() {
    let router = router();
    // A stack that was created and deleted, and a live one — the deleted name and the live
    // stack's id are two of the identifiers the rows name, and the live stack is also what the
    // control at the end calls against.
    deploy(&router, "wording-gone", TEMPLATE).await;
    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "wording-gone")]).await;
    assert_eq!(
        status, 200,
        "expected DeleteStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;
    deploy(&router, "wording-live", TEMPLATE).await;
    let (status, described) = describe_stack(&router, "wording-live").await;
    assert_eq!(
        status, 200,
        "expected the live stack to be readable, got {status} {described}"
    );
    let live_id = xml_text(&described, "StackId").expect("the live stack's own id");

    // A stack id of the right shape that no stack carries: the Region and account are this
    // emulator's, and the uuid is one nothing minted.
    let ghost_id = format!(
        "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/wording-ghost/\
         00000000-0000-0000-0000-000000000000"
    );
    // The identifiers differ along what the rule generalises over: the FORM of the name (a
    // label, a stack id), the STATE of what it names (never created, created and deleted,
    // live but out of Region), its CASE, and its LENGTH at the shortest the member's pattern
    // admits (`StackNameOrId` is `^([a-zA-Z][-a-zA-Z0-9]*)|(arn:…)$`, so one letter is a
    // name).
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a name nothing created", "wording-absent", REGION),
        ("the name of a deleted stack", "wording-gone", REGION),
        ("a live stack's name in another case", "WORDING-LIVE", REGION),
        ("a stack id no stack carries", ghost_id.as_str(), REGION),
        (
            "a live stack's id addressed from another Region",
            live_id.as_str(),
            OTHER_REGION,
        ),
        ("the shortest name the pattern admits", "z", REGION),
    ];

    for (identifier_case, identifier, region) in cases {
        // What the operations really answered for this identifier, so the table is shown to
        // be ONE template across them rather than six sentences that each happen to match.
        let mut worded: BTreeSet<String> = BTreeSet::new();
        for row in bracketing_calls() {
            let mut members = vec![member("StackName", identifier)];
            members.extend_from_slice(&row.extra);
            let (status, code, message, body) = answer(&router, region, row.action, &members).await;
            assert_eq!(
                status, 400,
                "case {identifier_case:?} on {} ({}): expected HTTP 400, got {status} {body}",
                row.action, row.case
            );
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {identifier_case:?} on {} ({}): expected <Code>ValidationError</Code>, got \
                 {code:?}",
                row.action,
                row.case
            );
            let expected = bracketed(identifier);
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {identifier_case:?} on {} ({}): expected {expected:?}, got {message:?}",
                row.action,
                row.case
            );
            worded.insert(message.unwrap_or_default());
        }
        assert_eq!(
            worded.len(),
            1,
            "case {identifier_case:?}: expected one shared sentence across these operations, got \
             {worded:?}"
        );

        // The template belongs to the OPERATION: the reads of the stack's own record and of
        // its resources answer the same class with a different sentence, so one service-wide
        // message cannot satisfy both sides.
        for with_id_action in ["DescribeStacks", "DescribeStackResources", "GetTemplate"] {
            let members = vec![member("StackName", identifier)];
            let (status, code, message, body) =
                answer(&router, region, with_id_action, &members).await;
            assert_eq!(
                status, 400,
                "case {identifier_case:?} on {with_id_action}: expected HTTP 400, got {status} \
                 {body}"
            );
            assert_eq!(
                code.as_deref(),
                Some("ValidationError"),
                "case {identifier_case:?} on {with_id_action}: expected a ValidationError, got \
                 {code:?}"
            );
            let expected = format!("Stack with id {identifier} does not exist");
            assert_eq!(
                message.as_deref(),
                Some(expected.as_str()),
                "case {identifier_case:?} on {with_id_action}: expected {expected:?}, got \
                 {message:?}"
            );
        }
    }

    // The refused `CreateChangeSet` calls minted nothing: the name they could not resolve is
    // still unresolvable, so the refusal was an answer and not a side effect.
    let (status, code, message, body) = answer(
        &router,
        REGION,
        "DescribeStacks",
        &[member("StackName", "wording-absent")],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"after the refusals\": expected the name to still resolve to nothing, got \
         {status} {body}"
    );
    assert_eq!(
        code.as_deref(),
        Some("ValidationError"),
        "case \"after the refusals\": expected a ValidationError, got {code:?}"
    );
    assert_eq!(
        message.as_deref(),
        Some("Stack with id wording-absent does not exist"),
        "case \"after the refusals\": expected the read's own template, got {message:?}"
    );
}

#[tokio::test]
async fn the_same_calls_against_a_live_stack_are_served() {
    let router = router();
    deploy(&router, "wording-served", TEMPLATE).await;
    let (status, described) = describe_stack(&router, "wording-served").await;
    assert_eq!(
        status, 200,
        "expected the stack to be readable, got {status} {described}"
    );
    let stack_id = xml_text(&described, "StackId").expect("the stack's own id");

    // Addressed by name and by stack id, since both are what `StackName` accepts: the rows
    // above were about the identifiers they named, not about the operations.
    for (pass, addressed) in [stack_id.as_str(), "wording-served"].into_iter().enumerate() {
        for (index, row) in bracketing_calls().into_iter().enumerate() {
            let mut members = vec![member("StackName", addressed)];
            for (key, value) in row.extra {
                // A change set is named within its stack, so each pass asks for one of its
                // own: what is under test here is that the call is SERVED, not how a repeated
                // name is answered.
                if key == "ChangeSetName" {
                    members.push(member(&key, &format!("{value}-{pass}{index}")));
                } else {
                    members.push((key, value));
                }
            }
            let (status, body) = call(&router, row.action, &members).await;
            assert_eq!(
                status, 200,
                "case {:?} addressed by {addressed:?}: expected {} to be served, got {status} \
                 {body}",
                row.case, row.action
            );
        }
    }
}
