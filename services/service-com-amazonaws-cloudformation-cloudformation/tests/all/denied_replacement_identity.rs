// SPDX-License-Identifier: CC0-1.0

//! What a stack policy's refusal leaves behind, end to end over the awsQuery wire — read
//! back through the API of the service that owns the protected resource.
//!
//! The rule under test: an update action the stack policy in force does not permit is not
//! PERFORMED, so the resource it was planned for keeps the identity it already has and the
//! service that owns it holds nothing under the declaration the refused update asked for;
//! the change the same update planned for a resource the policy does permit is not left
//! behind either, the stack having rolled back to the configuration in force. Once the
//! policy permits the action, the byte-identical update carries it out: a replacement mints
//! a new identity for the resource and the owning service holds the new declaration.
//!
//! `stack_policy_enforcement.rs` pins the VERDICT — which action a plan is held to, the
//! status the stack settles at and the event that attributes the refusal. This suite pins
//! the consequence of that verdict for the resource itself, which is the whole point of
//! setting a policy: the rows span the ways a statement can name the replacement it refuses
//! (the resource by logical id, every resource by wildcard, and a statement conditioned on
//! the resource's type), beside the policy that permits it — the one row that must change
//! the resource.
//!
//! An `AWS::SNS::Topic` is the protected resource: its `TopicName` is the property the topic
//! takes its name from, so an update that renames it is a REPLACEMENT, and the topic is read
//! back through Amazon SNS's own wire.

use crate::stack_lifecycle::{
    call, deploy, describe_stack, member, router, sns, wait_for_the_transition, xml_text,
    xml_texts,
};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

/// The blanket permission every documented stack policy that means to allow anything
/// carries, which the rows that refuse ONE action sit behind.
const ALLOW_EVERYTHING: &str =
    r#"{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}"#;

/// Whether the row's policy lets the planned replacement happen.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The policy refuses it: nothing about either resource changes.
    Refused,
    /// The policy permits it: the protected resource is replaced.
    Performed,
}

/// One row: the policy in force when the renaming update is issued.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The policy document, whose statements are in document order.
    policy: String,
    expected: Expected,
}

/// `{"Statement":[…]}` around the statements a row carries.
fn policy_of(statements: &[&str]) -> String {
    format!(r#"{{"Statement":[{}]}}"#, statements.join(","))
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a replacement refused for the resource named by its logical id",
            stack: "identity-named",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Replace","Principal":"*",
                    "Resource":"LogicalResourceId/Protected"}"#,
            ]),
            expected: Expected::Refused,
        },
        Row {
            // The same refusal written over every resource of the stack.
            case: "a replacement refused for every resource by wildcard",
            stack: "identity-wildcard",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Replace","Principal":"*",
                    "Resource":"*"}"#,
            ]),
            expected: Expected::Refused,
        },
        Row {
            // "You can use the ResourceType condition key to apply a statement to resources
            // of a given type": the statement names no resource but the type of one.
            case: "a replacement refused by a statement conditioned on the resource type",
            stack: "identity-typed",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Replace","Principal":"*",
                    "Resource":"*",
                    "Condition":{"StringEquals":{"ResourceType":["AWS::SNS::Topic"]}}}"#,
            ]),
            expected: Expected::Refused,
        },
        Row {
            // The control: the one row whose policy permits the replacement, which is the
            // only one that may change the resource.
            case: "a policy that permits every update action",
            stack: "identity-permitted",
            policy: policy_of(&[ALLOW_EVERYTHING]),
            expected: Expected::Performed,
        },
    ]
}

/// The name the protected topic is created under.
fn created_name(stack: &str) -> String {
    format!("{stack}-protected")
}

/// The name the update asks for instead — the create-only property whose change makes the
/// update a replacement.
fn renamed(stack: &str) -> String {
    format!("{stack}-renamed")
}

/// The template: the protected topic under `name`, and an unprotected sibling whose
/// `DisplayName` the same update changes in place.
fn topic_template(stack: &str, name: &str, display_name: &str) -> String {
    format!(
        r#"{{"Resources":{{
             "Protected":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"{name}"}}}},
             "Open":{{"Type":"AWS::SNS::Topic",
               "Properties":{{"TopicName":"{stack}-open","DisplayName":"{display_name}"}}}}}}}}"#
    )
}

/// `SetStackPolicy` with a document, asserted to succeed.
async fn set_policy(router: &MultiServiceRouter, case: &str, stack: &str, policy: &str) {
    let (http_status, body) = call(
        router,
        "SetStackPolicy",
        &[
            member("StackName", stack),
            member("StackPolicyBody", policy),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected SetStackPolicy to succeed, got {http_status} {body}"
    );
}

/// `UpdateStack` with a template body, asserted to be ACCEPTED: the policy's verdict is
/// asynchronous, so a refused update is answered 200 all the same.
async fn update_stack(router: &MultiServiceRouter, case: &str, stack: &str, body: &str) {
    let (http_status, answered) = call(
        router,
        "UpdateStack",
        &[member("StackName", stack), member("TemplateBody", body)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected UpdateStack to be accepted, got {http_status} {answered}"
    );
}

/// The `StackStatus` a stack reports now.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (http_status, body) = describe_stack(router, stack).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {http_status} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The physical id the stack reports for one of its resources.
async fn physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (http_status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected DescribeStackResource({logical_id}) to succeed, got \
         {http_status} {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// Whether Amazon SNS holds a topic of this name, read through SNS's own wire: a topic ARN
/// ends with the name the topic was created under.
async fn sns_holds_a_topic_named(router: &MultiServiceRouter, case: &str, name: &str) -> bool {
    let (http_status, body) = sns(router, "ListTopics", &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected ListTopics to succeed, got {http_status} {body}"
    );
    xml_texts(&body, "TopicArn")
        .iter()
        .any(|arn| arn.ends_with(&format!(":{name}")))
}

/// The `DisplayName` Amazon SNS reports for a topic, empty where it reports none.
async fn display_name(router: &MultiServiceRouter, case: &str, topic_arn: &str) -> String {
    let (http_status, body) = sns(router, "GetTopicAttributes", &[("TopicArn", topic_arn)]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected GetTopicAttributes({topic_arn}) to succeed, got {http_status} \
         {body}"
    );
    xml_texts(&body, "key")
        .into_iter()
        .zip(xml_texts(&body, "value"))
        .find(|(key, _)| key == "DisplayName")
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// Wait past the instant the update's terminal status becomes due. Two spans, because the
/// rollback of a refused update is a transition of its own.
async fn wait_for_the_update() {
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// Assert the replacement HAPPENED: the stack completed, the protected resource carries a
/// new identity under the name the update asked for, Amazon SNS holds that topic, and the
/// sibling's in-place change is in force.
async fn assert_the_replacement_happened(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    before: &str,
    open_arn: &str,
) {
    let reported = stack_status(router, case, stack).await;
    assert_eq!(
        reported, status::UPDATE_COMPLETE,
        "case {case:?}: expected the permitted update to complete, got {reported:?}"
    );
    let after = physical_id(router, case, stack, "Protected").await;
    assert_ne!(
        after, before,
        "case {case:?}: expected the replacement to mint a new identity, got {after:?}"
    );
    let wanted = renamed(stack);
    assert!(
        after.ends_with(&format!(":{wanted}")),
        "case {case:?}: expected the new identity to carry {wanted:?}, got {after:?}"
    );
    assert!(
        sns_holds_a_topic_named(router, case, &wanted).await,
        "case {case:?}: expected Amazon SNS to hold the topic {wanted:?} the update asked for"
    );
    let reported = display_name(router, case, open_arn).await;
    assert_eq!(
        reported, "second",
        "case {case:?}: expected the sibling's in-place change to be in force, got \
         {reported:?}"
    );
}

#[tokio::test]
async fn a_replacement_a_stack_policy_refuses_leaves_the_resource_as_it_was() {
    let router = router();

    for row in rows() {
        let created = topic_template(row.stack, &created_name(row.stack), "first");
        deploy(&router, row.stack, &created).await;
        let before = physical_id(&router, row.case, row.stack, "Protected").await;
        let open_arn = physical_id(&router, row.case, row.stack, "Open").await;
        set_policy(&router, row.case, row.stack, &row.policy).await;

        // The one update every row issues: the protected topic renamed — a change to the
        // property it takes its name from, so a replacement — and the sibling's
        // `DisplayName` changed in place.
        let updated = topic_template(row.stack, &renamed(row.stack), "second");
        update_stack(&router, row.case, row.stack, &updated).await;
        wait_for_the_update().await;

        match row.expected {
            Expected::Performed => {
                assert_the_replacement_happened(
                    &router, row.case, row.stack, &before, &open_arn,
                )
                .await;
            }
            Expected::Refused => {
                let reported = stack_status(&router, row.case, row.stack).await;
                assert_eq!(
                    reported,
                    status::UPDATE_ROLLBACK_COMPLETE,
                    "case {:?}: expected the refused update to roll the stack back, got \
                     {reported:?}",
                    row.case
                );
                // Nothing was done to the protected resource, so it carries the identity it
                // already had.
                let after = physical_id(&router, row.case, row.stack, "Protected").await;
                assert_eq!(
                    after, before,
                    "case {:?}: expected the protected resource to keep its identity, got \
                     {after:?}",
                    row.case
                );
                // And the service that owns it holds the declaration in force, not the one
                // the refused update asked for.
                let kept = created_name(row.stack);
                assert!(
                    sns_holds_a_topic_named(&router, row.case, &kept).await,
                    "case {:?}: expected Amazon SNS to still hold the topic {kept:?}",
                    row.case
                );
                let asked = renamed(row.stack);
                assert!(
                    !sns_holds_a_topic_named(&router, row.case, &asked).await,
                    "case {:?}: expected Amazon SNS to hold no topic {asked:?}, the refused \
                     replacement never having been performed",
                    row.case
                );
                // The change the same update planned for the resource the policy DOES permit
                // is not left behind either: the stack rolled back to the configuration in
                // force.
                let reported = display_name(&router, row.case, &open_arn).await;
                assert_eq!(
                    reported, "first",
                    "case {:?}: expected the sibling's change to be rolled back, got \
                     {reported:?}",
                    row.case
                );

                // Once the policy permits the action, the byte-identical update performs it.
                set_policy(
                    &router,
                    row.case,
                    row.stack,
                    &policy_of(&[ALLOW_EVERYTHING]),
                )
                .await;
                update_stack(&router, row.case, row.stack, &updated).await;
                wait_for_the_update().await;
                assert_the_replacement_happened(
                    &router, row.case, row.stack, &before, &open_arn,
                )
                .await;
            }
        }
    }
}
