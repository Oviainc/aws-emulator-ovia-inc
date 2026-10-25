// SPDX-License-Identifier: CC0-1.0

//! The `/tags/{resourceArn}` operations against EVERY kind of resource this service
//! persists a tag map for, end to end over HTTP.
//!
//! The rule under test: an ARN-addressed tagging operation resolves `resourceArn`
//! against the resources the store holds, whatever kind of resource the ARN names, and
//! reads or writes THAT resource's own tag map — so a merge appends a key it does not
//! carry and replaces the value of one it does, a removal drops the keys it names, and
//! only an ARN naming no resource the store holds is refused.
//!
//! `TagResource` enumerates the kinds — "The only resources that can be tagged are rule
//! groups namespaces, scrapers, and workspaces" — and an anomaly detector carries a tag
//! map too (`CreateAnomalyDetector` accepts `tags`, `PutAnomalyDetector` carries none),
//! so the table below has one row per kind and the recorded case (a rule groups
//! namespace) is one of them.
//!
//! The harness (`Amp`, `Reply`, `text`, `tags_of`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` builds, and the seeded scraper record is the
//! builder `tests/all/scraper_listing.rs` already owns: `CreateScraper` is still
//! scaffolded (its own stub issue), so seeding is how a scraper ARN exists at all.

use crate::scraper_listing::{eks_source, scraper, workspace_destination};
use crate::workspace_lifecycle::{tags_of, text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    RESOURCE_NOT_FOUND_CODE, STATUS_ACTIVE, TAGS_PER_RESOURCE_LIMIT,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const REGION: &str = "us-west-2";

/// One taggable resource, named the way the store reaches it, so the suite can read its
/// tags back through the operation that reports that KIND of resource.
#[derive(Debug, Clone)]
enum Taggable {
    Workspace {
        workspace_id: String,
    },
    RuleGroupsNamespace {
        workspace_id: String,
        name: String,
    },
    AnomalyDetector {
        workspace_id: String,
        detector_id: String,
    },
    Scraper {
        scraper_id: String,
    },
}

/// One row of the table: a resource kind, the ARN that addresses it and the tags it was
/// created carrying.
struct Row {
    case: &'static str,
    arn: String,
    resource: Taggable,
    tags: BTreeMap<String, String>,
}

/// What a request sends to `/tags/{resourceArn}` at one step of the table.
enum Action {
    /// `TagResource` with this `tags` map.
    Merge(Value),
    /// `UntagResource` with these `tagKeys`.
    Remove(&'static [&'static str]),
}

/// One step applied to every row, in order, with the tag map every reader must then
/// report. The steps span what the rule generalises over beside the resource kind: the
/// number of keys in one call, whether each is new or already carried, and whether a
/// removal names a key the resource has.
struct Step {
    case: &'static str,
    action: Action,
    expected: &'static [(&'static str, &'static str)],
}

fn map_of(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn json_map(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
            .collect(),
    )
}

/// The tags the resource's OWN reader reports — the operation a caller of that kind of
/// resource uses, not the tagging API, so the two cannot disagree.
async fn reported_tags(amp: &Amp, case: &str, resource: &Taggable) -> BTreeMap<String, String> {
    match resource {
        Taggable::Workspace { workspace_id } => {
            let described = amp
                .call("GET", &format!("/workspaces/{workspace_id}"), REGION, "")
                .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribeWorkspace must answer 200, got {described:?}"
            );
            tags_of(&described.body["workspace"]["tags"])
        }
        Taggable::RuleGroupsNamespace { workspace_id, name } => {
            let described = amp
                .call(
                    "GET",
                    &format!("/workspaces/{workspace_id}/rulegroupsnamespaces/{name}"),
                    REGION,
                    "",
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribeRuleGroupsNamespace must answer 200, got {described:?}"
            );
            let described_tags = tags_of(&described.body["ruleGroupsNamespace"]["tags"]);
            // The listing reports the same stored record, so it reports the same map.
            let listed = amp
                .call(
                    "GET",
                    &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
                    REGION,
                    "",
                )
                .await;
            let summary = listed.body["ruleGroupsNamespaces"]
                .as_array()
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| &text(&entry["name"]) == name)
                        .cloned()
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report the namespace, got {listed:?}")
                });
            assert_eq!(
                tags_of(&summary["tags"]),
                described_tags,
                "case {case:?}: ListRuleGroupsNamespaces must report the tags \
                 DescribeRuleGroupsNamespace reports ({described_tags:?}), got {summary}"
            );
            described_tags
        }
        Taggable::AnomalyDetector {
            workspace_id,
            detector_id,
        } => {
            let described = amp
                .call(
                    "GET",
                    &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
                    REGION,
                    "",
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: DescribeAnomalyDetector must answer 200, got {described:?}"
            );
            tags_of(&described.body["anomalyDetector"]["tags"])
        }
        Taggable::Scraper { scraper_id } => {
            let listed = amp.call("GET", "/scrapers", REGION, "").await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: ListScrapers must answer 200, got {listed:?}"
            );
            let summary = listed.body["scrapers"]
                .as_array()
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| &text(&entry["scraperId"]) == scraper_id)
                        .cloned()
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report the scraper, got {listed:?}")
                });
            tags_of(&summary["tags"])
        }
    }
}

/// One of every taggable kind, each created (or, for a scraper, seeded) carrying the
/// same two tags, so the table's steps mean the same thing on every row.
async fn one_of_every_kind(amp: &Amp) -> Vec<Row> {
    let created_tags = [("env", "dev"), ("team", "obs")];
    let created_workspace = amp
        .call(
            "POST",
            "/workspaces",
            REGION,
            &json!({ "alias": "tagged", "tags": json_map(&created_tags) }).to_string(),
        )
        .await;
    assert_eq!(
        created_workspace.status, 202,
        "CreateWorkspace must answer its modelled 202, got {created_workspace:?}"
    );
    let workspace_id = text(&created_workspace.body["workspaceId"]);
    let workspace_arn = text(&created_workspace.body["arn"]);
    wait_for_due_transition();

    let namespace_name = "rulesfile1";
    let namespace = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            REGION,
            &json!({
                "name": namespace_name,
                "data": aws_smithy_types::base64::encode(b"groups: []\n".as_slice()),
                "tags": json_map(&created_tags),
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        namespace.status, 202,
        "CreateRuleGroupsNamespace must answer its modelled 202, got {namespace:?}"
    );
    let namespace_arn = text(&namespace.body["arn"]);

    let detector = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/anomalydetectors"),
            REGION,
            &json!({
                "alias": "requests",
                "configuration": { "randomCutForest": { "query": "sum(rate(requests_total[5m]))" } },
                "tags": json_map(&created_tags),
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        detector.status, 202,
        "CreateAnomalyDetector must answer its modelled 202, got {detector:?}"
    );
    let detector_id = text(&detector.body["anomalyDetectorId"]);
    let detector_arn = text(&detector.body["arn"]);

    let mut seeded = scraper(
        "scraper-tagged",
        Some("collector"),
        STATUS_ACTIVE,
        eks_source("metrics-cluster"),
        workspace_destination(&workspace_id),
    );
    seeded.tags = map_of(&created_tags);
    let scraper_arn = seeded.arn.clone();
    amp.store()
        .lock()
        .scrapers
        .insert(seeded.scraper_id.clone(), seeded.clone());

    // Both child resources are created in a transient status; let the clock settle them
    // so every row is read back from a resource in its terminal status.
    wait_for_due_transition();

    vec![
        Row {
            case: "a workspace ARN",
            arn: workspace_arn,
            resource: Taggable::Workspace {
                workspace_id: workspace_id.clone(),
            },
            tags: map_of(&created_tags),
        },
        Row {
            case: "a rule groups namespace ARN",
            arn: namespace_arn,
            resource: Taggable::RuleGroupsNamespace {
                workspace_id: workspace_id.clone(),
                name: namespace_name.to_string(),
            },
            tags: map_of(&created_tags),
        },
        Row {
            case: "an anomaly detector ARN",
            arn: detector_arn,
            resource: Taggable::AnomalyDetector {
                workspace_id: workspace_id.clone(),
                detector_id,
            },
            tags: map_of(&created_tags),
        },
        Row {
            case: "a scraper ARN",
            arn: scraper_arn,
            resource: Taggable::Scraper {
                scraper_id: seeded.scraper_id,
            },
            tags: map_of(&created_tags),
        },
    ]
}

/// Every taggable kind's ARN resolves, and the merge and the removal act on THAT
/// resource's stored tag map: both the tagging API and the resource's own reader report
/// the result, and no other resource's tags move.
#[tokio::test]
async fn every_taggable_kind_resolves_and_carries_the_tags_it_is_given() {
    let amp = Amp::new();
    let mut rows = one_of_every_kind(&amp).await;

    let steps = [
        Step {
            case: "a key the resource does not carry is appended",
            action: Action::Merge(json!({ "owner": "sre" })),
            expected: &[("env", "dev"), ("owner", "sre"), ("team", "obs")],
        },
        Step {
            case: "a key the resource carries has its value replaced",
            action: Action::Merge(json!({ "env": "prod" })),
            expected: &[("env", "prod"), ("owner", "sre"), ("team", "obs")],
        },
        Step {
            case: "one call appends and replaces at once",
            action: Action::Merge(json!({ "env": "stage", "cost": "42" })),
            expected: &[
                ("cost", "42"),
                ("env", "stage"),
                ("owner", "sre"),
                ("team", "obs"),
            ],
        },
        Step {
            case: "a merge of no tags leaves the map as it was",
            action: Action::Merge(json!({})),
            expected: &[
                ("cost", "42"),
                ("env", "stage"),
                ("owner", "sre"),
                ("team", "obs"),
            ],
        },
        Step {
            case: "a removal drops the key it names",
            action: Action::Remove(&["team"]),
            expected: &[("cost", "42"), ("env", "stage"), ("owner", "sre")],
        },
        Step {
            case: "removing a key the resource does not carry changes nothing",
            action: Action::Remove(&["team"]),
            expected: &[("cost", "42"), ("env", "stage"), ("owner", "sre")],
        },
        Step {
            case: "a removal naming several keys drops all of them",
            action: Action::Remove(&["cost", "owner"]),
            expected: &[("env", "stage")],
        },
    ];

    for step in &steps {
        for row in &mut rows {
            let case = format!("{} / {}", row.case, step.case);
            let reply = match &step.action {
                Action::Merge(tags) => amp.tag(REGION, &row.arn, tags).await,
                Action::Remove(keys) => amp.untag(REGION, &row.arn, keys).await,
            };
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the tagging operation must answer 200, got {reply:?}"
            );
            let expected = map_of(step.expected);
            let listed = amp.list_tags(REGION, &row.arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: ListTagsForResource must answer 200, got {listed:?}"
            );
            assert_eq!(
                tags_of(&listed.body["tags"]),
                expected,
                "case {case:?}: ListTagsForResource must report {expected:?}, got {listed:?}"
            );
            let reported = reported_tags(&amp, &case, &row.resource).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the resource's own reader must report {expected:?}, \
                 got {reported:?}"
            );
            row.tags = expected;
        }
    }

    // Each row's final map is still its own: tagging one resource moved no other
    // resource's tags, whatever kind either is.
    for row in &rows {
        let reported = reported_tags(&amp, row.case, &row.resource).await;
        assert_eq!(
            reported, row.tags,
            "case {:?}: the resource must still carry only its own tags {:?}, got {reported:?}",
            row.case, row.tags
        );
    }
}

/// An ARN that names no resource the store holds is refused by all three operations,
/// whatever kind of resource it looks like, and writes nothing.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_refused() {
    let amp = Amp::new();
    let rows = one_of_every_kind(&amp).await;
    let account = engine::emulator_account_id();
    let workspace_id = match &rows[0].resource {
        Taggable::Workspace { workspace_id } => workspace_id.clone(),
        other => panic!("the first row is the workspace row, got {other:?}"),
    };

    let unknown = [
        (
            "a workspace the account does not hold",
            format!("arn:aws:aps:{REGION}:{account}:workspace/ws-00000000-0000-0000-0000-000000000000"),
        ),
        (
            "a rule groups namespace the workspace does not hold",
            format!("arn:aws:aps:{REGION}:{account}:rulegroupsnamespace/{workspace_id}/never-created"),
        ),
        (
            "an anomaly detector the workspace does not hold",
            format!(
                "arn:aws:aps:{REGION}:{account}:anomalydetector/{workspace_id}/ad-00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "a scraper the account does not hold",
            format!("arn:aws:aps:{REGION}:{account}:scraper/scraper-never-created"),
        ),
        (
            "an ARN of another service's resource",
            format!("arn:aws:kms:{REGION}:{account}:key/1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d"),
        ),
    ];

    for (kind, arn) in &unknown {
        let replies = [
            ("TagResource", amp.tag(REGION, arn, &json!({ "a": "b" })).await),
            ("UntagResource", amp.untag(REGION, arn, &["a"]).await),
            ("ListTagsForResource", amp.list_tags(REGION, arn).await),
        ];
        for (operation, reply) in replies {
            let case = format!("{kind} / {operation}");
            assert_eq!(
                reply.status, 404,
                "case {case:?}: an ARN naming no stored resource must be refused 404, \
                 got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                RESOURCE_NOT_FOUND_CODE,
                "case {case:?}: expected the code this service answers on the \
                 ARN-addressed path, got {reply:?}"
            );
        }
    }

    for row in &rows {
        let reported = reported_tags(&amp, row.case, &row.resource).await;
        assert_eq!(
            reported, row.tags,
            "case {:?}: a refused request must leave every stored tag map alone, \
             expected {:?}, got {reported:?}",
            row.case, row.tags
        );
    }
}

/// The per-resource tag bound is enforced on the resource the ARN NAMES, not on one
/// kind of resource: a merge that would carry a rule groups namespace past it is
/// refused and stores nothing, while replacing a value it already carries still works.
#[tokio::test]
async fn the_tag_bound_is_enforced_on_the_resource_the_arn_names() {
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(REGION, "bounded").await;
    let seeded: Vec<(String, String)> = (0..TAGS_PER_RESOURCE_LIMIT)
        .map(|index| (format!("seed-{index:02}"), format!("value-{index:02}")))
        .collect();
    let created = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/rulegroupsnamespaces"),
            REGION,
            &json!({
                "name": "bounded",
                "data": aws_smithy_types::base64::encode(b"groups: []\n".as_slice()),
                "tags": Value::Object(
                    seeded
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                        .collect(),
                ),
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "seeding the namespace's tags must answer 202, got {created:?}"
    );
    let arn = text(&created.body["arn"]);
    wait_for_due_transition();

    let refused = amp
        .tag(REGION, &arn, &json!({ "one-too-many": "c" }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"one past the bound\": the merge must be refused, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["fieldList"][0]["name"]),
        "tags",
        "case \"one past the bound\": the refusal must name the member, got {refused:?}"
    );
    let unchanged = amp.list_tags(REGION, &arn).await;
    assert_eq!(
        unchanged.body["tags"]
            .as_object()
            .map_or(0, serde_json::Map::len),
        TAGS_PER_RESOURCE_LIMIT,
        "case \"one past the bound\": a refused merge must store nothing, got {unchanged:?}"
    );

    let replaced = amp
        .tag(REGION, &arn, &json!({ "seed-00": "replaced" }))
        .await;
    assert_eq!(
        replaced.status, 200,
        "case \"replace at the bound\": a merge that adds no key must answer 200, \
         got {replaced:?}"
    );
    let listed = amp.list_tags(REGION, &arn).await;
    assert_eq!(
        text(&listed.body["tags"]["seed-00"]),
        "replaced",
        "case \"replace at the bound\": the new value must be stored, got {listed:?}"
    );
}
