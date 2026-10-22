// SPDX-License-Identifier: CC0-1.0

//! The members a workload carries because the SERVICE computes them, not because a
//! request supplied them: the risk roll-up, the pillar priority order, the tag map, the
//! discovery configuration and the review-owner acknowledgement.
//!
//! Every assertion here is about the rule, not about one recorded workload: a workload
//! is projected with every member the service derives for it, and the derivation reads
//! the workload's own stored state (its attached lenses, its tags, its discovery
//! configuration), so each table varies that state.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The `Risk` buckets, from `smithy_query(query="shape", shape="Risk")`.
const RISKS: [&str; 5] = ["UNANSWERED", "HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE"];

/// How many questions the AWS Well-Architected Framework lens asks: the framework's
/// published question numbering — OPS 1-11, SEC 11, REL 1-13, PERF 1-5, COST 1-11,
/// SUS 1-6. Spelled out here so the test pins the number rather than reading it back
/// out of the code under test.
const FRAMEWORK_LENS_QUESTIONS: i64 = 11 + 11 + 13 + 5 + 11 + 6;

/// The pillar order a workload is reviewed in when it names none, by `PillarId`
/// (`smithy_query(query="shape", shape="PillarId")`).
fn framework_pillar_order() -> Value {
    json!([
        "operationalExcellence",
        "security",
        "reliability",
        "performance",
        "costOptimization",
        "sustainability"
    ])
}

/// The roll-up a workload with `unanswered` outstanding questions reports: every `Risk`
/// bucket present, the answered ones at 0 because nothing can answer a question yet.
fn expected_risk_counts(unanswered: i64) -> Value {
    let mut counts = serde_json::Map::new();
    for risk in RISKS {
        let count = if risk == "UNANSWERED" { unanswered } else { 0 };
        counts.insert(risk.to_string(), json!(count));
    }
    Value::Object(counts)
}

fn workload_of(answer: &Value) -> Value {
    answer.get("Workload").cloned().unwrap_or(Value::Null)
}

/// `RiskCounts` is the roll-up over the questions of the workload's attached lenses, in
/// EVERY projection of the workload — the read, the update response and the list
/// summary. Rows differ along the lens set: how many lenses, how they are named (alias
/// or ARN), whether the same lens is named twice, and whether the emulator ships the
/// lens's question set at all.
#[tokio::test]
async fn risk_roll_up_counts_the_questions_of_the_attached_lenses() {
    let cases: Vec<(&str, Value, i64)> = vec![
        ("no-lenses", json!([]), 0),
        (
            "framework-lens-by-alias",
            json!(["wellarchitected"]),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        // "For Amazon Web Services official lenses, this is either the lens alias ...
        // or the lens ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless"
        // (`LensAlias` doc), so both names resolve to the same question set.
        (
            "framework-lens-by-arn",
            json!(["arn:aws:wellarchitected:us-east-1::lens/wellarchitected"]),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        (
            "framework-lens-named-twice",
            json!([
                "wellarchitected",
                "arn:aws:wellarchitected:us-east-1::lens/wellarchitected"
            ]),
            FRAMEWORK_LENS_QUESTIONS,
        ),
        // A lens whose definition this emulator does not ship asks no questions here,
        // so it adds nothing to the roll-up.
        ("unshipped-lens-only", json!(["serverless"]), 0),
        (
            "framework-lens-and-unshipped-lenses",
            json!(["wellarchitected", "serverless", "softwareasaservice"]),
            FRAMEWORK_LENS_QUESTIONS,
        ),
    ];

    for (case, lenses, unanswered) in cases {
        let client = Client::new();
        let mut request = minimal_create(&format!("risk-{case}"), &format!("tok-risk-{case}"));
        request["Lenses"] = lenses.clone();
        let workload_id = created_id(&client.create(request).await, case);
        let expected = expected_risk_counts(unanswered);

        let fetched = workload_of(&client.get(&workload_id).await.body);
        assert_eq!(
            fetched.get("RiskCounts"),
            Some(&expected),
            "case {case:?}: GetWorkload must report {expected:?} for lenses {lenses:?}, got {fetched:?}"
        );

        let updated = workload_of(
            &client
                .update(&workload_id, json!({ "Notes": "touched" }))
                .await
                .body,
        );
        assert_eq!(
            updated.get("RiskCounts"),
            Some(&expected),
            "case {case:?}: UpdateWorkload must report {expected:?}, got {updated:?}"
        );

        let listed = client.list(json!({})).await;
        let summary = listed
            .body
            .get("WorkloadSummaries")
            .and_then(Value::as_array)
            .and_then(|summaries| summaries.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary.get("RiskCounts"),
            Some(&expected),
            "case {case:?}: the list summary must report {expected:?}, got {summary:?}"
        );
    }
}

/// The catalog the roll-up and the default pillar order are derived from: the AWS
/// Well-Architected Framework lens's pillars. Checked here on entries the issue did not
/// cite — the sustainability pillar's own count, and a lens alias the emulator ships no
/// definition for.
#[tokio::test]
async fn the_framework_lens_catalog_matches_its_published_question_numbering() {
    let total: usize = state::FRAMEWORK_LENS_PILLARS
        .iter()
        .map(|(_, _, count)| *count)
        .sum();
    assert_eq!(
        i64::try_from(total).unwrap_or(-1),
        FRAMEWORK_LENS_QUESTIONS,
        "the framework lens asks {FRAMEWORK_LENS_QUESTIONS} questions, catalog says {total}"
    );
    assert_eq!(
        json!(state::default_pillar_priorities()),
        framework_pillar_order(),
        "the default pillar order must be the catalog's pillar order, got {:?}",
        state::default_pillar_priorities()
    );
    assert_eq!(
        state::FRAMEWORK_LENS_PILLARS
            .iter()
            .find(|(pillar, _, _)| *pillar == "sustainability")
            .map(|(_, _, count)| *count),
        Some(6),
        "the sustainability pillar asks SUS 1-6, catalog says {:?}",
        state::FRAMEWORK_LENS_PILLARS
    );

    for (case, lens, expected) in [
        ("alias", "wellarchitected", total),
        (
            "arn",
            "arn:aws:wellarchitected:us-east-1::lens/wellarchitected",
            total,
        ),
        (
            "versioned-arn",
            "arn:aws:wellarchitected:us-east-1::lens/wellarchitected/2023-10-03",
            total,
        ),
        ("unshipped-alias", "serverless", 0),
        (
            "unshipped-custom-arn",
            "arn:aws:wellarchitected:us-west-2:000000000000:lens/0123456789abcdef0123456789abcdef",
            0,
        ),
    ] {
        let actual = state::lens_question_count(lens);
        assert_eq!(
            actual, expected,
            "case {case:?}: lens {lens:?} asks {expected} questions here, got {actual}"
        );
    }
}

/// A workload is always reviewed in some pillar order: one created without
/// `PillarPriorities` gets the framework's own pillar order, and one that names its
/// priorities keeps exactly what it named. Rows differ along how many priorities the
/// request names and in what order.
#[tokio::test]
async fn pillar_priorities_default_to_the_framework_pillar_order() {
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        ("omitted", None, framework_pillar_order()),
        // An explicitly empty list names no priority order either.
        ("empty", Some(json!([])), framework_pillar_order()),
        (
            "single-pillar",
            Some(json!(["sustainability"])),
            json!(["sustainability"]),
        ),
        (
            "reordered-subset",
            Some(json!(["costOptimization", "security"])),
            json!(["costOptimization", "security"]),
        ),
        (
            "every-pillar-reversed",
            Some(json!([
                "sustainability",
                "costOptimization",
                "performance",
                "reliability",
                "security",
                "operationalExcellence"
            ])),
            json!([
                "sustainability",
                "costOptimization",
                "performance",
                "reliability",
                "security",
                "operationalExcellence"
            ]),
        ),
    ];

    for (case, priorities, expected) in cases {
        let client = Client::new();
        let mut request = minimal_create(&format!("pillars-{case}"), &format!("tok-pillar-{case}"));
        if let Some(priorities) = priorities.clone() {
            request["PillarPriorities"] = priorities;
        }
        let workload_id = created_id(&client.create(request).await, case);

        for (reader, workload) in [
            ("GetWorkload", workload_of(&client.get(&workload_id).await.body)),
            (
                "UpdateWorkload",
                workload_of(
                    &client
                        .update(&workload_id, json!({ "Notes": "touched" }))
                        .await
                        .body,
                ),
            ),
        ] {
            assert_eq!(
                workload.get("PillarPriorities"),
                Some(&expected),
                "case {case:?}: {reader} must report PillarPriorities {expected:?}, got {workload:?}"
            );
        }
    }
}

/// The tag map is reported as the workload holds it — empty when the workload carries
/// no tags, never absent. Rows differ along how many tags the request supplies.
#[tokio::test]
async fn an_untagged_workload_reports_an_empty_tag_map() {
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        ("untagged", None, json!({})),
        ("one-tag", Some(json!({"env": "test"})), json!({"env": "test"})),
        (
            "three-tags",
            Some(json!({"env": "test", "owner": "guild", "cost-centre": "42"})),
            json!({"env": "test", "owner": "guild", "cost-centre": "42"}),
        ),
        // `TagValue` has no minimum length, so an empty value is a tag like any other.
        ("empty-value", Some(json!({"env": ""})), json!({"env": ""})),
    ];

    for (case, tags, expected) in cases {
        let client = Client::new();
        let mut request = minimal_create(&format!("tags-{case}"), &format!("tok-tags-{case}"));
        if let Some(tags) = tags.clone() {
            request["Tags"] = tags;
        }
        let workload_id = created_id(&client.create(request).await, case);

        for (reader, workload) in [
            ("GetWorkload", workload_of(&client.get(&workload_id).await.body)),
            (
                "UpdateWorkload",
                workload_of(
                    &client
                        .update(&workload_id, json!({ "Notes": "touched" }))
                        .await
                        .body,
                ),
            ),
        ] {
            assert_eq!(
                workload.get("Tags"),
                Some(&expected),
                "case {case:?}: {reader} must report Tags {expected:?}, got {workload:?}"
            );
        }
    }
}

/// A workload always has a discovery configuration, and its Trusted Advisor
/// integration status is `DISABLED` until somebody enables it. Rows differ along which
/// members of the configuration the request supplies. Status values come from
/// `smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")` and the
/// resource-definition values from `shape="DefinitionType"`.
#[tokio::test]
async fn discovery_config_reports_the_default_trusted_advisor_status() {
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "never-configured",
            None,
            json!({"TrustedAdvisorIntegrationStatus": "DISABLED"}),
        ),
        (
            "explicitly-disabled",
            Some(json!({"TrustedAdvisorIntegrationStatus": "DISABLED"})),
            json!({"TrustedAdvisorIntegrationStatus": "DISABLED"}),
        ),
        (
            "enabled",
            Some(json!({"TrustedAdvisorIntegrationStatus": "ENABLED"})),
            json!({"TrustedAdvisorIntegrationStatus": "ENABLED"}),
        ),
        // Only the resource definition supplied: the status is still defaulted, and the
        // definition is kept as given.
        (
            "resource-definition-only",
            Some(json!({"WorkloadResourceDefinition": ["WORKLOAD_METADATA"]})),
            json!({
                "TrustedAdvisorIntegrationStatus": "DISABLED",
                "WorkloadResourceDefinition": ["WORKLOAD_METADATA"],
            }),
        ),
        (
            "enabled-with-both-definitions",
            Some(json!({
                "TrustedAdvisorIntegrationStatus": "ENABLED",
                "WorkloadResourceDefinition": ["WORKLOAD_METADATA", "APP_REGISTRY"],
            })),
            json!({
                "TrustedAdvisorIntegrationStatus": "ENABLED",
                "WorkloadResourceDefinition": ["WORKLOAD_METADATA", "APP_REGISTRY"],
            }),
        ),
    ];

    for (case, config, expected) in cases {
        let client = Client::new();
        let mut request =
            minimal_create(&format!("discovery-{case}"), &format!("tok-discovery-{case}"));
        if let Some(config) = config.clone() {
            request["DiscoveryConfig"] = config;
        }
        let workload_id = created_id(&client.create(request).await, case);

        for (reader, workload) in [
            ("GetWorkload", workload_of(&client.get(&workload_id).await.body)),
            (
                "UpdateWorkload",
                workload_of(
                    &client
                        .update(&workload_id, json!({ "Notes": "touched" }))
                        .await
                        .body,
                ),
            ),
            (
                "GetWorkload after the update",
                workload_of(&client.get(&workload_id).await.body),
            ),
        ] {
            assert_eq!(
                workload.get("DiscoveryConfig"),
                Some(&expected),
                "case {case:?}: {reader} must report DiscoveryConfig {expected:?}, got {workload:?}"
            );
        }
    }
}

/// `IsReviewOwnerUpdateAcknowledged` is true for a workload created with a review owner
/// — `CreateWorkload` documents `ReviewOwner` as required, so there is nothing left for
/// its owner to acknowledge — and it is STATE, so an update that sets it moves it and a
/// later read shows what was set. Rows differ along what the update sets.
#[tokio::test]
async fn a_workload_with_a_review_owner_is_acknowledged() {
    for (case, patch, expected) in [
        ("untouched", json!({ "Notes": "touched" }), true),
        (
            "acknowledged-again",
            json!({ "IsReviewOwnerUpdateAcknowledged": true }),
            true,
        ),
        (
            "withdrawn",
            json!({ "IsReviewOwnerUpdateAcknowledged": false }),
            false,
        ),
    ] {
        let client = Client::new();
        let workload_id = created_id(
            &client
                .create(minimal_create(
                    &format!("ack-{case}"),
                    &format!("tok-ack-{case}"),
                ))
                .await,
            case,
        );

        let created = workload_of(&client.get(&workload_id).await.body);
        assert_eq!(
            created.get("IsReviewOwnerUpdateAcknowledged"),
            Some(&Value::Bool(true)),
            "case {case:?}: a workload created with a review owner is acknowledged, got {created:?}"
        );

        let updated = workload_of(&client.update(&workload_id, patch.clone()).await.body);
        assert_eq!(
            updated.get("IsReviewOwnerUpdateAcknowledged"),
            Some(&Value::Bool(expected)),
            "case {case:?}: after {patch:?} the flag must be {expected}, got {updated:?}"
        );

        let refetched = workload_of(&client.get(&workload_id).await.body);
        assert_eq!(
            refetched.get("IsReviewOwnerUpdateAcknowledged"),
            Some(&Value::Bool(expected)),
            "case {case:?}: the flag must be persisted, got {refetched:?}"
        );
    }
}

/// The roll-up follows the workload's own state rather than being minted once: two
/// workloads on the SAME emulator with different lens sets report different
/// `UNANSWERED` counts, in the list view as well as in the reads.
#[tokio::test]
async fn each_workload_reports_its_own_roll_up() {
    let client = Client::new();
    let mut with_framework = minimal_create("roll-up-framework", "tok-roll-framework");
    with_framework["Lenses"] = json!(["wellarchitected"]);
    let with_framework_id = created_id(&client.create(with_framework).await, "with-framework");

    let mut without = minimal_create("roll-up-none", "tok-roll-none");
    without["Lenses"] = json!([]);
    let without_id = created_id(&client.create(without).await, "without");

    let listed = client.list(json!({})).await;
    let summaries = listed
        .body
        .get("WorkloadSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        2,
        "both workloads must be listed, got {listed:?}"
    );

    for (case, workload_id, unanswered) in [
        ("with-framework-lens", &with_framework_id, FRAMEWORK_LENS_QUESTIONS),
        ("with-no-lens", &without_id, 0),
    ] {
        let expected = expected_risk_counts(unanswered);
        let workload = workload_of(&client.get(workload_id).await.body);
        assert_eq!(
            workload.get("RiskCounts"),
            Some(&expected),
            "case {case:?}: expected {expected:?}, got {workload:?}"
        );
        let summary = summaries
            .iter()
            .find(|summary| summary.get("WorkloadId").and_then(Value::as_str) == Some(workload_id))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary.get("RiskCounts"),
            Some(&expected),
            "case {case:?}: the summary must carry the same roll-up {expected:?}, got {summary:?}"
        );
    }
}
