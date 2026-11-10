// SPDX-License-Identifier: CC0-1.0

//! How this service answers a stack identifier written as a stack's UNIQUE ID — the segment
//! a stack id (`arn:…:stack/<name>/<unique-id>`) ends in — over the awsQuery wire.
//!
//! **The rule every case here holds the service to:** the identifier forms CloudFormation
//! admits are the forms a stack is ADDRESSED by — a stack name, a stack id, and the unique id
//! that stack id ends in — so a value in one of those forms reaches the stack lookup and a
//! request naming no stack is answered as the missing stack it is, in each operation's own
//! wording; only a value in none of those forms is refused as a malformed request, with the
//! constraint envelope naming the member path the operation states.
//!
//! A unique id is hexadecimal, so it starts with a digit as often as not and is outside the
//! stack-NAME expression: that is what makes it the case the name expression alone gets wrong.
//!
//! The tables span the class rather than one recorded request: the admitted side crosses three
//! unique ids differing along what the rule generalises over (a live stack's own, that same
//! value in the other case of hexadecimal, and one nothing ever minted) with every operation
//! whose identifier member this service checks itself and whose answer for an absent stack is
//! a refusal. The values that are only NEARLY a unique id — the group count, the group lengths
//! and the digits being what the form is — are rows of the malformed-identifier table in
//! `stack_identifier_refusals.rs`, which is the one suite that pins that refusal.

use crate::harness::{call, create_stack, member, router, wait_for_the_transition, xml_text};

/// A template with one resource, so every read a row makes has something to report for the
/// control cases.
const TEMPLATE: &str = r#"{"Resources":{
     "First":{"Type":"AWS::SNS::Topic","Properties":{"DisplayName":"unique-id-first"}}}}"#;

/// A stack policy body, for the row that reaches `SetStackPolicy`.
const POLICY: &str = r#"{"Statement":[{"Effect":"Allow","Action":"Update:*",
     "Principal":"*","Resource":"*"}]}"#;

/// `Stack with id <identifier> does not exist`, the wording the reads of a stack's own record
/// and of its resources answer.
fn with_id(identifier: &str) -> String {
    format!("Stack with id {identifier} does not exist")
}

/// `Stack '<identifier>' does not exist`, `DescribeStackResource`'s wording.
fn quoted(identifier: &str) -> String {
    format!("Stack '{identifier}' does not exist")
}

/// `Stack [<identifier>] does not exist`, the wording the read of a stack's event log answers.
fn bracketed(identifier: &str) -> String {
    format!("Stack [{identifier}] does not exist")
}

/// `Stack: <identifier> does not exist`, the wording the stack-policy read and
/// `CancelUpdateStack` answer.
fn labelled(identifier: &str) -> String {
    format!("Stack: {identifier} does not exist")
}

/// One operation that takes a stack identifier this service checks itself: the members it
/// needs besides `StackName`, the not-found wording it answers for a stack that is not
/// addressable, and whether a request naming a LIVE stack is served (a mutation is left out of
/// that control, since it would move the stack the other rows read).
struct UniqueIdRow {
    action: &'static str,
    extra: &'static [(&'static str, &'static str)],
    not_found: fn(&str) -> String,
    serves_a_live_stack: bool,
}

const ROWS: &[UniqueIdRow] = &[
    UniqueIdRow {
        action: "ListStackResources",
        extra: &[],
        not_found: with_id,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "DescribeStackResources",
        extra: &[],
        not_found: with_id,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "DescribeStacks",
        extra: &[],
        not_found: with_id,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "GetTemplate",
        extra: &[],
        not_found: with_id,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "DescribeStackResource",
        extra: &[("LogicalResourceId", "First")],
        not_found: quoted,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "DescribeStackEvents",
        extra: &[],
        not_found: bracketed,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "GetStackPolicy",
        extra: &[],
        not_found: labelled,
        serves_a_live_stack: true,
    },
    UniqueIdRow {
        action: "CancelUpdateStack",
        extra: &[],
        not_found: labelled,
        serves_a_live_stack: false,
    },
    // Two operations that WRITE: an identifier that resolves to nothing is refused by them
    // too, so the rule holds for the mutating reads of a stack as well as the reporting
    // ones. Neither is crossed with the live stack, since a write would move the stack the
    // rows above read.
    UniqueIdRow {
        action: "SetStackPolicy",
        extra: &[("StackPolicyBody", POLICY)],
        not_found: labelled,
        serves_a_live_stack: false,
    },
    UniqueIdRow {
        action: "UpdateStack",
        extra: &[("TemplateBody", TEMPLATE)],
        not_found: bracketed,
        serves_a_live_stack: false,
    },
];

/// A unique id nothing ever minted, in the 8-4-4-4-12 hexadecimal form a stack id ends in.
const UNMINTED_UNIQUE_ID: &str = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

fn members_of(row: &UniqueIdRow, identifier: &str) -> Vec<(String, String)> {
    let mut members = vec![member("StackName", identifier)];
    members.extend(row.extra.iter().map(|(key, value)| member(key, value)));
    members
}

/// The unique id a stack id ends in: everything after the last `/`.
fn unique_id_of(stack_id: &str) -> String {
    stack_id
        .rsplit('/')
        .next()
        .expect("a stack id's last segment")
        .to_string()
}

#[tokio::test]
async fn a_stack_unique_id_reaches_the_lookup_and_is_answered_as_a_missing_stack() {
    let router = router();
    let (status, body) = create_stack(&router, "unique-id-live", TEMPLATE, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to be accepted, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, described) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "unique-id-live")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the stack to be readable, got {status} {described}"
    );
    let stack_id = xml_text(&described, "StackId").expect("the stack's own id");
    let unique_id = unique_id_of(&stack_id);

    // The unique id of the live stack, that same value in the other case of hexadecimal, and
    // one nothing ever minted: each is an addressable FORM, so each reaches the lookup — and
    // none of them addresses a stack, since a stack answers to its name and to its whole
    // stack id.
    let upper = unique_id.to_ascii_uppercase();
    for (case, identifier) in [
        ("the live stack's own unique id", unique_id.as_str()),
        ("that unique id in upper-case hexadecimal", upper.as_str()),
        ("a unique id nothing ever minted", UNMINTED_UNIQUE_ID),
    ] {
        for row in ROWS {
            let case = format!("{} naming {case}", row.action);
            let (status, body) = call(&router, row.action, &members_of(row, identifier)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected <Code>ValidationError</Code>, got {body}"
            );
            let expected = (row.not_found)(identifier);
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {body}"
            );
        }
    }

    // ...and the forms that DO address the stack are still served, so admitting the unique id
    // costs none of them.
    for (case, identifier) in [
        ("its name", "unique-id-live"),
        ("its whole stack id", stack_id.as_str()),
    ] {
        for row in ROWS {
            if !row.serves_a_live_stack {
                continue;
            }
            let case = format!("{} naming {case}", row.action);
            let (status, body) = call(&router, row.action, &members_of(row, identifier)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be served, got {status} {body}"
            );
        }
    }
}
