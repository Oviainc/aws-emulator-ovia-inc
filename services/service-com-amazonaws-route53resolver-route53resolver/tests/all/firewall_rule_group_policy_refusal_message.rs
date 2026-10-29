// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the BODY of the refusal `PutFirewallRuleGroupPolicy`
//! answers for an IAM policy document it will not attach.
//!
//! The rule under test: the refusal carries the whole wording the service words
//! that fault with — its service error code, the fault sentence echoing the
//! submitted document byte for byte, AND the remediation sentence that states
//! what a document the service would attach has to do — so no sentence of it is
//! dropped, whichever class of document was refused and whatever the document
//! looks like.
//!
//! The table spans both classes of refused document (one outside the policy
//! grammar, one that shares the rule group with no account but its owner) and,
//! inside each, the forms a document takes: its type, its size, its internal
//! whitespace and the characters it carries. The recorded same-account share is
//! one row of it.
//!
//! The message is asserted in FULL: the service answers this refusal with the
//! code, the fault sentence and the remediation sentence and nothing else, so
//! no part of it is cut before the comparison.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::EMPTY_POLICY_DOCUMENT;

use super::harness::{call, emulator, error_type, ok, string_member, ACCOUNT};

/// An account that is not the one this emulator's resources belong to, so a
/// policy naming it really does share the rule group.
const OTHER_ACCOUNT: &str = "123456789012";

/// The whole message the service answers for a document it will not attach,
/// written out here rather than built from the emulator's own constants so the
/// wording itself is what this suite pins: the code in square brackets, the
/// fault sentence with the submitted document double-quoted inside it, and the
/// remediation sentence.
fn expected_message(policy: &str) -> String {
    format!(
        "[RSLVR-02035] The specified IAM policy isn't valid: \"{policy}\". Provide a valid policy \
         that allows a DNS Firewall rule group to be shared with another AWS account."
    )
}

/// A document that shares `arn` with `OTHER_ACCOUNT`, which the service does
/// attach: the policy every row below has to leave in place.
fn sharing_document(arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": OTHER_ACCOUNT },
            "Action": "route53resolver:GetFirewallRuleGroup",
            "Resource": arn,
        }],
    })
    .to_string()
}

/// The documents the write refuses, and what makes each one refusable.
///
/// Rows 1-5 miss the policy grammar; rows 6-10 are documents the grammar admits
/// that share the group with nobody but its owner. Within each class they
/// differ in the document's JSON type, its size, its internal whitespace and
/// the characters it carries.
fn refused_documents(arn: &str) -> Vec<(&'static str, String)> {
    vec![
        ("the empty string", String::new()),
        ("not JSON at all", "this is not a policy".to_string()),
        ("a JSON number", "17".to_string()),
        (
            "a JSON string carrying a quote and a non-ASCII character",
            json!("Allow \"everyone\" — really").to_string(),
        ),
        (
            "a statement that is not an object",
            json!({ "Version": "2012-10-17", "Statement": ["route53resolver:*"] }).to_string(),
        ),
        (
            "a document with no Statement element",
            EMPTY_POLICY_DOCUMENT.to_string(),
        ),
        (
            "the owner account named as a principal, pretty-printed over several lines",
            format!(
                "{{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {{\n      \
                 \"Effect\": \"Allow\",\n      \"Principal\": {{ \"AWS\": \
                 \"arn:aws:iam::{ACCOUNT}:root\" }},\n      \"Action\": \
                 \"route53resolver:GetFirewallRuleGroup\",\n      \"Resource\": \"{arn}\"\n    \
                 }}\n  ]\n}}\n"
            ),
        ),
        (
            "the owner account named as a principal, with tabs between the elements",
            format!(
                "{{\t\"Version\":\t\"2012-10-17\",\t\"Statement\":\t[{{\t\"Effect\":\t\"Allow\",\
                 \t\"Principal\":\t{{\t\"AWS\":\t\"{ACCOUNT}\"\t}},\t\"Action\":\t\
                 \"route53resolver:ListFirewallRuleGroups\",\t\"Resource\":\t\"{arn}\"\t}}]\t}}"
            ),
        ),
        ("an empty statement list", {
            json!({ "Version": "2012-10-17", "Statement": [] }).to_string()
        }),
        (
            "many statements, every one of them for the owner account",
            json!({
                "Version": "2012-10-17",
                "Statement": Value::Array(
                    (0..60)
                        .map(|index| {
                            json!({
                                "Sid": format!("OwnerOnly{index}"),
                                "Effect": "Allow",
                                "Principal": { "AWS": ACCOUNT },
                                "Action": "route53resolver:GetFirewallRuleGroup",
                                "Resource": arn,
                            })
                        })
                        .collect(),
                ),
            })
            .to_string(),
        ),
    ]
}

/// Every refused document is answered with the whole two-sentence message, the
/// document echoed inside it exactly as it was submitted, and the rule group
/// keeps the policy it already carried.
#[tokio::test]
async fn a_refused_policy_document_is_answered_with_the_whole_message() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateFirewallRuleGroup",
        json!({ "CreatorRequestId": "policy-message", "Name": "policy-message" }),
    )
    .await;
    let group = created
        .get("FirewallRuleGroup")
        .expect("the create answers a FirewallRuleGroup");
    let arn = string_member(group, "Arn");

    // The policy the group carries while every row below is refused.
    let attached = sharing_document(&arn);
    let written = ok(
        &router,
        "PutFirewallRuleGroupPolicy",
        json!({ "Arn": arn, "FirewallRuleGroupPolicy": attached }),
    )
    .await;
    assert_eq!(
        written.get("ReturnValue"),
        Some(&Value::Bool(true)),
        "a document sharing the group with another account must be attached, got {written}"
    );

    for (case, policy) in refused_documents(&arn) {
        let message = refusal_message(&router, case, &arn, &policy).await;
        let expected = expected_message(&policy);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );

        let read = ok(&router, "GetFirewallRuleGroupPolicy", json!({ "Arn": arn })).await;
        let stored = string_member(&read, "FirewallRuleGroupPolicy");
        assert_eq!(
            stored, attached,
            "case {case:?}: a refused document must leave the stored policy alone, got {stored:?}"
        );
    }
}

/// Read the refusal `PutFirewallRuleGroupPolicy` answers for `policy`,
/// requiring the status and the error shape the service refuses a document it
/// will not attach with.
async fn refusal_message(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    arn: &str,
    policy: &str,
) -> String {
    let (status, refusal) = call(
        router,
        "PutFirewallRuleGroupPolicy",
        json!({ "Arn": arn, "FirewallRuleGroupPolicy": policy }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the write must answer HTTP 400, got {status}: {refusal}"
    );
    let answered = error_type(&refusal);
    assert!(
        answered.contains("ValidationException"),
        "case {case:?}: the write must answer ValidationException, got {answered:?}: {refusal}"
    );
    string_member(&refusal, "Message")
}

/// The message states the document it was handed and nothing else from the
/// request: two groups refusing the same document answer the same wording, and
/// one group refusing two documents answers two messages differing only in the
/// echoed document.
#[tokio::test]
async fn the_echoed_document_is_the_messages_only_variable_part() {
    let router = emulator();

    // Two groups, so the refusal cannot be carrying the group it was asked
    // about: the ARNs differ, the documents do not.
    let mut arns = Vec::new();
    for name in ["message-group-a", "message-group-b"] {
        let created = ok(
            &router,
            "CreateFirewallRuleGroup",
            json!({ "CreatorRequestId": name, "Name": name }),
        )
        .await;
        let group = created
            .get("FirewallRuleGroup")
            .expect("the create answers a FirewallRuleGroup");
        arns.push(string_member(group, "Arn"));
    }

    // Each group carries a document the service DID attach, so the read-back
    // below tells a refusal that persisted nothing apart from a store that
    // never held anything.
    for arn in &arns {
        let written = ok(
            &router,
            "PutFirewallRuleGroupPolicy",
            json!({ "Arn": arn, "FirewallRuleGroupPolicy": sharing_document(arn) }),
        )
        .await;
        assert_eq!(
            written.get("ReturnValue"),
            Some(&Value::Bool(true)),
            "a document sharing {arn} with another account must be attached, got {written}"
        );
    }

    let shared_case = "a document that shares the group with nobody, refused for two groups";
    let document = json!({ "Version": "2012-10-17" }).to_string();
    let mut messages = Vec::new();
    for arn in &arns {
        messages.push(refusal_message(&router, shared_case, arn, &document).await);
    }
    assert_eq!(
        messages[0], messages[1],
        "case {shared_case:?}: the wording must not depend on the group, got {messages:?}"
    );
    assert_eq!(
        messages[0],
        expected_message(&document),
        "case {shared_case:?}: got {:?}",
        messages[0]
    );

    // The same group, two documents: only the echoed document differs, so
    // replacing it in one message yields the other.
    let case = "two documents refused for one group";
    let other_document = "not a policy at all".to_string();
    let other_message = refusal_message(&router, case, &arns[0], &other_document).await;
    assert_eq!(
        other_message,
        messages[0].replace(&document, &other_document),
        "case {case:?}: got {other_message:?}"
    );

    // No refusal attached anything: each group still reports the document that
    // was attached before them.
    for arn in &arns {
        let read = ok(&router, "GetFirewallRuleGroupPolicy", json!({ "Arn": arn })).await;
        let stored = string_member(&read, "FirewallRuleGroupPolicy");
        assert_eq!(
            stored,
            sharing_document(arn),
            "case {case:?}: a refused write must leave {arn}'s policy alone, got {stored:?}"
        );
    }
}
