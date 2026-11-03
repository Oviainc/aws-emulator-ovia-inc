// SPDX-License-Identifier: CC0-1.0

//! `DescribeManagedRuleGroup`, end to end over the wire.
//!
//! The rule asserted here: `DescribeManagedRuleGroup` reports what the VENDOR
//! publishes about the group a `(VendorName, Name)` pair names — the capacity a web
//! ACL spends on running it, the rules it runs with the action each takes, the
//! namespace every label those rules add carries, and the version being described
//! (the one the request names, or the vendor's default when it names none) — all of
//! it read from the service's managed-rule-group catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`), the same catalogue that
//! prices a `ManagedRuleGroupStatement` and that
//! `ListAvailableManagedRuleGroupVersions` lists the versions from. A pair the
//! catalogue does not list names nothing that exists, and a version its vendor does
//! not offer is nothing WAF can describe.
//!
//! Every expectation below is composed FROM that catalogue rather than written as a
//! literal, and the tables span every group in it, both declared scopes and every
//! version each group offers. Every call goes through the router's front door with
//! the harness in [`crate::web_acl_lifecycle`], so what these tests observe is what
//! an SDK observes.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    ManagedRuleGroupDescriptor, ManagedVersionState, PublishedRuleAction,
    MANAGED_RULE_GROUP_CATALOGUE,
};

use crate::managed_rule_groups::managed_statement;
use crate::web_acl_lifecycle::{visibility, Waf};

/// A `DescribeManagedRuleGroup` request body for `(vendor, name)` in `scope`.
fn request(scope: &str, vendor: &str, name: &str) -> Value {
    json!({ "Scope": scope, "VendorName": vendor, "Name": name })
}

/// The notification topic every change to an AWS-published managed rule group is
/// announced on: ONE topic, in the region its publisher publishes it in, whatever
/// group is asked about and whatever region the caller signed for. Written out here
/// rather than imported so the table does not restate the code under test.
fn expected_topic() -> String {
    format!(
        "arn:aws:sns:us-east-1:{}:aws-managed-waf-rule-notifications",
        ::engine::emulator_account_id()
    )
}

/// The `RuleAction` a published action is reported as, written out here rather than
/// imported so the table does not restate the code under test: `RuleAction` carries
/// exactly the one member the action names, with no custom response or request
/// handling.
fn expected_action(action: PublishedRuleAction) -> Value {
    match action {
        PublishedRuleAction::Block => json!({ "Block": {} }),
        PublishedRuleAction::Count => json!({ "Count": {} }),
        PublishedRuleAction::Challenge => json!({ "Challenge": {} }),
    }
}

/// The rules a response reports, as `(Name, Action)` pairs in the reported order.
fn reported_rules(response: &Value) -> Vec<(String, Value)> {
    response["Rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            (
                rule["Name"].as_str().unwrap_or_default().to_string(),
                rule["Action"].clone(),
            )
        })
        .collect()
}

/// The label names a response's `AvailableLabels` reports.
fn reported_labels(response: &Value, member: &str) -> Vec<String> {
    response[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every group the catalogue publishes describes ITSELF: its published capacity, its
/// rules with the action each takes, the namespace its labels carry, the labels its
/// rules add, the topic its changes are announced on and the vendor's default
/// version — and the same description in either scope, because a managed rule group
/// is a vendor's publication and a scope is only the namespace a request is made in.
///
/// The table is the WHOLE catalogue, one row per published group, crossed with both
/// declared scopes: it spans versioned and unversioned groups, rule lists from one
/// rule to twenty-odd, and blocking, counting and challenging rules.
#[tokio::test]
async fn every_catalogued_group_is_described_from_the_published_catalogue() {
    let waf = Waf::new();
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        for group in MANAGED_RULE_GROUP_CATALOGUE {
            let case = format!("{scope} {}/{}", group.vendor_name, group.name);
            let response = waf
                .ok(
                    "DescribeManagedRuleGroup",
                    request(scope, group.vendor_name, group.name),
                )
                .await;

            assert_eq!(
                response["Capacity"],
                json!(group.capacity),
                "case {case:?}: expected the published capacity {}, got {response}",
                group.capacity
            );
            assert_eq!(
                response["VersionName"],
                json!(group.current_default_version()),
                "case {case:?}: a request naming no version describes the vendor's \
                 default, got {response}"
            );

            // One topic for every announced change, exactly as
            // `DescribeAllManagedProducts` reports it.
            let expected_topic = expected_topic();
            assert_eq!(
                response["SnsTopicArn"],
                json!(expected_topic),
                "case {case:?}: expected the notification topic {expected_topic:?}, \
                 got {response}"
            );

            // A vendor publishes its own group's labels FULLY QUALIFIED, so the
            // namespace is the prefix they carry and is not reported a second time
            // on its own.
            assert!(
                response["LabelNamespace"].is_null(),
                "case {case:?}: a vendor's own group reports no separate label \
                 namespace, got {response}"
            );

            let expected_rules: Vec<(String, Value)> = group
                .rules
                .iter()
                .map(|rule| (rule.name.to_string(), expected_action(rule.action)))
                .collect();
            assert!(
                !expected_rules.is_empty(),
                "case {case:?}: every published group runs at least one rule"
            );
            assert_eq!(
                reported_rules(&response),
                expected_rules,
                "case {case:?}: expected the published rules {expected_rules:?}, \
                 got {response}"
            );

            // "The labels that one or more rules in this rule group add to matching
            // web requests": the labels the catalogue CITES for the group, each
            // fully qualified under the group's published namespace, plus the
            // vendor-independent token labels it publishes. A group the catalogue
            // cites none for reports no such member at all.
            let namespace = group.managed_label_namespace();
            let expected_labels: BTreeSet<String> = group
                .rules
                .iter()
                .filter_map(|rule| rule.label().map(|label| format!("{namespace}{label}")))
                .chain(group.token_labels.iter().map(|label| (*label).to_string()))
                .collect();
            assert_eq!(
                reported_labels(&response, "AvailableLabels")
                    .into_iter()
                    .collect::<BTreeSet<String>>(),
                expected_labels,
                "case {case:?}: expected exactly the cited labels of the group's \
                 rules, got {response}"
            );
            if expected_labels.is_empty() {
                assert!(
                    response["AvailableLabels"].is_null(),
                    "case {case:?}: a group whose labels nobody published reports no \
                     list rather than an empty one, got {response}"
                );
            }

            // "The labels that one or more rules in this rule group match against in
            // label match statements": the matched labels the catalogue cites, and
            // no member at all for a group that matches on none.
            let expected_consumed: BTreeSet<String> = group
                .rules
                .iter()
                .filter_map(|rule| {
                    rule.consumed_label()
                        .map(|label| format!("{namespace}{label}"))
                })
                .collect();
            assert_eq!(
                reported_labels(&response, "ConsumedLabels")
                    .into_iter()
                    .collect::<BTreeSet<String>>(),
                expected_consumed,
                "case {case:?}: expected exactly the cited matched labels, \
                 got {response}"
            );
            if expected_consumed.is_empty() {
                assert!(
                    response["ConsumedLabels"].is_null(),
                    "case {case:?}: a group whose rules match on no label reports no \
                     consumed list rather than an empty one, got {response}"
                );
            }
        }
    }
}

/// The capacity this call reports for a group is the capacity a web ACL SPENDS on
/// running it: `CheckCapacity` for one rule that runs the group answers the same
/// figure the describe reports, because both read the group's own published
/// capacity.
///
/// The table is the whole catalogue, so it spans every capacity the catalogue
/// publishes, from the smallest to the largest.
#[tokio::test]
async fn the_reported_capacity_is_what_a_web_acl_spends_on_running_the_group() {
    let waf = Waf::new();
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        let described = waf
            .ok(
                "DescribeManagedRuleGroup",
                request("REGIONAL", group.vendor_name, group.name),
            )
            .await;
        let rules = json!([{
            "Name": "managed",
            "Priority": 0,
            "Statement": managed_statement(group.vendor_name, group.name),
            "OverrideAction": { "None": {} },
            "VisibilityConfig": visibility("managed"),
        }]);
        let priced = waf
            .ok(
                "CheckCapacity",
                json!({ "Scope": "REGIONAL", "Rules": rules }),
            )
            .await;
        assert_eq!(
            described["Capacity"], priced["Capacity"],
            "case {case:?}: the described capacity {} must be what a rule running the \
             group costs, got {}",
            described["Capacity"], priced["Capacity"]
        );
    }
}

/// Every version a vendor offers of a group can be described, and the version
/// reported is the one described: the named one, or the vendor's default when the
/// request names none. What the describe reports about the group itself — its
/// capacity and its rules — is the group's, so it is reported whichever version was
/// asked for.
///
/// The table is the whole catalogue crossed with every version each group offers,
/// plus the request that names none, so a group offering three versions contributes
/// four rows.
#[tokio::test]
async fn every_offered_version_can_be_described_and_a_request_naming_none_gets_the_default() {
    let waf = Waf::new();
    let mut rows = 0_usize;
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let offered: Vec<Option<&str>> = std::iter::once(None)
            .chain(group.versions.iter().map(|release| Some(release.name)))
            .collect();
        for version in offered {
            rows += 1;
            let case = format!("{}@{version:?}", group.name);
            let mut body = request("REGIONAL", group.vendor_name, group.name);
            if let Some(version) = version {
                body["VersionName"] = json!(version);
            }
            let response = waf.ok("DescribeManagedRuleGroup", body).await;
            let expected = version.or_else(|| group.current_default_version());
            assert_eq!(
                response["VersionName"],
                json!(expected),
                "case {case:?}: expected the described version {expected:?}, \
                 got {response}"
            );
            assert_eq!(
                response["Capacity"],
                json!(group.capacity),
                "case {case:?}: the capacity is the group's, so every version reports \
                 it, got {response}"
            );
            assert_eq!(
                reported_rules(&response).len(),
                group.rules.len(),
                "case {case:?}: the rules are the group's, so every version reports \
                 them, got {response}"
            );
        }
    }
    assert!(
        rows > MANAGED_RULE_GROUP_CATALOGUE.len(),
        "every group must contribute its default row plus one per offered version, \
         got {rows} rows"
    );
}

/// A version its vendor does not publish at all is nothing WAF can describe, so the
/// call is refused with the modelled `WAFInvalidParameterException` naming the
/// managed rule group and carrying the rejected version — and the refusal describes
/// nothing. A version the vendor published and has since RETIRED is a different
/// class, answered by `WAFExpiredManagedRuleGroupVersionException` (see
/// [`crate::managed_rule_group_version_expiry`]), so no row here names one.
///
/// The rows differ in HOW the version is wrong: a name no vendor publishes, an
/// offered name in the wrong case, a name offered for another group but neither
/// offered nor retired for this one, and a name that is merely a prefix of an
/// offered one.
#[tokio::test]
async fn a_version_the_vendor_does_not_offer_cannot_be_described() {
    // A group that offers exactly one version, and a version another group offers
    // but this one neither offers nor has retired: the catalogue publishes both
    // kinds of entry.
    let single = MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .find(|group| group.versions.len() == 1)
        .expect("the catalogue publishes a group with one offered version");
    let elsewhere = MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .flat_map(|group| group.versions.iter())
        .map(|release| release.name)
        .find(|name| single.version_state(name) == ManagedVersionState::Unknown)
        .expect("another group offers a version this one never published");
    let offered = single
        .current_default_version()
        .expect("the group offers a version");

    let cases: Vec<(&str, String)> = vec![
        ("a version no vendor publishes", "Version_999.0".to_string()),
        ("an offered version in the wrong case", offered.to_lowercase()),
        (
            "a version offered for another group only",
            elsewhere.to_string(),
        ),
        (
            "a prefix of an offered version",
            offered[..offered.len() - 1].to_string(),
        ),
    ];

    let waf = Waf::new();
    for (case, version) in cases {
        let mut body = request("REGIONAL", single.vendor_name, single.name);
        body["VersionName"] = json!(&version);
        let response = waf
            .refused(
                "DescribeManagedRuleGroup",
                body,
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            response["Field"],
            json!("MANAGED_RULE_SET"),
            "case {case:?}: the refusal must name the managed rule group, got {response}"
        );
        assert_eq!(
            response["Parameter"],
            json!(&version),
            "case {case:?}: the refusal must carry the rejected version, got {response}"
        );
        assert!(
            response["Capacity"].is_null() && response["Rules"].is_null(),
            "case {case:?}: a refusal must describe nothing, got {response}"
        );
    }
}

/// A `(VendorName, Name)` pair no vendor publishes names nothing that exists, so the
/// call is refused with the modelled `WAFNonexistentItemException` — whichever member
/// is wrong, exactly as a `ManagedRuleGroupStatement` naming an uncatalogued pair is
/// refused.
#[tokio::test]
async fn a_pair_the_catalogue_does_not_list_cannot_be_described() {
    let catalogued = MANAGED_RULE_GROUP_CATALOGUE[0];
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a name no vendor publishes",
            catalogued.vendor_name.to_string(),
            "AWSManagedRulesNoSuchRuleSet".to_string(),
        ),
        (
            "a catalogued name under an unknown vendor",
            "SomeOtherVendor".to_string(),
            catalogued.name.to_string(),
        ),
        (
            "a catalogued name whose case does not match",
            catalogued.vendor_name.to_string(),
            catalogued.name.to_lowercase(),
        ),
        (
            "the vendor name in the wrong case",
            catalogued.vendor_name.to_lowercase(),
            catalogued.name.to_string(),
        ),
    ];

    let waf = Waf::new();
    for (case, vendor, name) in cases {
        let response = waf
            .refused(
                "DescribeManagedRuleGroup",
                request("REGIONAL", &vendor, &name),
                "WAFNonexistentItemException",
                case,
            )
            .await;
        assert!(
            response["Capacity"].is_null() && response["Rules"].is_null(),
            "case {case:?}: a refusal must describe nothing, got {response}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused, and the
/// refusal describes nothing. One row per shape of a wrong value — an unknown word,
/// a declared value in the wrong case, one with a trailing space, and the empty
/// string.
///
/// `Scope` is a modelled enum, so the value set is enforced at the request boundary
/// and the refusal names the member; the handler's own `is_known_scope` guard covers
/// anything that gets past it.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    let group = MANAGED_RULE_GROUP_CATALOGUE[0];
    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let response = waf
            .refused(
                "DescribeManagedRuleGroup",
                request(scope, group.vendor_name, group.name),
                "ValidationException",
                scope,
            )
            .await;
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("'scope'"),
            "case {scope:?}: the refusal must name the member at fault, got {response}"
        );
        assert!(
            response["Capacity"].is_null(),
            "case {scope:?}: a refusal must describe nothing, got {response}"
        );
    }
}

/// Describing a managed rule group REPORTS: two identical reads answer identically,
/// and the describe leaves what every other reader of the same catalogue reports
/// untouched — the versions on offer and the managed products.
#[tokio::test]
async fn describing_a_managed_rule_group_changes_nothing() {
    let waf = Waf::new();
    let group = MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .max_by_key(|group| group.versions.len())
        .expect("the catalogue publishes at least one group");
    let body = request("REGIONAL", group.vendor_name, group.name);

    let versions_before = waf
        .ok(
            "ListAvailableManagedRuleGroupVersions",
            body.clone(),
        )
        .await;
    let products_before = waf
        .ok("DescribeAllManagedProducts", json!({ "Scope": "REGIONAL" }))
        .await;

    let first = waf.ok("DescribeManagedRuleGroup", body.clone()).await;
    let second = waf.ok("DescribeManagedRuleGroup", body.clone()).await;
    assert_eq!(
        first, second,
        "two identical reads must answer identically, got {first} then {second}"
    );

    let versions_after = waf
        .ok(
            "ListAvailableManagedRuleGroupVersions",
            body.clone(),
        )
        .await;
    assert_eq!(
        versions_before, versions_after,
        "a describe must leave the offered versions untouched, got {versions_before} \
         then {versions_after}"
    );
    let products_after = waf
        .ok("DescribeAllManagedProducts", json!({ "Scope": "REGIONAL" }))
        .await;
    assert_eq!(
        products_before, products_after,
        "a describe must leave the managed products untouched, got {products_before} \
         then {products_after}"
    );
}

/// The published rules the describes above compose their expectations from are well
/// formed: every group runs at least one rule, each rule name satisfies the
/// `EntityName` the model gives `RuleSummary$Name` (`length 1..128`, pattern
/// `^[\w\-]+$`), no group names a rule twice, each rule's label satisfies the
/// `LabelName` the model gives `LabelSummary$Name` (`length 1..1024`, pattern
/// `^[0-9A-Za-z_\-:]+$`) for every label the catalogue cites, and every rule
/// reports exactly one action.
#[test]
fn the_published_rules_are_well_formed() {
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let case = format!("{}/{}", group.vendor_name, group.name);
        assert!(
            !group.rules.is_empty(),
            "case {case:?}: every published group runs at least one rule"
        );
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for rule in group.rules {
            assert!(
                (1..=128).contains(&rule.name.len())
                    && rule
                        .name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)),
                "case {case:?}: rule {:?} must satisfy EntityName",
                rule.name
            );
            assert!(
                seen.insert(rule.name),
                "case {case:?}: a rule name is published once, got {:?} twice",
                rule.name
            );
            // A label is reported only where the catalogue cites the published
            // text; where it does, the fully qualified label must satisfy the
            // modelled `LabelName`.
            if let Some(label) = rule.label() {
                let qualified = format!("{}{label}", group.managed_label_namespace());
                assert!(
                    (1..=1024).contains(&qualified.len())
                        && qualified
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:".contains(&byte)),
                    "case {case:?}: the label {qualified:?} of rule {:?} must satisfy LabelName",
                    rule.name
                );
                assert!(
                    !qualified.contains(group.name),
                    "case {case:?}: a published label carries the group's published \
                     slug, not its API name, got {qualified:?}"
                );
            }
            // A matched label is one of the group's own published labels, so a rule
            // that matches on one also publishes it.
            if let Some(matched) = rule.consumed_label() {
                assert_eq!(
                    Some(matched),
                    rule.label(),
                    "case {case:?}: rule {:?} matches on a label the group publishes",
                    rule.name
                );
            }
            let action = rule.action.rule_action();
            let present = [
                action.block.is_some(),
                action.allow.is_some(),
                action.count.is_some(),
                action.captcha.is_some(),
                action.challenge.is_some(),
                action.monetize.is_some(),
            ]
            .into_iter()
            .filter(|present| *present)
            .count();
            assert_eq!(
                present, 1,
                "case {case:?}: rule {:?} must take exactly one action, got {action:?}",
                rule.name
            );
        }
        // The namespace every one of those labels carries, which qualifies each of
        // them in what the describe publishes.
        let namespace = group.managed_label_namespace();
        assert!(
            namespace.starts_with("awswaf:managed:aws:") && namespace.ends_with(':'),
            "case {case:?}: expected a managed label namespace prefix, got {namespace:?}"
        );
    }

    // The two descriptor readings the handler relies on, directly: a version the
    // group offers resolves to itself, one it does not resolves to nothing, and a
    // request naming none resolves to the vendor's default.
    let group: &ManagedRuleGroupDescriptor = &MANAGED_RULE_GROUP_CATALOGUE[0];
    for release in group.versions {
        assert_eq!(
            group.offered_version(Some(release.name)),
            Some(release.name),
            "an offered version must resolve to itself, got {:?}",
            release.name
        );
    }
    assert_eq!(
        group.offered_version(None),
        group.current_default_version(),
        "a request naming no version must resolve to the vendor's default"
    );
    assert_eq!(
        group.offered_version(Some("Version_999.0")),
        None,
        "a version no vendor offers must resolve to nothing"
    );
}
