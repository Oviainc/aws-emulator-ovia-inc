// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the firewall policies that USE a rule group: the count
//! `RuleGroupResponse.NumberOfAssociations` reports, and the delete that count
//! refuses.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{error_code, error_message, minimal_policy_document, string_at, Harness};

/// What a delete refused because another resource still references the object
/// carries. RECORDED behaviour, spelled out here rather than read back from the
/// code under test: the model fixes the classification and the status only.
const DELETE_IN_USE_MESSAGE: &str = "Unable to delete the object because it is still in use";

/// A `FirewallPolicy` document referencing every ARN in `rule_group_arns` from
/// the reference list its KIND belongs to: a stateless rule group is named in
/// `StatelessRuleGroupReferences` (which also carries the `Priority` the model
/// requires there), every stateful kind in `StatefulRuleGroupReferences`.
fn document_referencing(rule_group_arns: &[&str]) -> Value {
    let mut document = minimal_policy_document();
    let stateless: Vec<Value> = rule_group_arns
        .iter()
        .filter(|arn| arn.contains(":stateless-rulegroup/"))
        .enumerate()
        .map(|(index, arn)| json!({ "ResourceArn": arn, "Priority": index + 1 }))
        .collect();
    let stateful: Vec<Value> = rule_group_arns
        .iter()
        .filter(|arn| !arn.contains(":stateless-rulegroup/"))
        .map(|arn| json!({ "ResourceArn": arn }))
        .collect();
    if !stateless.is_empty() {
        document["StatelessRuleGroupReferences"] = json!(stateless);
    }
    if !stateful.is_empty() {
        document["StatefulRuleGroupReferences"] = json!(stateful);
    }
    document
}

/// The `NumberOfAssociations` the rule group `selector` names reports, read back
/// through `DescribeRuleGroup`.
async fn described_associations(harness: &Harness, selector: &Value) -> i64 {
    let described = harness.ok("DescribeRuleGroup", selector).await;
    described["RuleGroupResponse"]["NumberOfAssociations"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected RuleGroupResponse.NumberOfAssociations in {described:?}"))
}

/// One row: a rule group kind, the document that creates it, and how many
/// firewall policies will reference it.
#[derive(Debug)]
struct Case {
    /// The row's label, which is also the rule group's name.
    label: &'static str,
    group_type: &'static str,
    capacity: i64,
    /// How many firewall policies reference the rule group.
    policies: usize,
}

/// The rows: the rule-group kinds the service holds — which is also both
/// reference lists a policy names a rule group in
/// (`smithy_query(query="shape",shape="RuleGroupType")`: `STATELESS`,
/// `STATEFUL`, `STATEFUL_DOMAIN`) — crossed with how many policies use the rule
/// group: none, one, and several.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "assoc-stateless-unreferenced",
            group_type: "STATELESS",
            capacity: 20,
            policies: 0,
        },
        Case {
            label: "assoc-stateless-one-policy",
            group_type: "STATELESS",
            capacity: 20,
            policies: 1,
        },
        Case {
            label: "assoc-stateful-unreferenced",
            group_type: "STATEFUL",
            capacity: 10,
            policies: 0,
        },
        Case {
            label: "assoc-stateful-one-policy",
            group_type: "STATEFUL",
            capacity: 10,
            policies: 1,
        },
        Case {
            label: "assoc-stateful-three-policies",
            group_type: "STATEFUL",
            capacity: 10,
            policies: 3,
        },
        Case {
            label: "assoc-domain-two-policies",
            group_type: "STATEFUL_DOMAIN",
            capacity: 90,
            policies: 2,
        },
    ]
}

/// Create the rule group a row describes and answer its ARN and the selector
/// every later call identifies it by.
async fn create_rule_group(harness: &Harness, case: &Case) -> (String, Value) {
    let arn = if case.group_type == "STATEFUL_DOMAIN" {
        string_at(
            &harness
                .ok(
                    "CreateRuleGroup",
                    &json!({
                        "RuleGroupName": case.label,
                        "Type": case.group_type,
                        "Capacity": case.capacity,
                        "RuleGroup": { "RulesSource": { "RulesSourceList": {
                            "Targets": [".one.example", "two.example"],
                            "TargetTypes": ["TLS_SNI"],
                            "GeneratedRulesType": "ALLOWLIST",
                        }}},
                    }),
                )
                .await,
            &["RuleGroupResponse", "RuleGroupArn"],
        )
        .to_string()
    } else {
        harness
            .shared_rule_group(case.label, case.group_type, case.capacity)
            .await
    };
    let selector = json!({ "RuleGroupName": case.label, "Type": case.group_type });
    (arn, selector)
}

/// RULE: `RuleGroupResponse.NumberOfAssociations` is "the number of firewall
/// policies that use this rule group" — the stored firewall policies whose
/// document names that rule group's ARN in either reference list — so it rises
/// as policies take the reference and falls as they give it up, whatever the
/// rule group's kind, and every response that projects the rule group reports
/// the same count.
#[tokio::test]
async fn a_rule_group_reports_the_number_of_policies_that_reference_it() {
    let harness = Harness::new();

    for case in cases() {
        let (arn, selector) = create_rule_group(&harness, &case).await;

        // A rule group nothing references is used by no policy, which is the
        // count a create reports.
        assert_eq!(
            described_associations(&harness, &selector).await,
            0,
            "case {case:?}: expected a freshly created rule group to be used by no policy"
        );

        // Each policy that takes the reference is one more policy using the
        // rule group.
        let policy_names: Vec<String> = (0..case.policies)
            .map(|index| format!("{}-policy-{index}", case.label))
            .collect();
        for (taken, policy_name) in policy_names.iter().enumerate() {
            let created = harness
                .ok(
                    "CreateFirewallPolicy",
                    &json!({
                        "FirewallPolicyName": policy_name,
                        "FirewallPolicy": document_referencing(&[&arn]),
                    }),
                )
                .await;
            let expected = i64::try_from(taken + 1).expect("a row's policy count fits an i64");
            assert_eq!(
                described_associations(&harness, &selector).await,
                expected,
                "case {case:?}: expected {expected} policies to use the rule group after \
                 {created:?}"
            );
        }

        // Every projection of the rule group reports the same count: the read
        // and the update answer from the same place.
        let described = harness.ok("DescribeRuleGroup", &selector).await;
        let mut update = selector.clone();
        update["UpdateToken"] = json!(string_at(&described, &["UpdateToken"]));
        update["Description"] = json!("re-described");
        let updated = harness.ok("UpdateRuleGroup", &update).await;
        assert_eq!(
            updated["RuleGroupResponse"]["NumberOfAssociations"],
            described["RuleGroupResponse"]["NumberOfAssociations"],
            "case {case:?}: expected the update to report the count the read did, got {updated:?}"
        );

        // A policy that gives the reference up — here by being deleted — stops
        // using the rule group, so the count falls back to none.
        for (left, policy_name) in policy_names.iter().enumerate() {
            harness
                .ok(
                    "DeleteFirewallPolicy",
                    &json!({ "FirewallPolicyName": policy_name }),
                )
                .await;
            let expected = i64::try_from(case.policies - left - 1)
                .expect("a row's policy count fits an i64");
            assert_eq!(
                described_associations(&harness, &selector).await,
                expected,
                "case {case:?}: expected {expected} policies to use the rule group after \
                 deleting {policy_name}"
            );
        }
    }
}

/// RULE: a rule group a firewall policy still uses cannot be deleted —
/// `InvalidOperationException` is "the operation failed because it's not valid.
/// For example, you might have tried to delete a rule group or firewall policy
/// that's in use" — and nothing is removed, so the delete becomes possible again
/// exactly when the last policy using it gives the reference up.
#[tokio::test]
async fn a_rule_group_in_use_by_a_policy_cannot_be_deleted() {
    let harness = Harness::new();

    for case in cases().into_iter().filter(|case| case.policies > 0) {
        let (arn, selector) = create_rule_group(&harness, &case).await;
        let policy_names: Vec<String> = (0..case.policies)
            .map(|index| format!("{}-blocking-{index}", case.label))
            .collect();
        for policy_name in &policy_names {
            harness
                .ok(
                    "CreateFirewallPolicy",
                    &json!({
                        "FirewallPolicyName": policy_name,
                        "FirewallPolicy": document_referencing(&[&arn]),
                    }),
                )
                .await;
        }

        // While ANY policy uses it, the delete is refused — including after all
        // but one of them have gone.
        for policy_name in &policy_names {
            let (status, body) = harness.call("DeleteRuleGroup", &selector).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 while a policy uses the rule group, got \
                 {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidOperationException",
                "case {case:?}: expected InvalidOperationException, got {body:?}"
            );
            assert_eq!(
                error_message(&body),
                DELETE_IN_USE_MESSAGE,
                "case {case:?}: expected the in-use refusal text, got {body:?}"
            );
            // Nothing was removed: the rule group is still there to read.
            assert!(
                described_associations(&harness, &selector).await > 0,
                "case {case:?}: expected the refused delete to leave the rule group in use"
            );
            harness
                .ok(
                    "DeleteFirewallPolicy",
                    &json!({ "FirewallPolicyName": policy_name }),
                )
                .await;
        }

        // With the last reference gone the delete is accepted, and reports the
        // rule group as used by nothing.
        let deleted = harness.ok("DeleteRuleGroup", &selector).await;
        assert_eq!(
            deleted["RuleGroupResponse"]["NumberOfAssociations"],
            json!(0),
            "case {case:?}: expected the accepted delete to report no policy using the rule \
             group, got {deleted:?}"
        );
        let (status, body) = harness.call("DescribeRuleGroup", &selector).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted rule group to be gone, got {status} with \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException after the delete, got {body:?}"
        );
    }
}

/// RULE: the policy document a successful `UpdateFirewallPolicy` stores REPLACES
/// the stored one, so the rule groups that policy uses afterwards are exactly the
/// ones its new document names: a reference the caller dropped stops counting
/// towards that rule group's `NumberOfAssociations` (and the rule group becomes
/// deletable again), and one it added starts counting.
#[tokio::test]
async fn updating_a_policy_moves_the_associations_its_references_moved() {
    let harness = Harness::new();
    let group_a = harness.shared_rule_group("swap-a", "STATEFUL", 10).await;
    let group_b = harness.shared_rule_group("swap-b", "STATEFUL", 10).await;
    let group_c = harness.shared_rule_group("swap-c", "STATELESS", 20).await;
    let selector_a = json!({ "RuleGroupName": "swap-a", "Type": "STATEFUL" });
    let selector_b = json!({ "RuleGroupName": "swap-b", "Type": "STATEFUL" });
    let selector_c = json!({ "RuleGroupName": "swap-c", "Type": "STATELESS" });

    let created = harness
        .ok(
            "CreateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "swapping-policy",
                "FirewallPolicy": document_referencing(&[&group_a]),
            }),
        )
        .await;
    let mut token = string_at(&created, &["UpdateToken"]).to_string();
    assert_eq!(
        described_associations(&harness, &selector_a).await,
        1,
        "case created: expected the policy to use the rule group it referenced"
    );

    // Rows: the references the policy's new document names, and the count each
    // of the three rule groups must then report.
    let rows: [(&str, &[&str], [i64; 3]); 5] = [
        ("swap-a-for-b", &["swap-b"], [0, 1, 0]),
        ("name-both-stateful", &["swap-a", "swap-b"], [1, 1, 0]),
        ("add-the-stateless-one", &["swap-a", "swap-b", "swap-c"], [1, 1, 1]),
        ("keep-only-the-stateless-one", &["swap-c"], [0, 0, 1]),
        ("drop-every-reference", &[], [0, 0, 0]),
    ];

    for (label, referenced, expected) in rows {
        let arns: Vec<&str> = referenced
            .iter()
            .map(|name| match *name {
                "swap-a" => group_a.as_str(),
                "swap-b" => group_b.as_str(),
                _ => group_c.as_str(),
            })
            .collect();
        let document = document_referencing(&arns);
        let updated = harness
            .ok(
                "UpdateFirewallPolicy",
                &json!({
                    "FirewallPolicyName": "swapping-policy",
                    "UpdateToken": token,
                    "FirewallPolicy": document.clone(),
                }),
            )
            .await;
        // The token marks the state of the resource, so the update mints a new
        // one and the caller's old one no longer unlocks the policy.
        let next = string_at(&updated, &["UpdateToken"]).to_string();
        assert_ne!(
            next, token,
            "case {label}: expected the update to mint a new UpdateToken, got {updated:?}"
        );

        // The stored document is the one the update sent: a later reader sees
        // the new references, not the old.
        let described = harness
            .ok(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": "swapping-policy" }),
            )
            .await;
        for member in ["StatelessRuleGroupReferences", "StatefulRuleGroupReferences"] {
            assert_eq!(
                described["FirewallPolicy"].get(member),
                document.get(member),
                "case {label}: expected the read to report the new {member}, got {described:?}"
            );
        }

        for (selector, count) in [
            (&selector_a, expected[0]),
            (&selector_b, expected[1]),
            (&selector_c, expected[2]),
        ] {
            let reported = described_associations(&harness, selector).await;
            assert_eq!(
                reported, count,
                "case {label}: expected {selector:?} to be used by {count} policies, got \
                 {reported}"
            );
        }

        // A rule group the new document no longer names is deletable again,
        // while one it names is not.
        for (selector, count) in [
            (&selector_a, expected[0]),
            (&selector_b, expected[1]),
            (&selector_c, expected[2]),
        ] {
            let (status, body) = harness.call("DeleteRuleGroup", selector).await;
            let expected_status = if count > 0 { 400 } else { 200 };
            assert_eq!(
                status, expected_status,
                "case {label}: expected the delete of {selector:?} to answer {expected_status} \
                 while {count} policies use it, got {status} with {body:?}"
            );
            if count == 0 {
                // Put it back, so the next row starts from the same three rule
                // groups.
                let group_type = string_at(selector, &["Type"]).to_string();
                let name = string_at(selector, &["RuleGroupName"]).to_string();
                let capacity = if group_type == "STATELESS" { 20 } else { 10 };
                harness
                    .shared_rule_group(&name, &group_type, capacity)
                    .await;
            }
        }

        // A stale token is refused, and the policy keeps the document the
        // accepted update stored.
        let (status, body) = harness
            .call(
                "UpdateFirewallPolicy",
                &json!({
                    "FirewallPolicyName": "swapping-policy",
                    "UpdateToken": token,
                    "FirewallPolicy": minimal_policy_document(),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label}: expected a stale UpdateToken to be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidTokenException",
            "case {label}: expected InvalidTokenException for a stale token, got {body:?}"
        );
        token = next;
    }
}

/// RULE: the count is of the policies that use THIS rule group, so a policy
/// referencing another rule group — or a second reference list of the same
/// policy — never counts here: a policy naming several rule groups is one
/// association for each of them and none for any other.
#[tokio::test]
async fn only_the_policies_naming_a_rule_group_count_as_its_associations() {
    let harness = Harness::new();

    let stateless_arn = harness
        .shared_rule_group("counted-stateless", "STATELESS", 20)
        .await;
    let stateful_arn = harness
        .shared_rule_group("counted-stateful", "STATEFUL", 10)
        .await;
    // A third rule group no policy ever names, which is the row the count must
    // stay at none for.
    harness
        .shared_rule_group("counted-untouched", "STATEFUL", 10)
        .await;

    // One policy naming BOTH of the first two rule groups, in the two different
    // reference lists.
    harness
        .ok(
            "CreateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "counting-policy",
                "FirewallPolicy": document_referencing(&[&stateless_arn, &stateful_arn]),
            }),
        )
        .await;

    let rows = [
        ("counted-stateless", "STATELESS", 1),
        ("counted-stateful", "STATEFUL", 1),
        ("counted-untouched", "STATEFUL", 0),
    ];
    for (name, group_type, expected) in rows {
        let selector = json!({ "RuleGroupName": name, "Type": group_type });
        let reported = described_associations(&harness, &selector).await;
        assert_eq!(
            reported, expected,
            "case {name} ({group_type}): expected {expected} policies to use it, got {reported}"
        );
    }

    // The rule group no policy names stays deletable while the referenced ones
    // do not.
    harness
        .ok(
            "DeleteRuleGroup",
            &json!({ "RuleGroupName": "counted-untouched", "Type": "STATEFUL" }),
        )
        .await;
    for (name, group_type) in [
        ("counted-stateless", "STATELESS"),
        ("counted-stateful", "STATEFUL"),
    ] {
        let (status, body) = harness
            .call(
                "DeleteRuleGroup",
                &json!({ "RuleGroupName": name, "Type": group_type }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {name} ({group_type}): expected the referenced rule group's delete to be \
             refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidOperationException",
            "case {name} ({group_type}): expected InvalidOperationException, got {body:?}"
        );
    }
}
