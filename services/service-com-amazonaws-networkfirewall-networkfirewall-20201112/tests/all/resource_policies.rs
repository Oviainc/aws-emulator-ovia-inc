// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for resource policies on a FIREWALL POLICY —
//! `PutResourcePolicy`, `DescribeResourcePolicy` and `DeleteResourcePolicy` —
//! and for the policy-document validation every resource kind shares.
//!
//! The resource-policy tests for a shared RULE GROUP, and the capacity a
//! firewall policy reserves for the rule groups it references, live in
//! `rule_group_capacity_and_resource_policy`.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{error_code, resource_policy_document, Harness, ACCOUNT, REGION};

/// RULE: a Network Firewall resource policy is validated against the RAM managed
/// permission for the shared resource's TYPE, so the actions its statement grants
/// must be exactly that type's fixed set; any other document is refused with
/// `InvalidResourcePolicyException` (HTTP 400) and nothing is attached, while an
/// accepted one is returned verbatim by `DescribeResourcePolicy` until it is
/// deleted.
///
/// The permitted set for a firewall policy is the one the AWS
/// `PutResourcePolicy` documentation lists for that resource type, held in
/// `state::FIREWALL_POLICY_POLICY_ACTIONS`; the rows vary the granted set around
/// it (order, an out-of-set action, a missing one, an extra one, and the set
/// that belongs to the other resource type).
#[tokio::test]
async fn only_the_permitted_action_set_is_attached_to_a_firewall_policy() {
    let harness = Harness::new();
    let permitted: Vec<&str> = state::FIREWALL_POLICY_POLICY_ACTIONS.to_vec();
    let arn = harness.shared_policy("shared-policy").await;

    let cases: [(&str, Vec<&str>, bool); 6] = [
        ("exact-set", permitted.clone(), true),
        (
            "reversed-set",
            permitted.iter().copied().rev().collect(),
            true,
        ),
        (
            "out-of-set-action",
            vec!["network-firewall:DeleteFirewallPolicy"],
            false,
        ),
        ("missing-an-action", vec![permitted[0]], false),
        (
            "with-an-extra-action",
            permitted
                .iter()
                .copied()
                .chain(["network-firewall:ListRuleGroups"])
                .collect(),
            false,
        ),
        (
            "with-the-rule-group-set",
            state::RULE_GROUP_POLICY_ACTIONS.to_vec(),
            false,
        ),
    ];

    for (label, actions, accepted) in cases {
        let document = resource_policy_document(&actions, &arn);

        // Start each row from "nothing attached", so it is judged on its own.
        let _ = harness
            .call("DeleteResourcePolicy", &json!({ "ResourceArn": arn }))
            .await;

        let (status, body) = harness
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "Policy": document }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label}: the permitted action set must be accepted, got {status} {body:?}"
            );
            let described = harness
                .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                described["Policy"],
                json!(document),
                "case {label}: expected the attached document, got {described:?}"
            );
            harness
                .ok("DeleteResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            let (status, body) = harness
                .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {label}: a detached policy is gone, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidResourcePolicyException"),
                "case {label}: expected InvalidResourcePolicyException, got {status} {body:?}"
            );
            let (status, body) = harness
                .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {label}: a refused policy attaches nothing, got {status} {body:?}"
            );
        }
    }
}

/// RULE: a resource policy has to BE a policy document — a JSON object with at
/// least one `Allow` statement naming a principal and an action — and it can only
/// be attached to a rule group or a firewall policy that exists; each failure is
/// answered with its own modelled error.
#[tokio::test]
async fn a_malformed_or_misdirected_resource_policy_is_refused() {
    let harness = Harness::new();
    let arn = harness.shared_policy("document-checks").await;
    let actions = state::FIREWALL_POLICY_POLICY_ACTIONS;

    let documents: [(&str, String); 6] = [
        ("not-json", "}{ not json".to_string()),
        ("json-but-not-an-object", json!([1, 2, 3]).to_string()),
        ("no-statement", json!({ "Version": "2012-10-17" }).to_string()),
        (
            "empty-statement-list",
            json!({ "Statement": [] }).to_string(),
        ),
        (
            "effect-deny",
            json!({ "Statement": [{
                "Effect": "Deny",
                "Principal": { "AWS": ACCOUNT },
                "Action": actions,
            }]})
            .to_string(),
        ),
        (
            "no-principal",
            json!({ "Statement": [{ "Effect": "Allow", "Action": actions }]}).to_string(),
        ),
    ];
    for (label, document) in documents {
        let (status, body) = harness
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "Policy": document }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidResourcePolicyException"),
            "case {label}: expected InvalidResourcePolicyException, got {status} {body:?}"
        );
    }

    // A statement given as a single object, rather than a list, is the other
    // accepted spelling.
    let single = json!({ "Statement": {
        "Effect": "Allow",
        "Principal": { "AWS": ACCOUNT },
        "Action": actions,
        "Resource": arn,
    }})
    .to_string();
    harness
        .ok(
            "PutResourcePolicy",
            &json!({ "ResourceArn": arn, "Policy": single }),
        )
        .await;

    let targets: [(&str, String, String, &str); 3] = [
        (
            "a-firewall-cannot-be-shared",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/anything"),
            resource_policy_document(&actions, "*"),
            "InvalidRequestException",
        ),
        (
            "an-absent-firewall-policy",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/absent"),
            resource_policy_document(&actions, "*"),
            "ResourceNotFoundException",
        ),
        (
            "an-absent-rule-group",
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:stateful-rulegroup/absent"),
            resource_policy_document(&state::RULE_GROUP_POLICY_ACTIONS, "*"),
            "ResourceNotFoundException",
        ),
    ];
    for (label, target, document, expected) in targets {
        let (status, body) = harness
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": target, "Policy": document }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected),
            "case {label}: expected {expected}, got {status} {body:?}"
        );
    }
}

/// RULE: a resource policy belongs to the resource it is attached to — it is
/// reachable only through that resource's own ARN — so deleting the resource
/// detaches it, and a resource created again under the same name carries none.
///
/// The rows are the two resource kinds that can carry a policy, each deleted
/// through its own operation.
#[tokio::test]
async fn deleting_a_shared_resource_detaches_its_resource_policy() {
    let harness = Harness::new();

    let kinds: [(&str, &[&str], Value); 2] = [
        (
            "firewall-policy",
            &state::FIREWALL_POLICY_POLICY_ACTIONS,
            json!({ "FirewallPolicyName": "detaching" }),
        ),
        (
            "rule-group",
            &state::RULE_GROUP_POLICY_ACTIONS,
            json!({ "RuleGroupName": "detaching", "Type": "STATEFUL" }),
        ),
    ];

    for (kind, actions, delete_request) in kinds {
        let delete_target = if kind == "firewall-policy" {
            "DeleteFirewallPolicy"
        } else {
            "DeleteRuleGroup"
        };
        // Two rounds: the second proves the name is reusable and starts clean.
        for round in ["first", "second"] {
            let case = format!("{kind}/{round}");
            let arn = if kind == "firewall-policy" {
                harness.shared_policy("detaching").await
            } else {
                harness
                    .shared_rule_group("detaching", "STATEFUL", 10)
                    .await
            };

            let (status, body) = harness
                .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: a new resource carries no policy, got {status} {body:?}"
            );

            let document = resource_policy_document(actions, &arn);
            harness
                .ok(
                    "PutResourcePolicy",
                    &json!({ "ResourceArn": arn, "Policy": document }),
                )
                .await;
            harness
                .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;

            harness.ok(delete_target, &delete_request).await;
            let (status, body) = harness
                .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: deleting the resource detaches its policy, got {status} {body:?}"
            );
        }
    }
}
