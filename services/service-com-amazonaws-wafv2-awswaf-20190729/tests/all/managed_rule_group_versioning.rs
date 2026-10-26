// SPDX-License-Identifier: CC0-1.0

//! Whether a published managed rule group is offered in NAMED versions, end to end
//! over the wire.
//!
//! The rule asserted here: whether a managed rule group is versioned is reference
//! data the vendor publishes PER GROUP, so every reader of the catalogue reports the
//! value stored for that group rather than the member's modelled default —
//! `ManagedRuleGroupSummary$VersioningSupported` is "indicates whether the managed
//! rule group is versioned. If it is, you can retrieve the versions list by calling
//! ListAvailableManagedRuleGroupVersions", and
//! `ManagedProductDescriptor$IsVersioningSupported` reports the same property of the
//! same group. A rule set its vendor revises and republishes is offered in versions;
//! a group whose content is an address list the vendor keeps continuously up to date
//! is not. Being versioned is independent of being an ADVANCED (intelligent threat
//! mitigation) rule set: every advanced group is versioned, but so is every ordinary
//! rule set.
//!
//! The table below is the per-group expectation, written out rather than composed
//! from the catalogue (the sibling suites in [`crate::available_managed_rule_groups`]
//! and [`crate::managed_products`] compose from it, so they hold for any assignment
//! of the flag; these rows are what pins the assignment itself). Its source is the
//! WAF Developer Guide's "AWS Managed Rules rule groups list": the baseline,
//! use-case-specific and intelligent-threat-mitigation rule-set groups carry a
//! version history, and the two IP-address-list groups do not. It spans every group
//! the catalogue publishes, including the ones no recorded request mentions.
//!
//! Every call goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MANAGED_RULE_GROUP_CATALOGUE;

use crate::web_acl_lifecycle::Waf;

/// Whether each AWS-vendor managed rule group is offered in named versions, by name.
///
/// Source: the WAF Developer Guide's "AWS Managed Rules rule groups list" — the
/// rule-set groups (the baseline and use-case-specific ones, and the
/// intelligent-threat-mitigation ones) are published with a version history, while
/// the IP-address-list groups are updated in place and offer no version to choose
/// between. The member these values are reported through is
/// `ManagedRuleGroupSummary$VersioningSupported`
/// (`smithy_query(query="shape",shape="ManagedRuleGroupSummary")`).
const PUBLISHED_VERSIONING: &[(&str, bool)] = &[
    ("AWSManagedRulesCommonRuleSet", true),
    ("AWSManagedRulesAdminProtectionRuleSet", true),
    ("AWSManagedRulesKnownBadInputsRuleSet", true),
    ("AWSManagedRulesSQLiRuleSet", true),
    ("AWSManagedRulesLinuxRuleSet", true),
    ("AWSManagedRulesUnixRuleSet", true),
    ("AWSManagedRulesWindowsRuleSet", true),
    ("AWSManagedRulesPHPRuleSet", true),
    ("AWSManagedRulesWordPressRuleSet", true),
    ("AWSManagedRulesAmazonIpReputationList", false),
    ("AWSManagedRulesAnonymousIpList", false),
    ("AWSManagedRulesBotControlRuleSet", true),
    ("AWSManagedRulesATPRuleSet", true),
    ("AWSManagedRulesACFPRuleSet", true),
    ("AWSManagedRulesAntiDDoSRuleSet", true),
];

/// The summaries a `ListAvailableManagedRuleGroups` response lists.
fn summaries(response: &Value) -> Vec<Value> {
    response["ManagedRuleGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The summary a response lists for `name`.
fn summary_for(response: &Value, name: &str) -> Value {
    summaries(response)
        .into_iter()
        .find(|summary| summary["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {name:?}: the group must be listed, got {response}"))
}

/// Every published group reports the versioning its vendor publishes for it, in
/// either scope: `true` for the rule-set groups and `false` for the
/// IP-address-list ones, and not one uniform value for the whole catalogue.
///
/// One row per group in [`PUBLISHED_VERSIONING`], crossed with both declared
/// scopes — the flag is a property of the group, so the namespace a request is made
/// in cannot change it.
#[tokio::test]
async fn each_published_group_reports_the_versioning_its_vendor_publishes() {
    // A table of all-equal rows would be satisfied by a uniformly defaulted flag.
    assert!(
        PUBLISHED_VERSIONING
            .iter()
            .any(|(_, versioned)| *versioned)
            && PUBLISHED_VERSIONING
                .iter()
                .any(|(_, versioned)| !*versioned),
        "the table must span both values of the flag, got {PUBLISHED_VERSIONING:?}"
    );

    let waf = Waf::new();
    for scope in ["CLOUDFRONT", "REGIONAL"] {
        let listed = waf
            .ok("ListAvailableManagedRuleGroups", json!({ "Scope": scope }))
            .await;
        for (name, versioned) in PUBLISHED_VERSIONING {
            let case = format!("{scope}/{name}");
            let summary = summary_for(&listed, name);
            assert_eq!(
                summary["VersioningSupported"],
                json!(versioned),
                "case {case:?}: expected VersioningSupported {versioned}, got {}",
                summary["VersioningSupported"]
            );
        }
    }
}

/// The other reader of the same catalogue agrees group by group:
/// `DescribeAllManagedProducts` reports `IsVersioningSupported` for a group exactly
/// as `ListAvailableManagedRuleGroups` reports `VersioningSupported` for it, and
/// being versioned does not make a group ADVANCED — the catalogue publishes groups
/// that are versioned without being advanced.
///
/// One row per group in [`PUBLISHED_VERSIONING`].
#[tokio::test]
async fn the_managed_product_reports_the_same_versioning_per_group() {
    let waf = Waf::new();
    let products = waf
        .ok("DescribeAllManagedProducts", json!({ "Scope": "REGIONAL" }))
        .await;
    let described = products["ManagedProducts"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut versioned_but_not_advanced = 0_usize;
    for (name, versioned) in PUBLISHED_VERSIONING {
        let product = described
            .iter()
            .find(|product| product["ManagedRuleSetName"] == json!(name))
            .unwrap_or_else(|| {
                panic!("case {name:?}: the group must be described, got {products}")
            });
        assert_eq!(
            product["IsVersioningSupported"],
            json!(versioned),
            "case {name:?}: expected IsVersioningSupported {versioned}, got {product}"
        );
        if *versioned && product["IsAdvancedManagedRuleSet"] == json!(false) {
            versioned_but_not_advanced += 1;
        }
    }
    assert!(
        versioned_but_not_advanced > 0,
        "the two flags are independent, so the catalogue must publish a group that \
         is versioned without being advanced, got {products}"
    );
}

/// A group reported as versioned keeps the promise the member's own doc makes: "you
/// can retrieve the versions list by calling ListAvailableManagedRuleGroupVersions",
/// which answers at least one version a web ACL can pin, and a current default
/// among them.
///
/// One row per group the table publishes as versioned.
#[tokio::test]
async fn a_versioned_group_offers_the_versions_list_its_flag_promises() {
    let waf = Waf::new();
    for (name, _) in PUBLISHED_VERSIONING.iter().filter(|(_, versioned)| *versioned) {
        let case = *name;
        let response = waf
            .ok(
                "ListAvailableManagedRuleGroupVersions",
                json!({ "Scope": "REGIONAL", "VendorName": "AWS", "Name": name }),
            )
            .await;
        let offered = response["Versions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !offered.is_empty(),
            "case {case:?}: a versioned group must offer a versions list, got {response}"
        );
        let names: Vec<&str> = offered
            .iter()
            .filter_map(|version| version["Name"].as_str())
            .collect();
        let current = response["CurrentDefaultVersion"].as_str().unwrap_or_default();
        assert!(
            names.contains(&current),
            "case {case:?}: the current default version must be one of the offered \
             ones {names:?}, got {current:?}"
        );
    }
}

/// The table above is the whole published catalogue: every catalogued group is
/// classified by it and carries the classified value in the stored catalogue, and
/// the table names no group the catalogue does not publish — so a group added to the
/// catalogue has to be classified here rather than silently taking a default.
#[test]
fn the_table_classifies_every_catalogued_group() {
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let expected = PUBLISHED_VERSIONING
            .iter()
            .find(|(name, _)| *name == group.name)
            .map(|(_, versioned)| *versioned)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: every published group must be classified by the table",
                    group.name
                )
            });
        assert_eq!(
            group.is_versioning_supported, expected,
            "case {:?}: the catalogue must carry VersioningSupported {expected}, got {}",
            group.name, group.is_versioning_supported
        );
    }
    for (name, _) in PUBLISHED_VERSIONING {
        assert!(
            MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .any(|group| group.name == *name),
            "case {name:?}: the table must name only groups the catalogue publishes"
        );
    }
}
