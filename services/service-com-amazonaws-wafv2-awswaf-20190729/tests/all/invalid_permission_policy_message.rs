// SPDX-License-Identifier: CC0-1.0

//! What a refused `PutPermissionPolicy` SAYS.
//!
//! The rule this suite asserts: a WAFv2 refusal carries the sentence the SERVICE puts
//! on the wire for that exception type, which is not the description the model's
//! `documentation` trait gives the exception shape — the prose documents what the
//! error means and lists the restrictions a request is judged against, while the wire
//! message is one fixed sentence per exception type, carrying nothing of the request.
//!
//! `WAFInvalidPermissionPolicyException` is where the two texts differ: its
//! documentation opens with [`DOCUMENTATION_SENTENCE`] and goes on to list the
//! restrictions, while the sentence WAF answers with is
//! [`INVALID_PERMISSION_POLICY`]. So this suite drives a TABLE of documents spanning
//! the restriction classes that documentation names — the policy language version, the
//! required `Effect`/`Action`/`Principal`, an `Effect` other than `Allow`, an extra or
//! wildcard action, a `Resource`, and a document that is not a policy at all — across
//! BOTH scopes, and asserts every one of them answers that single sentence and never
//! the documentation's opening line. A positive control runs first, so the suite also
//! shows the operation still attaches a policy within the restrictions.
//!
//! Only the message rule lives here: the storing, reading back, replacing and deleting
//! of a permission policy, and the wider accepted/refused tables, are
//! [`crate::permission_policy`], whose helpers and action catalogue this suite reuses
//! rather than restating.

use serde_json::json;

use crate::permission_policy::{
    allow, error_message, policy, required_actions, CREATE_WEB_ACL, GET_RULE_GROUP, OTHER_ACCOUNT,
    PUT_FIREWALL_MANAGER_RULE_GROUPS, UPDATE_WEB_ACL,
};
use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::Waf;

/// The sentence `WAFInvalidPermissionPolicyException` carries, as recorded from WAF.
/// Spelled out here as a literal, independently of the service's own constant, so
/// this suite pins the text and not merely its reuse.
const INVALID_PERMISSION_POLICY: &str = "The policy specification doesn't meet the \
     requirements to be attached to an AWS WAF resource.";

/// The OPENING SENTENCE of the `documentation` trait on the exception shape
/// (`smithy_query(query="shape",shape="WAFInvalidPermissionPolicyException")` — "<p>The
/// operation failed because the specified policy isn't in the proper format. </p> …"),
/// with its wrapping whitespace flattened. It describes the error; it is NOT what the
/// service answers, so no refusal here may carry it.
const DOCUMENTATION_SENTENCE: &str =
    "The operation failed because the specified policy isn't in the proper format.";

/// One row: a document outside the documented restrictions, and which restriction it
/// breaks.
struct RefusedDocument {
    case: &'static str,
    policy: String,
}

/// Every rejection class of `PutPermissionPolicy` answers the exception type's one
/// wire sentence — never the opening line of the shape's documentation — and discloses
/// nothing of the document it refused.
#[tokio::test]
async fn every_rejected_policy_carries_the_wire_sentence_not_the_documentation() {
    let waf = Waf::new();

    // The positive control: the refusals below report a rejected document, not a
    // `PutPermissionPolicy` that never works.
    let accepted = policy(json!([allow(
        json!([
            CREATE_WEB_ACL,
            UPDATE_WEB_ACL,
            PUT_FIREWALL_MANAGER_RULE_GROUPS,
            GET_RULE_GROUP
        ]),
        json!({ "AWS": OTHER_ACCOUNT })
    )]));
    let (_control_id, control_arn, _control_lock) =
        create_rule_group(&waf, "REGIONAL", "policy-message-control", 10, json!([])).await;
    waf.ok(
        "PutPermissionPolicy",
        json!({ "ResourceArn": &control_arn, "Policy": &accepted }),
    )
    .await;
    let read = waf
        .ok("GetPermissionPolicy", json!({ "ResourceArn": &control_arn }))
        .await;
    assert_eq!(
        read["Policy"].as_str(),
        Some(accepted.as_str()),
        "case \"the positive control\": GetPermissionPolicy must answer the stored policy \
         text, got {read}"
    );

    let rows = vec![
        RefusedDocument {
            case: "a document that is not JSON at all",
            policy: "{not valid json".to_string(),
        },
        RefusedDocument {
            case: "a JSON scalar in place of a policy object",
            policy: json!("allow the other account").to_string(),
        },
        RefusedDocument {
            case: "an older policy language version",
            policy: json!({
                "Version": "2008-10-17",
                "Statement": [allow(required_actions(), json!(OTHER_ACCOUNT))],
            })
            .to_string(),
        },
        RefusedDocument {
            case: "a wildcard action for the whole service",
            policy: policy(json!([allow(json!("wafv2:*"), json!(OTHER_ACCOUNT))])),
        },
        RefusedDocument {
            case: "a wildcard action beside the required ones",
            policy: policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    "wafv2:Get*"
                ]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedDocument {
            case: "an extra action outside the documented list",
            policy: policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    "wafv2:DeleteRuleGroup"
                ]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedDocument {
            case: "a required action missing",
            policy: policy(json!([allow(
                json!([CREATE_WEB_ACL, UPDATE_WEB_ACL]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedDocument {
            case: "an Effect other than Allow",
            policy: policy(json!([{
                "Effect": "Deny",
                "Principal": OTHER_ACCOUNT,
                "Action": required_actions(),
            }])),
        },
        RefusedDocument {
            case: "no Principal",
            policy: policy(json!([{ "Effect": "Allow", "Action": required_actions() }])),
        },
        RefusedDocument {
            case: "no Action",
            policy: policy(json!([{ "Effect": "Allow", "Principal": OTHER_ACCOUNT }])),
        },
        RefusedDocument {
            case: "a Resource parameter",
            policy: policy(json!([{
                "Effect": "Allow",
                "Principal": OTHER_ACCOUNT,
                "Action": required_actions(),
                "Resource": "*",
            }])),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Both scopes, so a message assembled from anything the rule group carries
        // would show up as a difference between the two answers.
        for scope in ["REGIONAL", "CLOUDFRONT"] {
            let name = format!("policy-message-{index}-{}", scope.to_lowercase());
            let (_id, arn, _lock) = create_rule_group(&waf, scope, &name, 10, json!([])).await;
            let refusal = waf
                .refused(
                    "PutPermissionPolicy",
                    json!({ "ResourceArn": &arn, "Policy": &row.policy }),
                    "WAFInvalidPermissionPolicyException",
                    row.case,
                )
                .await;
            let message = error_message(&refusal);
            assert_eq!(
                message, INVALID_PERMISSION_POLICY,
                "case {:?} in {scope}: expected the wire sentence {INVALID_PERMISSION_POLICY:?}, \
                 got {message:?}",
                row.case
            );
            assert_ne!(
                message, DOCUMENTATION_SENTENCE,
                "case {:?} in {scope}: the refusal must not carry the shape's documentation \
                 sentence, got {message:?}",
                row.case
            );
            assert!(
                !message.contains(&arn) && !message.contains(&name),
                "case {:?} in {scope}: the refusal must disclose nothing of the request, \
                 got {message:?}",
                row.case
            );
        }
    }
}
