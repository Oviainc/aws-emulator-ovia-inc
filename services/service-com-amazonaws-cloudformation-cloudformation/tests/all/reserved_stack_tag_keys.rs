// SPDX-License-Identifier: CC0-1.0

//! The tag keys a CALLER may put on a stack, a change set or a stack set, over the awsQuery
//! wire.
//!
//! **The rule every case here holds the service to: a `Tags` key under the prefix AWS
//! reserves for the tags it owns is not a tag a caller may set — every operation that takes a
//! `Tags` list refuses such a request with HTTP 400 `ValidationError` and the one fixed
//! sentence, writes nothing, and admits every key outside that prefix, which is stored and
//! read back as sent.**
//!
//! `Tag$Key` is "a string used to identify this tag ... Tags owned by Amazon Web Services
//! have the reserved prefix: `aws:`"
//! (`smithy_query(query="shape", shape="Tag")`), and CloudFormation itself writes keys in that
//! namespace onto the resources a stack provisions — so the namespace belongs to the service,
//! not to its callers.
//!
//! The rows span what the rule generalises over: the FIVE operations whose input carries a
//! `Tags` list (every one of them, so none can drift from the others), the keys in the
//! reserved namespace (one with a name after the prefix, one spelling a key the service
//! itself writes, the bare prefix on its own, the prefix in another letter case — the
//! namespace is AWS's whichever case a request spells it in, as every tag-refusing service of
//! this emulator matches it), WHERE in the list the offending key sits (first, and after an
//! ordinary key), and the keys that only RESEMBLE the prefix (the letters without the colon
//! that makes it, the prefix away from the start).
//!
//! The sentence is spelled out here rather than imported from the crate, because the sentence
//! IS the requirement: a reworded message must fail this suite.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_text,
    xml_texts,
};

/// The refusal a caller-supplied key in the reserved namespace is answered with: entirely
/// fixed text, naming neither the key nor the operation.
const RESERVED_KEY: &str = "aws: prefixed tag key names are not allowed for external use.";

/// The template every case here deploys: one topic, really created in Amazon SNS.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"reserved-tag-keys"}}}}"#;

/// A second revision of it, so an UPDATE has something to change.
const REVISED: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"reserved-tag-keys-revised"}}}}"#;

const STACK: &str = "reserved-tag-keys";
const STACK_SET: &str = "reserved-tag-key-set";
const CHANGE_SET: &str = "reserved-tag-key-plan";

/// The operations whose input carries a caller's `Tags` list — every one this service models,
/// so the rule is asserted over the whole surface it holds for rather than over the one
/// operation a report happened to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TakesTags {
    CreateStack,
    UpdateStack,
    CreateChangeSet,
    CreateStackSet,
    UpdateStackSet,
}

const TAKES_TAGS: &[TakesTags] = &[
    TakesTags::CreateStack,
    TakesTags::UpdateStack,
    TakesTags::CreateChangeSet,
    TakesTags::CreateStackSet,
    TakesTags::UpdateStackSet,
];

/// Keys in the reserved namespace, each paired with the ordinary keys it is sent beside: the
/// refusal must fire wherever in the list the offending key sits.
const REFUSED_LISTS: &[(&str, &[&str])] = &[
    ("the reserved prefix with a name after it", &["aws:mine"]),
    (
        "a key the service itself writes onto a stack's resources",
        &["aws:cloudformation:stack-name"],
    ),
    ("the bare reserved prefix as the whole key", &["aws:"]),
    (
        "the reserved prefix in upper case, which claims the same namespace",
        &["AWS:mine"],
    ),
    (
        "the reserved prefix in mixed case",
        &["Aws:cloudformation:logical-id"],
    ),
    (
        "a reserved key AFTER an ordinary one, so the list is scanned past its first entry",
        &["team", "aws:mine"],
    ),
];

/// Keys that only RESEMBLE the reserved prefix, which are ordinary keys of the caller's own.
const ADMITTED_KEYS: &[(&str, &str)] = &[
    ("the letters without the colon that makes the prefix", "aws"),
    ("the prefix away from the start of the key", "team-aws:mine"),
    ("a colon that opens no reserved namespace", "aw:s"),
    ("an ordinary key", "Name"),
];

/// The `Tags.member.N.Key` / `.Value` members one list of keys is sent as.
fn tag_members(keys: &[&str]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        members.push(member(&format!("Tags.member.{}.Key", index + 1), key));
        members.push(member(&format!("Tags.member.{}.Value", index + 1), "set"));
    }
    members
}

/// The stack every row that mutates one acts on, created with no tags and settled.
async fn settled_stack(router: &MultiServiceRouter) {
    let (status_code, body) = create_stack(router, STACK, TEMPLATE, &[]).await;
    assert_eq!(
        status_code, 200,
        "the stack under test must be created, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// The stack set every row that mutates one acts on, created with no tags.
async fn stack_set(router: &MultiServiceRouter) {
    let (status_code, body) = call(
        router,
        "CreateStackSet",
        &[
            member("StackSetName", STACK_SET),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the stack set under test must be created, got {status_code} {body}"
    );
}

/// Send `keys` as the `Tags` of `op` on a fresh router, answering that router and the HTTP
/// status and body of the call that carried the tags.
async fn tags_through(
    op: TakesTags,
    keys: &[&str],
) -> (MultiServiceRouter, u16, String) {
    let router = router();
    let tags = tag_members(keys);
    let (status_code, body) = match op {
        TakesTags::CreateStack => create_stack(&router, STACK, TEMPLATE, &tags).await,
        TakesTags::UpdateStack => {
            settled_stack(&router).await;
            let mut members = vec![
                member("StackName", STACK),
                member("TemplateBody", REVISED),
            ];
            members.extend(tags);
            call(&router, "UpdateStack", &members).await
        }
        TakesTags::CreateChangeSet => {
            settled_stack(&router).await;
            let mut members = vec![
                member("StackName", STACK),
                member("ChangeSetName", CHANGE_SET),
                member("TemplateBody", REVISED),
            ];
            members.extend(tags);
            call(&router, "CreateChangeSet", &members).await
        }
        TakesTags::CreateStackSet => {
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("TemplateBody", TEMPLATE),
            ];
            members.extend(tags);
            call(&router, "CreateStackSet", &members).await
        }
        TakesTags::UpdateStackSet => {
            stack_set(&router).await;
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("TemplateBody", REVISED),
            ];
            members.extend(tags);
            call(&router, "UpdateStackSet", &members).await
        }
    };
    (router, status_code, body)
}

/// The tag keys the resource `op` acts on carries now, read back through the operation that
/// reports it — empty for a resource the refused call would have created, since nothing was
/// written at all.
async fn stored_tag_keys(router: &MultiServiceRouter, op: TakesTags) -> Vec<String> {
    let (status_code, body) = match op {
        TakesTags::CreateStack | TakesTags::UpdateStack => describe_stack(router, STACK).await,
        TakesTags::CreateChangeSet => {
            call(
                router,
                "DescribeChangeSet",
                &[
                    member("ChangeSetName", CHANGE_SET),
                    member("StackName", STACK),
                ],
            )
            .await
        }
        TakesTags::CreateStackSet | TakesTags::UpdateStackSet => {
            call(
                router,
                "DescribeStackSet",
                &[member("StackSetName", STACK_SET)],
            )
            .await
        }
    };
    if status_code != 200 {
        // The record the refused call would have created is not there at all, which is the
        // strongest form of "nothing was written".
        return Vec::new();
    }
    xml_texts(&body, "Key")
}

#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_by_every_operation_that_takes_tags() {
    for op in TAKES_TAGS {
        for (case, keys) in REFUSED_LISTS {
            let (router, status_code, body) = tags_through(*op, keys).await;
            assert_eq!(
                status_code, 400,
                "case {op:?} {case:?}: expected the call to be refused, got {status_code} \
                 {body}"
            );
            assert_eq!(
                (
                    xml_text(&body, "Code").unwrap_or_default(),
                    xml_text(&body, "Message").unwrap_or_default()
                ),
                ("ValidationError".to_string(), RESERVED_KEY.to_string()),
                "case {op:?} {case:?}: expected the reserved-namespace refusal, got {body}"
            );
            // A refused request writes nothing: not the reserved key, and not the ordinary
            // key beside it either.
            let stored = stored_tag_keys(&router, *op).await;
            assert!(
                stored.is_empty(),
                "case {op:?} {case:?}: a refused request must write no tag, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_key_outside_the_reserved_namespace_is_admitted_by_every_operation_that_takes_tags() {
    for op in TAKES_TAGS {
        for (case, key) in ADMITTED_KEYS {
            let (_, status_code, body) = tags_through(*op, &[key]).await;
            assert_eq!(
                status_code, 200,
                "case {op:?} {case:?}: expected the key {key:?} to be admitted, got \
                 {status_code} {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_admitted_key_is_stored_as_sent() {
    // Read back through the two operations that REPORT a caller's tags: a stack's, and a
    // stack set's. The key is stored exactly as the request spelled it — a key that resembles
    // the reserved prefix is not rewritten on its way in.
    for (case, key) in ADMITTED_KEYS {
        for op in [TakesTags::CreateStack, TakesTags::CreateStackSet] {
            let (router, status_code, body) = tags_through(op, &[key]).await;
            assert_eq!(
                status_code, 200,
                "case {op:?} {case:?}: expected the create to be admitted, got {status_code} \
                 {body}"
            );
            let stored = stored_tag_keys(&router, op).await;
            assert_eq!(
                stored,
                vec![(*key).to_string()],
                "case {op:?} {case:?}: expected the key to be stored as sent, got {stored:?}"
            );
        }
    }
}
