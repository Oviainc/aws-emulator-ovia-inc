// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a stored record is PROJECTED onto a response: the
//! encryption settings every resource reports, and the tag collection a delete
//! reports for the resource it removed.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{minimal_policy_document, string_at, Harness};

/// One row of the encryption table: what the create request asked for, and the
/// settings every response for that resource must then report.
#[derive(Debug)]
struct EncryptionCase {
    label: &'static str,
    /// The request's `EncryptionConfiguration`, or `None` when it names none.
    requested: Option<Value>,
    /// The `EncryptionConfiguration` every response must carry.
    reported: Value,
}

/// The encryption table.
///
/// The AWS-owned rows differ in HOW the default key is reached (not named at
/// all, named without a key id, named with it); the customer-managed rows differ
/// in WHICH key identifier form the request uses — "you can use any of the key
/// identifiers that KMS supports" (`EncryptionConfiguration$KeyId`), i.e. a key
/// id, an alias and a key ARN.
fn encryption_cases() -> Vec<EncryptionCase> {
    let aws_owned = json!({ "Type": "AWS_OWNED_KMS_KEY", "KeyId": "AWS_OWNED_KMS_KEY" });
    let customer = |key_id: &str| json!({ "Type": "CUSTOMER_KMS", "KeyId": key_id });
    vec![
        EncryptionCase {
            label: "no-configuration",
            requested: None,
            reported: aws_owned.clone(),
        },
        EncryptionCase {
            label: "aws-owned-without-key-id",
            requested: Some(json!({ "Type": "AWS_OWNED_KMS_KEY" })),
            reported: aws_owned.clone(),
        },
        EncryptionCase {
            label: "aws-owned-with-key-id",
            requested: Some(aws_owned.clone()),
            reported: aws_owned,
        },
        EncryptionCase {
            label: "customer-key-id",
            requested: Some(customer("1234abcd-12ab-34cd-56ef-1234567890ab")),
            reported: customer("1234abcd-12ab-34cd-56ef-1234567890ab"),
        },
        EncryptionCase {
            label: "customer-key-alias",
            requested: Some(customer("alias/net-fw")),
            reported: customer("alias/net-fw"),
        },
        EncryptionCase {
            label: "customer-key-arn",
            requested: Some(customer(
                "arn:aws:kms:eu-west-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            )),
            reported: customer(
                "arn:aws:kms:eu-west-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        },
    ]
}

/// A minimal stateless rule-group document: one rule that matches anything.
fn stateless_document() -> Value {
    json!({ "RulesSource": { "StatelessRulesAndCustomActions": {
        "StatelessRules": [{
            "Priority": 1,
            "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
        }],
    }}})
}

/// A minimal stateful rule-group document: one Suricata rule line.
fn stateful_document() -> Value {
    json!({ "RulesSource": { "RulesString": "pass tcp any any -> any any (sid:1;)\n" } })
}

/// The `Tag` list a table row's key:value pairs make.
fn tag_list(pairs: &[(&str, &str)]) -> Vec<Value> {
    pairs
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect()
}

/// RULE: a response reports BOTH `EncryptionConfiguration` members for the key
/// protecting the resource — the type of key and that key's identifier — so a
/// resource encrypted with the Amazon Web Services owned key (the default when
/// the request names no configuration, "your data is encrypted by default with
/// an Amazon Web Services owned key") reports that key's own identifier, and a
/// resource encrypted with a customer managed key reports the identifier the
/// request gave; the settings a create reports are the settings every later read
/// and the delete report.
#[tokio::test]
async fn every_resource_reports_the_key_that_protects_it_in_both_members() {
    let harness = Harness::new();

    for case in encryption_cases() {
        // A firewall policy, two rule groups (both kinds) and a firewall, each
        // named for the row so the rows cannot observe each other.
        let policy_name = format!("enc-policy-{}", case.label);
        let mut policy_request = json!({
            "FirewallPolicyName": policy_name,
            "FirewallPolicy": minimal_policy_document(),
        });
        if let Some(requested) = &case.requested {
            policy_request["EncryptionConfiguration"] = requested.clone();
        }
        let policy = harness.ok("CreateFirewallPolicy", &policy_request).await;
        let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
        assert_eq!(
            policy["FirewallPolicyResponse"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the firewall policy's create response to report the key, \
             got {policy:?}"
        );

        let firewall_name = format!("enc-firewall-{}", case.label);
        let mut firewall_request = json!({
            "FirewallName": firewall_name,
            "FirewallPolicyArn": policy_arn,
            "VpcId": "vpc-0123abcd",
            "SubnetMappings": [{ "SubnetId": format!("subnet-{}", case.label) }],
        });
        if let Some(requested) = &case.requested {
            firewall_request["EncryptionConfiguration"] = requested.clone();
        }
        let firewall = harness.ok("CreateFirewall", &firewall_request).await;
        assert_eq!(
            firewall["Firewall"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the firewall's create response to report the key, \
             got {firewall:?}"
        );

        for (group_type, document) in [
            ("STATELESS", stateless_document()),
            ("STATEFUL", stateful_document()),
        ] {
            let group_name = format!("enc-{}-{}", group_type.to_lowercase(), case.label);
            let mut group_request = json!({
                "RuleGroupName": group_name,
                "Type": group_type,
                "Capacity": 10,
                "RuleGroup": document,
            });
            if let Some(requested) = &case.requested {
                group_request["EncryptionConfiguration"] = requested.clone();
            }
            let created = harness.ok("CreateRuleGroup", &group_request).await;
            assert_eq!(
                created["RuleGroupResponse"].get("EncryptionConfiguration"),
                Some(&case.reported),
                "case {case:?} {group_type}: expected the rule group's create response to \
                 report the key, got {created:?}"
            );

            let selector = json!({ "RuleGroupName": group_name, "Type": group_type });
            let described = harness.ok("DescribeRuleGroup", &selector).await;
            assert_eq!(
                described["RuleGroupResponse"].get("EncryptionConfiguration"),
                Some(&case.reported),
                "case {case:?} {group_type}: expected the read to report the stored key, \
                 got {described:?}"
            );
            let deleted = harness.ok("DeleteRuleGroup", &selector).await;
            assert_eq!(
                deleted["RuleGroupResponse"].get("EncryptionConfiguration"),
                Some(&case.reported),
                "case {case:?} {group_type}: expected the delete to report the removed rule \
                 group's key, got {deleted:?}"
            );
        }

        // The firewall goes before the policy it uses: a policy in use cannot be
        // deleted.
        let described = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": firewall_name }))
            .await;
        assert_eq!(
            described["Firewall"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the read to report the firewall's stored key, \
             got {described:?}"
        );
        let deleted = harness
            .ok("DeleteFirewall", &json!({ "FirewallName": firewall_name }))
            .await;
        assert_eq!(
            deleted["Firewall"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the delete to report the removed firewall's key, \
             got {deleted:?}"
        );

        let described = harness
            .ok(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name }),
            )
            .await;
        assert_eq!(
            described["FirewallPolicyResponse"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the read to report the policy's stored key, \
             got {described:?}"
        );
        let deleted = harness
            .ok(
                "DeleteFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name }),
            )
            .await;
        assert_eq!(
            deleted["FirewallPolicyResponse"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the delete to report the removed policy's key, \
             got {deleted:?}"
        );
    }
}

/// RULE: a delete answers from the record it removed rather than echoing a
/// request, so it reports that resource's tag collection as a LIST — empty when
/// the resource carries no tags — while a create echoes the request's own
/// optional `Tags` member, which is absent when the request named none.
#[tokio::test]
async fn a_delete_reports_the_removed_resource_s_tags_as_a_list() {
    let harness = Harness::new();

    // Rows differ in how many tags the resource carries, with the empty row the
    // one that separates "no tags" from "tags not reported".
    let cases: [(&str, &[(&str, &str)]); 4] = [
        ("untagged", &[]),
        ("one-tag", &[("env", "prod")]),
        ("two-tags", &[("env", "staging"), ("team", "net")]),
        ("three-tags", &[("a", "1"), ("b", "2"), ("c", "3")]),
    ];

    for (label, pairs) in cases {
        let tags = tag_list(pairs);
        let requested_tags = (!tags.is_empty()).then(|| json!(tags.clone()));
        let reported_tags = json!(tags);

        let policy_name = format!("tag-policy-{label}");
        let mut policy_request = json!({
            "FirewallPolicyName": policy_name,
            "FirewallPolicy": minimal_policy_document(),
        });
        if let Some(requested) = &requested_tags {
            policy_request["Tags"] = requested.clone();
        }
        let policy = harness.ok("CreateFirewallPolicy", &policy_request).await;
        assert_eq!(
            policy["FirewallPolicyResponse"].get("Tags"),
            requested_tags.as_ref(),
            "case {label}: expected the create to echo the request's Tags member, got {policy:?}"
        );
        let policy_arn =
            string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

        let firewall_name = format!("tag-firewall-{label}");
        let mut firewall_request = json!({
            "FirewallName": firewall_name,
            "FirewallPolicyArn": policy_arn,
            "VpcId": "vpc-0123abcd",
            "SubnetMappings": [{ "SubnetId": format!("subnet-tag-{label}") }],
        });
        if let Some(requested) = &requested_tags {
            firewall_request["Tags"] = requested.clone();
        }
        let firewall = harness.ok("CreateFirewall", &firewall_request).await;
        assert_eq!(
            firewall["Firewall"].get("Tags"),
            requested_tags.as_ref(),
            "case {label}: expected the create to echo the request's Tags member, got {firewall:?}"
        );

        let group_name = format!("tag-rule-group-{label}");
        let mut group_request = json!({
            "RuleGroupName": group_name,
            "Type": "STATEFUL",
            "Capacity": 10,
            "RuleGroup": stateful_document(),
        });
        if let Some(requested) = &requested_tags {
            group_request["Tags"] = requested.clone();
        }
        let created = harness.ok("CreateRuleGroup", &group_request).await;
        assert_eq!(
            created["RuleGroupResponse"].get("Tags"),
            requested_tags.as_ref(),
            "case {label}: expected the create to echo the request's Tags member, got {created:?}"
        );

        // Every delete reports the removed resource's tag collection, the empty
        // row included.
        let deleted = harness
            .ok(
                "DeleteRuleGroup",
                &json!({ "RuleGroupName": group_name, "Type": "STATEFUL" }),
            )
            .await;
        assert_eq!(
            deleted["RuleGroupResponse"].get("Tags"),
            Some(&reported_tags),
            "case {label}: expected the removed rule group's tags as a list, got {deleted:?}"
        );

        let deleted = harness
            .ok("DeleteFirewall", &json!({ "FirewallName": firewall_name }))
            .await;
        assert_eq!(
            deleted["Firewall"].get("Tags"),
            Some(&reported_tags),
            "case {label}: expected the removed firewall's tags as a list, got {deleted:?}"
        );

        let deleted = harness
            .ok(
                "DeleteFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name }),
            )
            .await;
        assert_eq!(
            deleted["FirewallPolicyResponse"].get("Tags"),
            Some(&reported_tags),
            "case {label}: expected the removed policy's tags as a list, got {deleted:?}"
        );
    }
}
