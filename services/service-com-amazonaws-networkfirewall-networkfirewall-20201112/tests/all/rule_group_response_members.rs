// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members every `RuleGroupResponse` reports across the
//! whole rule-group lifecycle — create, update, read and delete — with the
//! encryption settings and the tag collection the ones under test.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{string_at, Harness};
use crate::rule_group_lifecycle::{create_request, stateless_any, stateless_group, stateless_rule};

/// The settings a resource protected by the Amazon Web Services owned key
/// reports: `EncryptionType`'s `AWS_OWNED_KMS_KEY` member in BOTH the type and
/// the key identifier (`smithy_query(query="shape",shape="EncryptionType")` for
/// the member; the identifier the AWS-owned key reports is RECORDED behaviour).
fn aws_owned() -> Value {
    json!({ "Type": "AWS_OWNED_KMS_KEY", "KeyId": "AWS_OWNED_KMS_KEY" })
}

/// The settings a resource protected by a customer managed key reports: the
/// `CUSTOMER_KMS` type and the identifier the request gave — "you can use any of
/// the key identifiers that KMS supports" (`EncryptionConfiguration$KeyId`).
fn customer(key_id: &str) -> Value {
    json!({ "Type": "CUSTOMER_KMS", "KeyId": key_id })
}

/// A stateful domain-list document over `targets` (`RulesSourceList` requires
/// `Targets`, `TargetTypes` and `GeneratedRulesType`).
fn domain_list(targets: &[&str]) -> Value {
    json!({
        "RulesSource": {
            "RulesSourceList": {
                "Targets": targets,
                "TargetTypes": ["TLS_SNI"],
                "GeneratedRulesType": "ALLOWLIST",
            },
        },
    })
}

/// A Suricata flat-format rules string with one `pass` rule per entry of `sids`.
fn suricata(sids: &[u32]) -> String {
    sids.iter()
        .map(|sid| format!("pass tcp any any -> any any (sid:{sid};)\n"))
        .collect()
}

/// The `Tag` list a row's key:value pairs make.
fn tag_list(pairs: &[(&str, &str)]) -> Vec<Value> {
    pairs
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect()
}

/// One row of the table: a rule group to create, an update to send it, and the
/// encryption settings every response along the way must report.
#[derive(Debug)]
struct Case {
    /// The row's label, which is also the rule group's name.
    label: &'static str,
    group_type: &'static str,
    capacity: i64,
    /// The create's specification members (`RuleGroup` or `Rules`).
    specification: Value,
    /// The create's `EncryptionConfiguration`, or `None` when it names none.
    requested: Option<Value>,
    /// The settings the create and the read before the update must report.
    reported: Value,
    /// The row's tags, empty when the rule group carries none.
    tags: &'static [(&'static str, &'static str)],
    /// The update's members, beside the identifier and the update token.
    update: Value,
    /// The settings the update, the read after it and the delete must report.
    reported_after_update: Value,
}

/// The table: the rows span the rule group kinds, both ways of giving the
/// specification, how the key is reached (not named, named without a key id,
/// named as a key id / an alias / a key ARN), how many tags the rule group
/// carries, and whether the update re-states the key, rotates it or leaves it
/// alone.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "default-untagged-stateful",
            group_type: "STATEFUL",
            capacity: 10,
            specification: json!({ "Rules": suricata(&[1]) }),
            requested: None,
            reported: aws_owned(),
            tags: &[],
            update: json!({ "Rules": suricata(&[2, 3]) }),
            reported_after_update: aws_owned(),
        },
        Case {
            label: "default-untagged-stateless",
            group_type: "STATELESS",
            capacity: 10,
            specification: json!({ "RuleGroup": stateless_any() }),
            requested: None,
            reported: aws_owned(),
            tags: &[],
            update: json!({
                "RuleGroup": stateless_group(vec![stateless_rule(1, "aws:drop", json!({}))]),
            }),
            reported_after_update: aws_owned(),
        },
        Case {
            label: "default-untagged-stateful-domain",
            group_type: "STATEFUL_DOMAIN",
            capacity: 20,
            specification: json!({ "RuleGroup": domain_list(&[".a.test"]) }),
            requested: None,
            reported: aws_owned(),
            tags: &[],
            update: json!({ "RuleGroup": domain_list(&[".a.test", "b.test"]) }),
            reported_after_update: aws_owned(),
        },
        Case {
            label: "aws-owned-named-one-tag",
            group_type: "STATEFUL",
            capacity: 10,
            specification: json!({ "Rules": suricata(&[4]) }),
            // The type named without a key id: the key still names itself.
            requested: Some(json!({ "Type": "AWS_OWNED_KMS_KEY" })),
            reported: aws_owned(),
            tags: &[("env", "prod")],
            // An update that touches neither the rules nor the key.
            update: json!({ "Description": "after" }),
            reported_after_update: aws_owned(),
        },
        Case {
            label: "customer-key-id-two-tags",
            group_type: "STATEFUL",
            capacity: 10,
            specification: json!({ "Rules": suricata(&[5]) }),
            requested: Some(customer("1234abcd-12ab-34cd-56ef-1234567890ab")),
            reported: customer("1234abcd-12ab-34cd-56ef-1234567890ab"),
            tags: &[("env", "staging"), ("team", "net")],
            update: json!({ "Rules": suricata(&[6, 7]) }),
            reported_after_update: customer("1234abcd-12ab-34cd-56ef-1234567890ab"),
        },
        Case {
            label: "customer-alias-rotated-to-the-default-key",
            group_type: "STATEFUL",
            capacity: 10,
            specification: json!({ "Rules": suricata(&[8]) }),
            requested: Some(customer("alias/net-fw")),
            reported: customer("alias/net-fw"),
            tags: &[],
            // The update names the AWS-owned type without a key id, so the
            // response must report that key's own identifier.
            update: json!({ "EncryptionConfiguration": { "Type": "AWS_OWNED_KMS_KEY" } }),
            reported_after_update: aws_owned(),
        },
        Case {
            label: "customer-arn-rotated-to-another-key-three-tags",
            group_type: "STATEFUL",
            capacity: 10,
            specification: json!({ "Rules": suricata(&[9]) }),
            requested: Some(customer(
                "arn:aws:kms:eu-west-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            )),
            reported: customer(
                "arn:aws:kms:eu-west-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
            tags: &[("a", "1"), ("b", "2"), ("c", "3")],
            update: json!({ "EncryptionConfiguration": customer("alias/rotated") }),
            reported_after_update: customer("alias/rotated"),
        },
    ]
}

/// RULE: every `RuleGroupResponse` a rule-group operation returns reports the key
/// protecting that rule group in BOTH `EncryptionConfiguration` members — the key
/// type and that key's identifier, where the Amazon Web Services owned key (the
/// default: "your data is encrypted by default with an Amazon Web Services owned
/// key") names itself — so no response along the create -> update -> read ->
/// delete path ever leaves `KeyId` absent, whichever call last set the settings.
#[tokio::test]
async fn every_rule_group_response_reports_the_key_in_both_members() {
    let harness = Harness::new();

    for case in cases() {
        let mut create = create_request(
            case.label,
            case.group_type,
            case.capacity,
            &case.specification,
        );
        if let Some(requested) = &case.requested {
            create["EncryptionConfiguration"] = requested.clone();
        }
        let tags = tag_list(case.tags);
        if !tags.is_empty() {
            create["Tags"] = json!(tags);
        }
        let created = harness.ok("CreateRuleGroup", &create).await;
        assert_eq!(
            created["RuleGroupResponse"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the create to report the key in both members, got {created:?}"
        );

        let selector = json!({ "RuleGroupName": case.label, "Type": case.group_type });
        let described = harness.ok("DescribeRuleGroup", &selector).await;
        assert_eq!(
            described["RuleGroupResponse"].get("EncryptionConfiguration"),
            Some(&case.reported),
            "case {case:?}: expected the read before the update to report the stored key, \
             got {described:?}"
        );

        let mut update = selector.clone();
        update["UpdateToken"] = json!(string_at(&described, &["UpdateToken"]));
        for (member, value) in case.update.as_object().expect("an update is an object") {
            update[member] = value.clone();
        }
        let updated = harness.ok("UpdateRuleGroup", &update).await;
        assert_eq!(
            updated["RuleGroupResponse"].get("EncryptionConfiguration"),
            Some(&case.reported_after_update),
            "case {case:?}: expected the update to report the key in both members, got {updated:?}"
        );

        let described = harness.ok("DescribeRuleGroup", &selector).await;
        assert_eq!(
            described["RuleGroupResponse"].get("EncryptionConfiguration"),
            Some(&case.reported_after_update),
            "case {case:?}: expected the read after the update to report the stored key, \
             got {described:?}"
        );

        let deleted = harness.ok("DeleteRuleGroup", &selector).await;
        assert_eq!(
            deleted["RuleGroupResponse"].get("EncryptionConfiguration"),
            Some(&case.reported_after_update),
            "case {case:?}: expected the delete to report the removed rule group's key, \
             got {deleted:?}"
        );
    }
}

/// RULE: a delete answers from the record it REMOVED rather than echoing a
/// request, so it reports that rule group's tag collection as a list — empty when
/// the rule group carries none — while a create and an update report the `Tags`
/// member as the resource holds it, absent when it holds none (RECORDED: the
/// `DeleteRuleGroup` response for an untagged rule group carries `Tags: []`,
/// where the create and the update responses for the same rule group carry no
/// `Tags` member at all).
#[tokio::test]
async fn only_the_delete_reports_an_untagged_rule_group_s_tags_as_an_empty_list() {
    let harness = Harness::new();

    for case in cases() {
        let name = format!("tags-{}", case.label);
        let mut create =
            create_request(&name, case.group_type, case.capacity, &case.specification);
        let tags = tag_list(case.tags);
        let requested_tags = (!tags.is_empty()).then(|| json!(tags.clone()));
        if let Some(requested) = &requested_tags {
            create["Tags"] = requested.clone();
        }
        let created = harness.ok("CreateRuleGroup", &create).await;
        assert_eq!(
            created["RuleGroupResponse"].get("Tags"),
            requested_tags.as_ref(),
            "case {case:?}: expected the create to report the Tags member as the request gave \
             it, got {created:?}"
        );

        let selector = json!({ "RuleGroupName": name, "Type": case.group_type });
        let mut update = selector.clone();
        update["UpdateToken"] = json!(string_at(&created, &["UpdateToken"]));
        for (member, value) in case.update.as_object().expect("an update is an object") {
            update[member] = value.clone();
        }
        let updated = harness.ok("UpdateRuleGroup", &update).await;
        assert_eq!(
            updated["RuleGroupResponse"].get("Tags"),
            requested_tags.as_ref(),
            "case {case:?}: expected the update to report the rule group's tags as stored, \
             got {updated:?}"
        );

        let deleted = harness.ok("DeleteRuleGroup", &selector).await;
        assert_eq!(
            deleted["RuleGroupResponse"].get("Tags"),
            Some(&json!(tags)),
            "case {case:?}: expected the delete to report the removed rule group's tags as a \
             list, got {deleted:?}"
        );
    }
}
