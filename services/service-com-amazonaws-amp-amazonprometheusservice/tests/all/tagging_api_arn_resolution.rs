// SPDX-License-Identifier: CC0-1.0

//! The ARN a resource's OWN reader hands a caller is the ARN the `/tags/{resourceArn}`
//! operations resolve, end to end over HTTP.
//!
//! The rule under test: an ARN-addressed tagging operation resolves `resourceArn`
//! against the resources this store holds, whatever KIND of taggable resource the ARN
//! names, so the ARN any reader of that resource reports — the create response, the
//! describe, the listing — reads back that resource's own tag map, the same map its
//! reader reports.
//!
//! `tests/all/taggable_resource_kinds.rs` drives the merge-and-remove table over every
//! kind from the ARN each create returned. This suite pins the other half of the same
//! rule: that every reader of one resource reports ONE ARN and each of those ARNs
//! resolves, over a table of rule groups namespaces differing in the name the ARN's last
//! segment carries and in the size of the tag map — the empty map and the modelled
//! `TagMap` maximum included — and over a resource whose ARN carries another region.
//!
//! The harness (`Amp`, `Reply`, `tags_of`, `text`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` builds.

use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::TAGS_PER_RESOURCE_LIMIT;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const REGION: &str = "us-west-2";

/// A second signing region, so the table covers a resource whose ARN does not carry
/// [`REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// A rules file every namespace in this suite is created with: the content is beside the
/// point here, only the tags and the ARN are.
fn rules_data() -> String {
    aws_smithy_types::base64::encode(b"groups: []\n".as_slice())
}

fn tag_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn wire_tags(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// One entry of a listing, found by the member that identifies it.
fn summary_of(listed: &Reply, member: &str, key: &str, value: &str) -> Value {
    listed.body[member]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| text(&entry[key]) == value)
                .cloned()
        })
        .unwrap_or_else(|| panic!("the listing must report {value:?}, got {listed:?}"))
}

/// One row of the namespace table: the name it is created under and the tags it carries.
///
/// The rows differ along what the rule generalises over: the shape and the length of the
/// name the ARN's last segment carries, and the number of tags the resource holds.
/// `RuleGroupsNamespaceName` is length 1..128 with pattern `[0-9A-Za-z][-.0-9A-Z_a-z]*`
/// and `TagMap` holds at most 50 entries
/// (`smithy_query(query="shape",shape="RuleGroupsNamespaceName,TagMap")`), so the table
/// spans a one-character name, the punctuation the pattern allows, a digits-only name and
/// the 128-character maximum, with an empty tag map at one end and the full 50 at the
/// other.
struct NamespaceCase {
    case: &'static str,
    name: String,
    tags: BTreeMap<String, String>,
}

fn namespace_cases() -> Vec<NamespaceCase> {
    let every_tag_allowed: BTreeMap<String, String> = (0..TAGS_PER_RESOURCE_LIMIT)
        .map(|index| (format!("seed-{index:02}"), format!("value-{index:02}")))
        .collect();
    vec![
        NamespaceCase {
            case: "the shortest name the model allows, carrying no tags at all",
            name: "a".to_string(),
            tags: BTreeMap::new(),
        },
        NamespaceCase {
            case: "a name carrying the dot, dash and underscore the pattern allows",
            name: "team.obs_rules-2026".to_string(),
            tags: tag_map(&[("team", "obs")]),
        },
        NamespaceCase {
            case: "a digits-only name with three tags",
            name: "1234567890".to_string(),
            tags: tag_map(&[("cost", "42"), ("env", "prod"), ("owner", "sre")]),
        },
        NamespaceCase {
            case: "a name at the modelled 128-character maximum, with the TagMap maximum of tags",
            name: format!("n{}", "a".repeat(127)),
            tags: every_tag_allowed,
        },
    ]
}

/// Every reader of one resource reports the SAME ARN, and each of those ARNs — whichever
/// reader a caller got it from — resolves in `ListTagsForResource` to the tag map that
/// resource carries.
async fn assert_every_reported_arn_resolves(
    amp: &Amp,
    region: &str,
    case: &str,
    reported: &[(&str, String)],
    expected: &BTreeMap<String, String>,
) {
    let (first_reader, first_arn) = reported
        .first()
        .expect("every row names at least one reader");
    for (reader, arn) in reported {
        assert_eq!(
            arn, first_arn,
            "case {case:?}: {reader} must report the ARN {first_reader} reports \
             ({first_arn:?}), got {arn:?}"
        );
        let listed = amp.list_tags(region, arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the ARN {reader} reports must resolve, got {listed:?}"
        );
        assert_eq!(
            &tags_of(&listed.body["tags"]),
            expected,
            "case {case:?}: ListTagsForResource must report {expected:?} for the ARN \
             {reader} reports, got {listed:?}"
        );
    }
}

/// The ARN every reader of a workspace, a rule groups namespace and an anomaly detector
/// reports resolves in the tagging API, and reports that resource's own tags.
#[tokio::test]
async fn every_reported_arn_resolves_in_the_tagging_api() {
    let amp = Amp::new();

    let workspace_tags = tag_map(&[("owner", "platform")]);
    let created_workspace = amp
        .call(
            "POST",
            "/workspaces",
            REGION,
            &json!({ "alias": "reported-arns", "tags": wire_tags(&workspace_tags) }).to_string(),
        )
        .await;
    assert_eq!(
        created_workspace.status, 202,
        "CreateWorkspace must answer its modelled 202, got {created_workspace:?}"
    );
    let workspace_id = text(&created_workspace.body["workspaceId"]);
    // A child resource may be created only on an ACTIVE workspace, so let the clock
    // settle the workspace first.
    wait_for_due_transition();

    // Create every child resource next, then let the clock settle them all at once.
    let cases = namespace_cases();
    let mut created_namespaces: Vec<Reply> = Vec::new();
    for case in &cases {
        let created = amp
            .call(
                "POST",
                &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
                REGION,
                &json!({
                    "name": case.name,
                    "data": rules_data(),
                    "tags": wire_tags(&case.tags),
                })
                .to_string(),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: CreateRuleGroupsNamespace must answer its modelled 202, got {created:?}",
            case.case
        );
        created_namespaces.push(created);
    }

    let detector_tags = tag_map(&[("env", "dev"), ("team", "obs")]);
    let created_detector = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/anomalydetectors"),
            REGION,
            &json!({
                "alias": "requests",
                "configuration": {
                    "randomCutForest": { "query": "sum(rate(requests_total[5m]))" }
                },
                "tags": wire_tags(&detector_tags),
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        created_detector.status, 202,
        "CreateAnomalyDetector must answer its modelled 202, got {created_detector:?}"
    );
    let detector_id = text(&created_detector.body["anomalyDetectorId"]);
    wait_for_due_transition();

    let described_workspace = amp
        .call("GET", &format!("/workspaces/{workspace_id}"), REGION, "")
        .await;
    assert_eq!(
        described_workspace.status, 200,
        "DescribeWorkspace must answer 200, got {described_workspace:?}"
    );
    let listed_workspaces = amp.call("GET", "/workspaces", REGION, "").await;
    let workspace_summary = summary_of(
        &listed_workspaces,
        "workspaces",
        "workspaceId",
        &workspace_id,
    );
    assert_every_reported_arn_resolves(
        &amp,
        REGION,
        "a workspace",
        &[
            ("CreateWorkspace", text(&created_workspace.body["arn"])),
            (
                "DescribeWorkspace",
                text(&described_workspace.body["workspace"]["arn"]),
            ),
            ("ListWorkspaces", text(&workspace_summary["arn"])),
        ],
        &workspace_tags,
    )
    .await;

    let listed_namespaces = amp
        .call(
            "GET",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            REGION,
            "",
        )
        .await;
    assert_eq!(
        listed_namespaces.status, 200,
        "ListRuleGroupsNamespaces must answer 200, got {listed_namespaces:?}"
    );
    for (case, created) in cases.iter().zip(&created_namespaces) {
        let described = amp
            .call(
                "GET",
                &format!(
                    "/workspaces/{workspace_id}/rulegroupsnamespaces/{}",
                    case.name
                ),
                REGION,
                "",
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeRuleGroupsNamespace must answer 200, got {described:?}",
            case.case
        );
        assert_eq!(
            tags_of(&described.body["ruleGroupsNamespace"]["tags"]),
            case.tags,
            "case {:?}: DescribeRuleGroupsNamespace must report the stored tags {:?}, \
             got {described:?}",
            case.case,
            case.tags
        );
        let summary = summary_of(&listed_namespaces, "ruleGroupsNamespaces", "name", &case.name);
        assert_every_reported_arn_resolves(
            &amp,
            REGION,
            case.case,
            &[
                ("CreateRuleGroupsNamespace", text(&created.body["arn"])),
                (
                    "DescribeRuleGroupsNamespace",
                    text(&described.body["ruleGroupsNamespace"]["arn"]),
                ),
                ("ListRuleGroupsNamespaces", text(&summary["arn"])),
            ],
            &case.tags,
        )
        .await;
    }

    let described_detector = amp
        .call(
            "GET",
            &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
            REGION,
            "",
        )
        .await;
    assert_eq!(
        described_detector.status, 200,
        "DescribeAnomalyDetector must answer 200, got {described_detector:?}"
    );
    assert_every_reported_arn_resolves(
        &amp,
        REGION,
        "an anomaly detector",
        &[
            ("CreateAnomalyDetector", text(&created_detector.body["arn"])),
            (
                "DescribeAnomalyDetector",
                text(&described_detector.body["anomalyDetector"]["arn"]),
            ),
        ],
        &detector_tags,
    )
    .await;

    // Reading every child's tags moved no tag of the parent workspace: each ARN reads
    // the map of the resource it names and nothing else.
    let workspace_after = amp
        .call("GET", &format!("/workspaces/{workspace_id}"), REGION, "")
        .await;
    assert_eq!(
        tags_of(&workspace_after.body["workspace"]["tags"]),
        workspace_tags,
        "case \"a workspace\": the workspace must still carry only its own tags {workspace_tags:?}, \
         got {workspace_after:?}"
    );
}

/// A resource whose ARN carries another region resolves by that ARN too: the tagging API
/// resolves `resourceArn` against the stored records, so the region the ARN carries is
/// whatever the call that created the resource was signed for.
#[tokio::test]
async fn a_resource_created_in_another_region_resolves_by_its_own_arn() {
    let amp = Amp::new();
    let workspace_tags = tag_map(&[("region", "ireland")]);
    let created_workspace = amp
        .call(
            "POST",
            "/workspaces",
            OTHER_REGION,
            &json!({ "alias": "other-region", "tags": wire_tags(&workspace_tags) }).to_string(),
        )
        .await;
    assert_eq!(
        created_workspace.status, 202,
        "CreateWorkspace must answer its modelled 202, got {created_workspace:?}"
    );
    let workspace_id = text(&created_workspace.body["workspaceId"]);
    wait_for_due_transition();

    let namespace_tags = tag_map(&[("team", "obs")]);
    let created_namespace = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            OTHER_REGION,
            &json!({
                "name": "rules1",
                "data": rules_data(),
                "tags": wire_tags(&namespace_tags),
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        created_namespace.status, 202,
        "CreateRuleGroupsNamespace must answer its modelled 202, got {created_namespace:?}"
    );
    wait_for_due_transition();

    let described = amp
        .call(
            "GET",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/rules1"),
            OTHER_REGION,
            "",
        )
        .await;
    assert_eq!(
        described.status, 200,
        "DescribeRuleGroupsNamespace must answer 200, got {described:?}"
    );
    let namespace_arn = text(&described.body["ruleGroupsNamespace"]["arn"]);
    assert!(
        namespace_arn.contains(&format!(":{OTHER_REGION}:")),
        "case \"another region\": the ARN must carry the region the create was signed for, \
         got {namespace_arn:?}"
    );

    assert_every_reported_arn_resolves(
        &amp,
        OTHER_REGION,
        "a rule groups namespace created in another region",
        &[
            (
                "CreateRuleGroupsNamespace",
                text(&created_namespace.body["arn"]),
            ),
            ("DescribeRuleGroupsNamespace", namespace_arn),
        ],
        &namespace_tags,
    )
    .await;
    assert_every_reported_arn_resolves(
        &amp,
        OTHER_REGION,
        "a workspace created in another region",
        &[("CreateWorkspace", text(&created_workspace.body["arn"]))],
        &workspace_tags,
    )
    .await;
}
