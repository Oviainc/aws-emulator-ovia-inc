// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACTION rule `PutResourcePolicy` applies to a resource
//! policy: the actions a statement grants must be drawn FROM the set the shared
//! resource's type permits (containment), so any non-empty subset of that set is
//! accepted while an action outside it fails validation.
//!
//! The per-kind lifecycle tests (attach, read back, detach, detach on deleting the
//! resource) live in `resource_policies` and
//! `rule_group_capacity_and_resource_policy`; these suites vary the GRANTED SET
//! across the whole class the rule covers.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.

use serde_json::json;
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, resource_policy_document, split_resource_policy_document, Harness,
};

/// Every non-empty subset of `permitted`, as a list of (label, actions) rows.
fn non_empty_subsets<'a>(permitted: &[&'a str]) -> Vec<(String, Vec<&'a str>)> {
    (1u32..(1u32 << permitted.len()))
        .map(|mask| {
            let actions: Vec<&str> = permitted
                .iter()
                .copied()
                .enumerate()
                .filter(|(index, _)| (mask >> index) & 1 == 1)
                .map(|(_, action)| action)
                .collect();
            (actions.join("+"), actions)
        })
        .collect()
}

/// RULE: a resource policy's statements may grant any non-empty SUBSET of the
/// actions the shared resource's type permits — containment, not set equality, so
/// the actions a caller leaves out are simply not granted — and such a policy is
/// attached (HTTP 200, empty body), reported verbatim by `DescribeResourcePolicy`
/// and detachable with `DeleteResourcePolicy`.
///
/// The rows are the FULL power set (minus the empty grant, which no statement can
/// express) of each shareable resource type's permitted actions, from the lists the
/// AWS `PutResourcePolicy` documentation gives per resource type
/// (`smithy_query(query="doc",shape="PutResourcePolicyRequest",member="Policy")`),
/// held in `state::RULE_GROUP_POLICY_ACTIONS` and
/// `state::FIREWALL_POLICY_POLICY_ACTIONS`. That spans both sizes and every
/// combination, including the two-action rule-group grant that omits
/// `network-firewall:UpdateFirewallPolicy` and the single-action grants.
#[tokio::test]
async fn any_non_empty_subset_of_the_permitted_actions_is_attached_and_read_back() {
    let harness = Harness::new();
    let kinds: [(&str, &[&str], String); 2] = [
        (
            "rule-group",
            &state::RULE_GROUP_POLICY_ACTIONS,
            harness
                .shared_rule_group("action-subsets", "STATEFUL", 10)
                .await,
        ),
        (
            "firewall-policy",
            &state::FIREWALL_POLICY_POLICY_ACTIONS,
            harness.shared_policy("action-subsets").await,
        ),
    ];

    for (kind, permitted, arn) in kinds {
        for (label, actions) in non_empty_subsets(permitted) {
            let case = format!("{kind}/{label}");
            let document = resource_policy_document(&actions, &arn);

            let (status, body) = harness
                .call(
                    "PutResourcePolicy",
                    &json!({ "ResourceArn": arn, "Policy": document }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: a subset of the permitted actions must be accepted, got {status} \
                 {body:?}"
            );
            assert!(
                body.is_null() || body == json!({}),
                "case {case}: expected an empty body, got {body:?}"
            );

            let described = harness
                .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                described["Policy"],
                json!(document),
                "case {case}: expected the attached document, got {described:?}"
            );

            let (status, body) = harness
                .call("DeleteResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: detaching the attached policy must succeed, got {status} {body:?}"
            );
            let (status, body) = harness
                .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: a detached policy is gone, got {status} {body:?}"
            );
        }
    }
}

/// RULE: an action the shared resource's type does NOT permit fails validation
/// (`InvalidResourcePolicyException`, HTTP 400) and attaches nothing, however many
/// permitted actions the same document grants beside it.
///
/// The out-of-set catalog is the actions the AWS `PutResourcePolicy`
/// documentation lists for the OTHER resource types
/// (`smithy_query(query="doc",shape="PutResourcePolicyRequest",member="Policy")`:
/// `network-firewall:ListFirewalls` and `network-firewall:DescribeFirewallMetadata`
/// belong to a firewall resource, and each of the two shareable types' sets is out
/// of set for the other), plus an action of another service entirely. Each row is
/// tried alone, appended to the type's full permitted set, and appended to a
/// one-action subset of it.
#[tokio::test]
async fn an_action_outside_the_permitted_set_fails_validation() {
    let harness = Harness::new();
    let kinds: [(&str, &[&str], String); 2] = [
        (
            "rule-group",
            &state::RULE_GROUP_POLICY_ACTIONS,
            harness
                .shared_rule_group("out-of-set", "STATELESS", 10)
                .await,
        ),
        (
            "firewall-policy",
            &state::FIREWALL_POLICY_POLICY_ACTIONS,
            harness.shared_policy("out-of-set").await,
        ),
    ];

    for (kind, permitted, arn) in kinds {
        // Actions no statement on either shareable type may name, plus the other
        // shareable type's own set, which is out of set here.
        let mut outsiders: Vec<&str> = vec![
            "network-firewall:ListFirewalls",
            "network-firewall:DescribeFirewallMetadata",
            "s3:GetObject",
        ];
        let other: &[&str] = if kind == "rule-group" {
            &state::FIREWALL_POLICY_POLICY_ACTIONS
        } else {
            &state::RULE_GROUP_POLICY_ACTIONS
        };
        outsiders.extend_from_slice(other);

        for outsider in outsiders {
            let grants: [(&str, Vec<&str>); 3] = [
                ("alone", vec![outsider]),
                (
                    "with-the-permitted-set",
                    permitted.iter().copied().chain([outsider]).collect(),
                ),
                ("with-one-permitted-action", vec![permitted[0], outsider]),
            ];
            for (shape, actions) in grants {
                let case = format!("{kind}/{outsider}/{shape}");
                let (status, body) = harness
                    .call(
                        "PutResourcePolicy",
                        &json!({
                            "ResourceArn": arn,
                            "Policy": resource_policy_document(&actions, &arn),
                        }),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidResourcePolicyException"),
                    "case {case}: expected InvalidResourcePolicyException, got {status} {body:?}"
                );
                let (status, body) = harness
                    .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case}: a refused policy attaches nothing, got {status} {body:?}"
                );
            }
        }
    }
}

/// RULE: the set a policy grants is the UNION of its `Allow` statements' actions,
/// so the same containment test applies however the grants are split across
/// statements.
///
/// The rows split the rule-group permitted set
/// (`state::RULE_GROUP_POLICY_ACTIONS`) across one, two and three statements, and
/// then put an out-of-set action in a statement of its own beside permitted ones.
#[tokio::test]
async fn containment_is_tested_on_the_union_of_the_statements() {
    let harness = Harness::new();
    let permitted = state::RULE_GROUP_POLICY_ACTIONS;
    let arn = harness
        .shared_rule_group("split-statements", "STATEFUL", 10)
        .await;
    let outsider = ["network-firewall:DeleteRuleGroup"];

    let cases: [(&str, Vec<&[&str]>, bool); 5] = [
        ("one-statement-full-set", vec![&permitted], true),
        (
            "two-statements-full-set",
            vec![&permitted[..1], &permitted[1..]],
            true,
        ),
        (
            "three-statements-one-action-each",
            vec![&permitted[..1], &permitted[1..2], &permitted[2..]],
            true,
        ),
        (
            "two-statements-subset",
            vec![&permitted[..1], &permitted[2..]],
            true,
        ),
        (
            "out-of-set-action-in-its-own-statement",
            vec![&permitted[..1], &outsider],
            false,
        ),
    ];

    for (label, statements, accepted) in cases {
        let _ = harness
            .call("DeleteResourcePolicy", &json!({ "ResourceArn": arn }))
            .await;
        let document = split_resource_policy_document(&statements, &arn);
        let (status, body) = harness
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "Policy": document }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label}: the union is within the permitted set, got {status} {body:?}"
            );
            let described = harness
                .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;
            assert_eq!(
                described["Policy"],
                json!(document),
                "case {label}: expected the attached document, got {described:?}"
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
