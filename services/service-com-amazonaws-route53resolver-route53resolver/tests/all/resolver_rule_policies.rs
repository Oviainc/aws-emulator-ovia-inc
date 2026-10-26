// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver rule policy slice:
//! `PutResolverRulePolicy` and `GetResolverRulePolicy`.
//!
//! The rule under test: a resource policy is attached to the Resolver rule its
//! `Arn` names and is reported back by a read of that rule's policy, EXCEPT
//! when the document shares the rule with no account other than the one that
//! owns it — which shares the rule with nobody and so leaves it with no policy,
//! reported as the document that grants nothing. Each suite drives a TABLE of
//! documents spanning the forms the policy grammar admits, so no case is tied
//! to one recorded document.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::state::EMPTY_POLICY_DOCUMENT;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, emulator, error_type, expect_error, ok, string_member, ACCOUNT, REGION};

/// An account that is not the one this emulator's resources belong to, so a
/// policy naming it really does share the rule.
const OTHER_ACCOUNT: &str = "123456789012";

/// Create a SYSTEM Resolver rule and return `(Id, Arn)`. A SYSTEM rule needs no
/// Resolver endpoint and no target addresses, so a policy case needs nothing
/// else stored.
async fn system_rule(router: &MultiServiceRouter, creator: &str, domain: &str) -> (String, String) {
    let created = ok(
        router,
        "CreateResolverRule",
        json!({
            "CreatorRequestId": creator,
            "Name": creator,
            "RuleType": "SYSTEM",
            "DomainName": domain,
        }),
    )
    .await;
    let rule = created
        .get("ResolverRule")
        .expect("the create answers a ResolverRule");
    (string_member(rule, "Id"), string_member(rule, "Arn"))
}

/// The `ResolverRulePolicy` a read of `arn` reports.
async fn stored_policy(router: &MultiServiceRouter, arn: &str) -> String {
    let read = ok(router, "GetResolverRulePolicy", json!({ "Arn": arn })).await;
    string_member(&read, "ResolverRulePolicy")
}

/// Write `policy` for `arn` and assert the call reported success.
async fn put_policy(router: &MultiServiceRouter, case: &str, arn: &str, policy: &str) {
    let written = ok(
        router,
        "PutResolverRulePolicy",
        json!({ "Arn": arn, "ResolverRulePolicy": policy }),
    )
    .await;
    assert_eq!(
        written.get("ReturnValue"),
        Some(&Value::Bool(true)),
        "case {case:?}: PutResolverRulePolicy must report ReturnValue true, got {written}"
    );
}

/// One statement sharing `arn` with `principal`, in the grammar's statement
/// form: an `Effect`, an `Action` list and the `Resource` the doc asks for
/// ("specify the ARN for the rule that you want to share with another account").
fn share_statement(principal: Value, arn: &str) -> Value {
    json!({
        "Effect": "Allow",
        "Principal": principal,
        "Action": ["route53resolver:GetResolverRule", "route53resolver:AssociateResolverRule"],
        "Resource": arn,
    })
}

/// A whole document carrying `statements`.
fn policy_document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A document that shares the rule with an account other than its owner is
/// stored as written, and the read reports exactly that document — whatever
/// form the grammar lets the document take.
#[tokio::test]
async fn a_shared_policy_is_reported_back_as_written() {
    let router = emulator();
    let (_, arn) = system_rule(&router, "policy-shared", "shared.example.com").await;

    // Each row is a different shape of sharing document: how the principal is
    // written, how many statements the document carries, and which optional
    // elements it states.
    let cases: Vec<(&str, String)> = vec![
        (
            "principal by account id",
            policy_document(json!([share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)])),
        ),
        (
            "principal by account root ARN",
            policy_document(json!([share_statement(
                json!({ "AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root") }),
                &arn,
            )])),
        ),
        (
            "principal list naming the owner and another account",
            policy_document(json!([share_statement(
                json!({ "AWS": [ACCOUNT, OTHER_ACCOUNT] }),
                &arn,
            )])),
        ),
        (
            "wildcard principal",
            policy_document(json!([share_statement(json!("*"), &arn)])),
        ),
        (
            "service principal",
            policy_document(json!([share_statement(
                json!({ "Service": "ram.amazonaws.com" }),
                &arn,
            )])),
        ),
        (
            "two statements",
            policy_document(json!([
                share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn),
                share_statement(json!({ "AWS": "210987654321" }), &arn),
            ])),
        ),
        (
            "one statement, not a list",
            policy_document(share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)),
        ),
        (
            "statement with a Sid and a Condition, and a single Action",
            json!({
                "Statement": [{
                    "Sid": "ShareOneRule",
                    "Effect": "Allow",
                    "Principal": { "AWS": OTHER_ACCOUNT },
                    "Action": "route53resolver:GetResolverRule",
                    "Resource": arn,
                    "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-example" } },
                }],
            })
            .to_string(),
        ),
        (
            "no Version element",
            json!({ "Statement": [share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)] })
                .to_string(),
        ),
        (
            "a long document: one statement per shared account",
            policy_document(Value::Array(
                (0..40)
                    .map(|index| {
                        let account = format!("{:012}", 100_000_000_000u64 + index);
                        share_statement(json!({ "AWS": account }), &arn)
                    })
                    .collect(),
            )),
        ),
    ];

    for (case, policy) in cases {
        put_policy(&router, case, &arn, &policy).await;
        let read = stored_policy(&router, &arn).await;
        assert_eq!(
            read, policy,
            "case {case:?}: GetResolverRulePolicy must report the document that was written, got {read:?}"
        );
        // A read reports the policy without changing it: a second read of the
        // same rule answers the same document.
        let reread = stored_policy(&router, &arn).await;
        assert_eq!(
            reread, policy,
            "case {case:?}: a second GetResolverRulePolicy must report the same document, got {reread:?}"
        );
    }
}

/// A document that shares the rule with no account other than its owner stores
/// nothing: the rule keeps no policy and the read reports the document that
/// grants nothing.
#[tokio::test]
async fn a_policy_that_shares_with_nobody_stores_nothing() {
    let router = emulator();
    let (_, arn) = system_rule(&router, "policy-self", "self.example.com").await;

    // Rows differ in HOW the document fails to share: the owner named by id, by
    // its root ARN, by a list of both forms, and a document with no statement
    // to grant anything at all.
    let cases: Vec<(&str, String)> = vec![
        (
            "owner by account id",
            policy_document(json!([share_statement(json!({ "AWS": ACCOUNT }), &arn)])),
        ),
        (
            "owner by account root ARN",
            policy_document(json!([share_statement(
                json!({ "AWS": format!("arn:aws:iam::{ACCOUNT}:root") }),
                &arn,
            )])),
        ),
        (
            "owner twice, by id and by ARN",
            policy_document(json!([share_statement(
                json!({ "AWS": [ACCOUNT, format!("arn:aws:iam::{ACCOUNT}:root")] }),
                &arn,
            )])),
        ),
        (
            "two statements, both for the owner",
            policy_document(json!([
                share_statement(json!({ "AWS": ACCOUNT }), &arn),
                share_statement(
                    json!({ "AWS": format!("arn:aws:iam::{ACCOUNT}:user/analyst") }),
                    &arn,
                ),
            ])),
        ),
        (
            "a statement with no Principal",
            policy_document(json!([{
                "Effect": "Allow",
                "Action": ["route53resolver:GetResolverRule"],
                "Resource": arn,
            }])),
        ),
        ("an empty statement list", policy_document(json!([]))),
        ("no Statement element", EMPTY_POLICY_DOCUMENT.to_string()),
    ];

    for (case, policy) in cases {
        // Start each row from a rule that IS shared, so "stores nothing" is
        // observable: the self-share has to clear the share that was there.
        let shared = policy_document(json!([share_statement(
            json!({ "AWS": OTHER_ACCOUNT }),
            &arn,
        )]));
        put_policy(&router, case, &arn, &shared).await;
        assert_eq!(
            stored_policy(&router, &arn).await,
            shared,
            "case {case:?}: the sharing policy must be stored before the row runs"
        );

        put_policy(&router, case, &arn, &policy).await;
        let read = stored_policy(&router, &arn).await;
        assert_eq!(
            read, EMPTY_POLICY_DOCUMENT,
            "case {case:?}: a rule shared with nobody must report {EMPTY_POLICY_DOCUMENT}, got {read:?}"
        );
    }
}

/// A rule nobody has written a policy for reports the document that grants
/// nothing, rather than refusing the read.
#[tokio::test]
async fn a_rule_with_no_policy_reports_the_empty_document() {
    let router = emulator();
    let (_, arn) = system_rule(&router, "policy-absent", "absent.example.com").await;

    let read = stored_policy(&router, &arn).await;
    assert_eq!(
        read, EMPTY_POLICY_DOCUMENT,
        "a rule with no resource policy must report {EMPTY_POLICY_DOCUMENT}, got {read:?}"
    );
}

/// A document outside the policy grammar is refused with
/// `InvalidPolicyDocument`, and the policy the rule already carried stays.
#[tokio::test]
async fn an_invalid_document_is_refused_and_stores_nothing() {
    let router = emulator();
    let (_, arn) = system_rule(&router, "policy-invalid", "invalid.example.com").await;

    let shared = policy_document(json!([share_statement(
        json!({ "AWS": OTHER_ACCOUNT }),
        &arn,
    )]));
    put_policy(&router, "the policy to preserve", &arn, &shared).await;

    // Rows differ in which part of the grammar the document misses: it is not
    // JSON at all, it is JSON but not an object, or one of its statements is
    // not a statement.
    let cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("not JSON", "this is not a policy".to_string()),
        ("truncated JSON", "{\"Statement\": [".to_string()),
        (
            "a JSON array",
            json!([share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)]).to_string(),
        ),
        ("a JSON string", json!("Allow").to_string()),
        ("a JSON number", "17".to_string()),
        (
            "a statement that is not an object",
            policy_document(json!(["route53resolver:GetResolverRule"])),
        ),
        (
            "a statement with no Effect",
            policy_document(json!([{
                "Principal": { "AWS": OTHER_ACCOUNT },
                "Action": ["route53resolver:GetResolverRule"],
                "Resource": arn,
            }])),
        ),
        (
            "a statement with no Action",
            policy_document(json!([{
                "Effect": "Allow",
                "Principal": { "AWS": OTHER_ACCOUNT },
                "Resource": arn,
            }])),
        ),
        (
            "a second statement with no Effect",
            policy_document(json!([
                share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn),
                { "Action": ["route53resolver:GetResolverRule"], "Resource": arn },
            ])),
        ),
    ];

    for (case, policy) in cases {
        expect_error(
            &router,
            "PutResolverRulePolicy",
            json!({ "Arn": arn, "ResolverRulePolicy": policy }),
            case,
            "InvalidPolicyDocument",
        )
        .await;
        let read = stored_policy(&router, &arn).await;
        assert_eq!(
            read, shared,
            "case {case:?}: a refused document must leave the stored policy alone, got {read:?}"
        );
    }
}

/// An `Arn` the store holds no Resolver rule under is refused with
/// `UnknownResourceException` by both operations, and the write persists
/// nothing for it.
#[tokio::test]
async fn an_arn_naming_no_rule_is_refused() {
    let router = emulator();
    let (deleted_id, deleted_arn) =
        system_rule(&router, "policy-deleted", "gone.example.com").await;
    let (live_id, live_arn) = system_rule(&router, "policy-live", "live.example.com").await;
    ok(
        &router,
        "DeleteResolverRule",
        json!({ "ResolverRuleId": deleted_id }),
    )
    .await;

    // Rows differ in how the ARN misses a stored rule: a rule id that was never
    // minted, a rule that has since been deleted, another resource type, another
    // region or account, and the rule's own id instead of its ARN.
    let cases: Vec<(&str, String)> = vec![
        (
            "a rule id nothing was created under",
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-rule/rslvr-rr-0000000000000000"
            ),
        ),
        ("a deleted rule's ARN", deleted_arn),
        (
            "another resource type",
            live_arn.replace("resolver-rule/", "resolver-endpoint/"),
        ),
        ("another region", live_arn.replace(REGION, "eu-west-1")),
        (
            "another account",
            live_arn.replace(ACCOUNT, "123456789012"),
        ),
        ("the rule id, not its ARN", live_id.clone()),
    ];

    let policy = policy_document(json!([share_statement(
        json!({ "AWS": OTHER_ACCOUNT }),
        &live_arn,
    )]));

    for (case, arn) in cases {
        expect_error(
            &router,
            "PutResolverRulePolicy",
            json!({ "Arn": arn, "ResolverRulePolicy": policy }),
            case,
            "UnknownResourceException",
        )
        .await;
        expect_error(
            &router,
            "GetResolverRulePolicy",
            json!({ "Arn": arn }),
            case,
            "UnknownResourceException",
        )
        .await;
        // The refusal names the ARN that was asked for, so the caller can see
        // which resource the service found nothing under.
        let (_, refusal) = call(
            &router,
            "GetResolverRulePolicy",
            json!({ "Arn": arn.clone() }),
        )
        .await;
        let message = string_member(&refusal, "Message");
        assert!(
            message.contains(&arn),
            "case {case:?}: the refusal must name the ARN {arn:?}, got {message:?} ({})",
            error_type(&refusal)
        );
    }

    // The rule that does exist is untouched by every refusal above: it still
    // carries no policy.
    let read = stored_policy(&router, &live_arn).await;
    assert_eq!(
        read, EMPTY_POLICY_DOCUMENT,
        "a refused write must persist nothing, got {read:?}"
    );
}
