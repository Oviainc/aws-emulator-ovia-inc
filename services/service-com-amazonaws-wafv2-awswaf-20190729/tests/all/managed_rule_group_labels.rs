// SPDX-License-Identifier: CC0-1.0

//! The LABEL SURFACE a vendor publishes for its managed rule groups, end to end over
//! the wire.
//!
//! The rule asserted here: `DescribeManagedRuleGroup` reports a published group's
//! label surface from the service's managed-rule-group catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`) — the labels its rules ADD as
//! `AvailableLabels` and the labels its rules MATCH AGAINST as `ConsumedLabels`, each
//! FULLY QUALIFIED under the namespace the publishing vendor states for the group
//! (`awswaf:managed:<vendor>:<published slug>:`, whose slug is published and is not
//! the group's API name), with a list the vendor publishes nothing for left OFF
//! rather than answered empty, and with no separate `LabelNamespace` member because
//! the namespace is the prefix those labels already carry.
//!
//! The labels a group publishes are therefore the texts a `LabelMatchStatement`
//! composing against that group is written with: the last suite here DISCOVERS a
//! label through the describe, writes a web ACL rule that matches it, and drives a
//! request that trips the publishing rule through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the
//! SAME shared engine — this service models no data-plane endpoint of its own,
//! because the requests a web ACL decides arrive at the protected resource's
//! endpoint.
//!
//! The tables span the class rather than one group: the first two cross the WHOLE
//! catalogue with both declared scopes, and the last one spans four groups whose
//! rules inspect four different request components.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MANAGED_RULE_GROUP_CATALOGUE;

use crate::web_acl_lifecycle::{visibility, Waf, TEST_REGION};

/// The `Scope` a web ACL in front of a regional resource lives in.
const SCOPE: &str = "REGIONAL";

/// The vendor of every group this suite describes
/// (`ManagedRuleGroupStatement$VendorName`).
const VENDOR: &str = "AWS";

/// The prefix a label of a group AWS itself publishes carries: the
/// `awswaf:managed:<vendor>:` of the syntax
/// `DescribeManagedRuleGroupResponse$LabelNamespace` states, with the vendor segment
/// AWS publishes its own groups under.
const AWS_MANAGED_PREFIX: &str = "awswaf:managed:aws:";

/// The prefix of the labels WAF's own client-token handling publishes, which are
/// vendor-independent: they state what WAF made of the token a request presented
/// rather than what a group's rule inspected.
const TOKEN_PREFIX: &str = "awswaf:managed:token:";

/// A `DescribeManagedRuleGroup` request body for `name` in `scope`.
fn request(scope: &str, name: &str) -> Value {
    json!({ "Scope": scope, "VendorName": VENDOR, "Name": name })
}

/// The label names a response's `member` list reports.
fn reported(response: &Value, member: &str) -> Vec<String> {
    response[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The four-segment namespace of a fully qualified managed label: its first four
/// colon-separated components and the separator that closes a namespace prefix, as
/// in `awswaf:managed:aws:core-rule-set:`.
fn namespace_of(label: &str) -> String {
    let segments: Vec<&str> = label.split(':').take(4).collect();
    format!("{}:", segments.join(":"))
}

/// Whether `label` satisfies the `LabelName` the model gives `LabelSummary$Name`
/// (`length 1..1024`, pattern `^[0-9A-Za-z_\-:]+$`).
fn is_label_name(label: &str) -> bool {
    (1..=1024).contains(&label.len())
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:".contains(&byte))
}

/// Every published group reports its label surface as its vendor publishes it: fully
/// qualified labels under the group's own published namespace, no separate
/// `LabelNamespace`, a consumed list only where the group's rules match on a label,
/// and no label naming the group's API name.
///
/// The table is the WHOLE catalogue, one row per published group, crossed with both
/// declared scopes: it spans groups whose labels name the component a rule inspects,
/// groups whose labels name a list or a bot category, and the groups whose label
/// texts the catalogue cites none of.
#[tokio::test]
async fn every_published_group_reports_its_label_surface_as_published() {
    let waf = Waf::new();
    let mut groups_with_labels = 0_usize;
    let mut groups_with_consumed = 0_usize;
    let mut groups_without_labels = 0_usize;

    for scope in [SCOPE, "CLOUDFRONT"] {
        for group in MANAGED_RULE_GROUP_CATALOGUE {
            let case = format!("{scope} {}/{}", group.vendor_name, group.name);
            let response = waf
                .ok("DescribeManagedRuleGroup", request(scope, group.name))
                .await;
            let available = reported(&response, "AvailableLabels");
            let consumed = reported(&response, "ConsumedLabels");
            let namespace = group.managed_label_namespace();

            // The namespace is the prefix the labels carry, so it is not reported a
            // second time as a member of its own.
            assert!(
                response["LabelNamespace"].is_null(),
                "case {case:?}: a vendor's own group reports no separate label \
                 namespace, got {response}"
            );

            if available.is_empty() {
                if scope == SCOPE {
                    groups_without_labels += 1;
                }
                // A list the vendor publishes nothing for is left off, not answered
                // as a list with nothing in it.
                assert!(
                    response["AvailableLabels"].is_null()
                        && response["ConsumedLabels"].is_null(),
                    "case {case:?}: a group whose label texts nobody published \
                     reports no label list at all, got {response}"
                );
                continue;
            }
            if scope == SCOPE {
                groups_with_labels += 1;
            }

            for label in &available {
                assert!(
                    is_label_name(label),
                    "case {case:?}: the published label {label:?} must satisfy \
                     LabelName, got {response}"
                );
                assert!(
                    label.starts_with(AWS_MANAGED_PREFIX) || label.starts_with(TOKEN_PREFIX),
                    "case {case:?}: expected a fully qualified managed label, \
                     got {label:?}"
                );
                assert!(
                    !label.contains(group.name),
                    "case {case:?}: a published label carries the group's published \
                     slug, never its API name, got {label:?}"
                );
                let label_namespace = namespace_of(label);
                assert!(
                    label_namespace == namespace || label.starts_with(TOKEN_PREFIX),
                    "case {case:?}: expected the label {label:?} under {namespace:?} \
                     or the vendor-independent token namespace, got \
                     {label_namespace:?}"
                );
            }
            assert!(
                available
                    .iter()
                    .any(|label| namespace_of(label) == namespace),
                "case {case:?}: the group must publish labels of its own namespace \
                 {namespace:?}, got {available:?}"
            );

            if consumed.is_empty() {
                assert!(
                    response["ConsumedLabels"].is_null(),
                    "case {case:?}: a group whose rules match on no label reports no \
                     consumed list rather than an empty one, got {response}"
                );
                continue;
            }
            if scope == SCOPE {
                groups_with_consumed += 1;
            }
            let published: BTreeSet<&String> = available.iter().collect();
            for label in &consumed {
                assert_eq!(
                    namespace_of(label),
                    namespace,
                    "case {case:?}: a consumed label of the group sits in its own \
                     namespace, got {label:?}"
                );
                assert!(
                    published.contains(label),
                    "case {case:?}: a label the group matches against is one it \
                     publishes, got {label:?} outside {available:?}"
                );
            }
        }
    }

    // The table only spans the class if the catalogue publishes groups of each kind.
    assert!(
        groups_with_labels > 1 && groups_with_consumed > 0 && groups_without_labels > 0,
        "expected the catalogue to span groups that publish labels ({groups_with_labels}), \
         groups that match on labels ({groups_with_consumed}) and groups whose label \
         texts nobody published ({groups_without_labels})"
    );
}

/// A published group's labels are ITS OWN: every group publishes its labels under a
/// namespace no other group publishes under, so a label of one group is never
/// reported for another and the namespace tells the groups apart.
///
/// The table is the whole catalogue: every ordered pair of distinct groups that
/// publish labels is compared.
#[tokio::test]
async fn no_two_published_groups_share_a_label_or_a_namespace() {
    let waf = Waf::new();
    let mut published: Vec<(&str, String, BTreeSet<String>)> = Vec::new();
    for group in MANAGED_RULE_GROUP_CATALOGUE {
        let response = waf
            .ok("DescribeManagedRuleGroup", request(SCOPE, group.name))
            .await;
        let namespace = group.managed_label_namespace();
        let own: BTreeSet<String> = reported(&response, "AvailableLabels")
            .into_iter()
            .filter(|label| namespace_of(label) == namespace)
            .collect();
        if own.is_empty() {
            continue;
        }
        published.push((group.name, namespace, own));
    }
    assert!(
        published.len() > 1,
        "the catalogue must publish labels for more than one group for this table to \
         compare any, got {}",
        published.len()
    );

    for (name, namespace, labels) in &published {
        for (other_name, other_namespace, other_labels) in &published {
            if name == other_name {
                continue;
            }
            assert_ne!(
                namespace, other_namespace,
                "case {name:?} vs {other_name:?}: two groups must publish their \
                 labels under different namespaces"
            );
            let shared: Vec<&String> = labels.intersection(other_labels).collect();
            assert!(
                shared.is_empty(),
                "case {name:?} vs {other_name:?}: no label is published by two \
                 groups, got {shared:?}"
            );
        }
    }
}

/// Every published group's notification topic is the ONE topic its publisher
/// announces changes on: an SNS ARN in the region that topic lives in, named for the
/// managed-rule notifications, whatever group is described and whatever region or
/// scope the request was made in.
#[tokio::test]
async fn the_reported_notification_topic_is_the_one_published_topic() {
    let waf = Waf::new();
    for scope in [SCOPE, "CLOUDFRONT"] {
        for group in MANAGED_RULE_GROUP_CATALOGUE {
            let case = format!("{scope} {}", group.name);
            let response = waf
                .ok("DescribeManagedRuleGroup", request(scope, group.name))
                .await;
            let arn = response["SnsTopicArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an SNS topic ARN, got {response}"))
                .to_string();
            let segments: Vec<&str> = arn.split(':').collect();
            assert_eq!(
                segments.get(..4).map(<[&str]>::to_vec),
                Some(vec!["arn", "aws", "sns", "us-east-1"]),
                "case {case:?}: expected the published topic's partition, service and \
                 region, got {arn:?}"
            );
            assert_eq!(
                segments.last().copied(),
                Some("aws-managed-waf-rule-notifications"),
                "case {case:?}: expected the managed-rule notification topic, \
                 got {arn:?}"
            );
            assert_eq!(
                segments.get(4).copied(),
                Some(::engine::emulator_account_id()),
                "case {case:?}: expected the emulator's account, got {arn:?}"
            );
        }
    }
}

/// One row of the composition table: the group to run, the rule of it whose
/// inspection this data plane performs, the last segment of the label that rule
/// publishes, and the two requests — one that trips the rule and one that does not.
struct Composed {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The managed rule group the web ACL runs.
    group: &'static str,
    /// The last segment of the published label the matching rule adds, which is what
    /// the row looks up in the described `AvailableLabels`.
    leaf: &'static str,
    /// The request that trips the publishing rule.
    tripping: Probe,
    /// A request that does not.
    control: Probe,
}

/// One request a row drives.
struct Probe {
    method: &'static str,
    path: &'static str,
    query: &'static str,
    /// The `User-Agent` the request carries, or none at all.
    user_agent: Option<&'static str>,
}

impl Probe {
    /// This probe as the edge inspects it.
    fn inspected(&self) -> InspectedRequest {
        InspectedRequest {
            method: self.method.to_string(),
            uri_path: self.path.to_string(),
            query_string: self.query.to_string(),
            headers: self
                .user_agent
                .map(|value| vec![("user-agent".to_string(), value.to_string())])
                .unwrap_or_default(),
            body: Vec::new(),
            client_ip: "198.51.100.5".to_string(),
        }
    }
}

/// A stub protected resource: an API Gateway REST API stage, the one ARN form
/// `AssociateWebACLRequest$ResourceArn` documents for one. Each row binds its own, so
/// no two web ACLs decide one request.
fn stage_arn(row: usize) -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/c1d2e3f4a{row}/stages/prod")
}

/// The published label whose last segment is `leaf`, as the describe reports it.
fn discovered(response: &Value, leaf: &str, case: &str) -> String {
    reported(response, "AvailableLabels")
        .into_iter()
        .find(|label| label.rsplit(':').next() == Some(leaf))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the describe must publish a label ending {leaf:?}, got {response}")
        })
}

/// The labels a group publishes are the labels a web ACL can MATCH: a rule written
/// against a label discovered through `DescribeManagedRuleGroup` matches exactly the
/// requests the publishing rule matched, under the namespace the describe reported it
/// with.
///
/// Each row runs its group with `OverrideAction` `Count`, so the group's own rules
/// only count and the label-matching rule that follows them is what settles the
/// request. The rows differ in the group, in the request component the publishing
/// rule inspects (an absent header, a query argument, the path, the method) and in
/// the shape of the published label.
#[tokio::test]
async fn a_published_label_is_the_label_a_matching_rule_sees() {
    let cases = [
        Composed {
            case: "the core rule set's rule for a request with no User-Agent",
            group: "AWSManagedRulesCommonRuleSet",
            leaf: "NoUserAgent_Header",
            tripping: Probe {
                method: "GET",
                path: "/prod/orders",
                query: "",
                user_agent: None,
            },
            control: Probe {
                method: "GET",
                path: "/prod/orders",
                query: "",
                user_agent: Some("integration-test"),
            },
        },
        Composed {
            case: "the SQL-database group's rule for an injected query argument",
            group: "AWSManagedRulesSQLiRuleSet",
            leaf: "SQLi_QueryArguments",
            tripping: Probe {
                method: "GET",
                path: "/prod/orders",
                query: "q=%27%20OR%201%3D1%20--",
                user_agent: Some("integration-test"),
            },
            control: Probe {
                method: "GET",
                path: "/prod/orders",
                query: "q=lamps",
                user_agent: Some("integration-test"),
            },
        },
        Composed {
            case: "the admin-protection group's rule for an administrative path",
            group: "AWSManagedRulesAdminProtectionRuleSet",
            leaf: "AdminProtection_URIPath",
            tripping: Probe {
                method: "GET",
                path: "/prod/admin",
                query: "",
                user_agent: Some("integration-test"),
            },
            control: Probe {
                method: "GET",
                path: "/prod/orders",
                query: "",
                user_agent: Some("integration-test"),
            },
        },
        Composed {
            case: "the known-bad-inputs group's rule for the PROPFIND method",
            group: "AWSManagedRulesKnownBadInputsRuleSet",
            leaf: "PROPFIND_Method",
            tripping: Probe {
                method: "PROPFIND",
                path: "/prod/orders",
                query: "",
                user_agent: Some("integration-test"),
            },
            control: Probe {
                method: "GET",
                path: "/prod/orders",
                query: "",
                user_agent: Some("integration-test"),
            },
        },
    ];

    let (waf, shared) = Waf::sharing_engine();
    let mut fronts: Vec<(String, String)> = Vec::new();
    for (row, case) in cases.iter().enumerate() {
        // Discovery: the label the rule is written against is read off the describe,
        // exactly as a client composing against the group reads it.
        let described = waf
            .ok("DescribeManagedRuleGroup", request(SCOPE, case.group))
            .await;
        let label = discovered(&described, case.leaf, case.case);

        let name = format!("composed-{row}");
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": &name,
                    "Scope": SCOPE,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&name),
                    "Rules": [
                        {
                            "Name": "group",
                            "Priority": 0,
                            "Statement": {
                                "ManagedRuleGroupStatement": {
                                    "VendorName": VENDOR,
                                    "Name": case.group,
                                }
                            },
                            "OverrideAction": { "Count": {} },
                            "VisibilityConfig": visibility("group"),
                        },
                        {
                            "Name": "on-label",
                            "Priority": 1,
                            "Statement": {
                                "LabelMatchStatement": { "Scope": "LABEL", "Key": &label }
                            },
                            "Action": { "Block": {} },
                            "VisibilityConfig": visibility("onlabel"),
                        },
                    ],
                }),
            )
            .await;
        let arn = created["Summary"]["ARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: CreateWebACL must answer Summary.ARN", case.case))
            .to_string();
        let stage = stage_arn(row);
        waf.ok(
            "AssociateWebACL",
            json!({ "WebACLArn": arn, "ResourceArn": &stage }),
        )
        .await;
        fronts.push((label, stage));
    }

    let state = service_com_amazonaws_wafv2_awswaf_20190729::state::service_state(&*shared)
        .expect("the wafv2 store is reachable from the shared engine");

    for (case, (label, stage)) in cases.iter().zip(fronts.iter()) {
        for (probe, expected_block) in [(&case.tripping, true), (&case.control, false)] {
            let decision = edge::decide(&state, stage, &probe.inspected()).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the associated web ACL must decide the request",
                    case.case
                )
            });
            let blocked = !decision.action.is_allow();
            assert_eq!(
                blocked, expected_block,
                "case {:?}: a rule matching the published label {label:?} must settle \
                 the {} request; expected blocked={expected_block}, got the action {:?}",
                case.case,
                if expected_block { "tripping" } else { "control" },
                decision.action
            );
            if expected_block {
                assert!(
                    decision.labels.iter().any(|carried| carried == label),
                    "case {:?}: the request must carry the published label {label:?}, \
                     got {:?}",
                    case.case,
                    decision.labels
                );
            } else {
                assert!(
                    !decision.labels.iter().any(|carried| carried == label),
                    "case {:?}: a request the publishing rule did not match carries \
                     no such label, got {:?}",
                    case.case,
                    decision.labels
                );
            }
        }
    }
}
