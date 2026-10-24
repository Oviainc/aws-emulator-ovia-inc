// SPDX-License-Identifier: CC0-1.0

//! `BatchUpdateRecommendationResourceExclusion` over a request that names one
//! recommendation-resource ARN more than once, driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The RULE, from the operation's own doc
//! (`smithy_query(query="operation",operation="BatchUpdateRecommendationResourceExclusion",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — "This API supports up to 25 unique recommendation resource ARNs per
//! request"): an entry list identifies each resource at most once, so a request
//! that repeats an identifier is not well formed and is refused WHOLE with the
//! modelled `ValidationException` (`@error("client")`, `@httpError(400)`),
//! applying none of its entries. The modelled `@uniqueItems` on
//! `RecommendationResourceExclusionList` cannot state this: it compares whole
//! `RecommendationResourceExclusion` structures, so `{arn: X, isExcluded: true}`
//! and `{arn: X, isExcluded: false}` are two distinct elements.
//!
//! Each test below asserts that rule over a TABLE whose rows span the class it
//! covers rather than one recorded call: the repeat's position in the list, how
//! many entries the list holds (up to the modelled `@length(1, 25)` maximum),
//! how many times one ARN occurs, how many distinct ARNs repeat, whether the
//! repeated entries agree or conflict on `isExcluded`, and whether the repeated
//! ARN names a stored resource, no resource at all, or a resource of a
//! prioritized recommendation. The counterpart test pins that a list of DISTINCT
//! ARNs still applies, read back through `ListRecommendationResources` and
//! `GetRecommendation`.
//!
//! Every call is made as an account whose support plan includes the Trusted
//! Advisor API; that gate is pinned in `tests/all/support_level_gate.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    call, dispatch, emulator_on_plan, error_code, label, recommendation_arn, resource_arn,
};

/// The path this operation binds (`PUT /v1/batch-update-recommendation-resource-exclusion`).
const BATCH_PATH: &str = "/v1/batch-update-recommendation-resource-exclusion";

/// The modelled maximum of `RecommendationResourceExclusionList`
/// (`@length(1, 25)`), which is also the count the operation's doc calls "up to
/// 25 unique recommendation resource ARNs per request".
const MAX_ENTRIES: usize = 25;

/// Every seeded recommendation and the resources the store keys under it, with
/// the `type` of the recommendation that owns them: the written-out source is the
/// seed in `src/state.rs`, so a response is compared against stated values and
/// not against the store it came from.
const SEEDED: &[SeededRecommendation] = &[
    SeededRecommendation {
        check_id: "amazon-ebs-snapshots",
        recommendation_type: "standard",
        resources: &[("res-ebs-0001", "included"), ("res-ebs-0002", "excluded")],
    },
    SeededRecommendation {
        check_id: "iam-access-key-rotation",
        recommendation_type: "standard",
        resources: &[("res-iam-0001", "included")],
    },
    SeededRecommendation {
        check_id: "low-utilization-ec2-instances",
        recommendation_type: "priority",
        resources: &[
            ("res-ec2-0001", "included"),
            ("res-ec2-0002", "included"),
            ("res-ec2-0003", "included"),
        ],
    },
    SeededRecommendation {
        check_id: "rds-idle-db-instances",
        recommendation_type: "priority",
        resources: &[("res-rds-0001", "included"), ("res-rds-0002", "included")],
    },
];

/// One seeded recommendation: the check it came from, its `type` (this operation
/// does not support the resources of a `priority` one) and its resources as
/// `(resource id, seeded exclusionStatus)`, in resource-ARN order.
struct SeededRecommendation {
    check_id: &'static str,
    recommendation_type: &'static str,
    resources: &'static [(&'static str, &'static str)],
}

fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// The exclusion status of every seeded resource, as the API reports it: one
/// `(resource id, exclusionStatus)` pair per resource, recommendation by
/// recommendation in the order `SEEDED` lists them.
async fn exclusion_statuses(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let mut statuses: Vec<(String, String)> = Vec::new();
    for recommendation in SEEDED {
        let check_id = recommendation.check_id;
        let path = format!(
            "/v1/recommendations/{}/resources",
            label(&recommendation_arn(check_id))
        );
        let (status, body) = call(router, "GET", &path, None).await;
        assert_eq!(
            status, 200,
            "reading {check_id}'s resources: expected 200, got {status} with {body}"
        );
        for summary in body["recommendationResourceSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("no recommendationResourceSummaries in {body}"))
        {
            statuses.push((
                summary["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a summary with no id: {summary}"))
                    .to_string(),
                summary["exclusionStatus"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a summary with no exclusionStatus: {summary}"))
                    .to_string(),
            ));
        }
    }
    statuses
}

/// The seeded exclusion statuses as `SEEDED` states them, so an "unchanged
/// store" assertion is against written-out values and not only against an
/// earlier read.
fn seeded_statuses() -> Vec<(String, String)> {
    SEEDED
        .iter()
        .flat_map(|recommendation| {
            recommendation
                .resources
                .iter()
                .map(|(id, exclusion)| ((*id).to_string(), (*exclusion).to_string()))
        })
        .collect()
}

/// The `message` the refusal carries, in the sentence this service builds every
/// input-validation refusal from (`src/service/validation.rs`): the clause count,
/// then one `<subject> at '<member path>' failed to satisfy constraint:
/// <requirement>` clause per offending value, joined with `; `.
fn expected_message(repeated: &[&str]) -> String {
    let clauses: Vec<String> = repeated
        .iter()
        .map(|arn| {
            format!(
                "Value '{arn}' at 'recommendationResourceExclusions' failed to satisfy \
                 constraint: Member must have unique arn values"
            )
        })
        .collect();
    match clauses.as_slice() {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// The request body for a list of `(arn, isExcluded)` entries.
fn body(entries: &[(String, bool)]) -> Value {
    json!({
        "recommendationResourceExclusions": entries
            .iter()
            .map(|(arn, is_excluded)| json!({"arn": arn, "isExcluded": is_excluded}))
            .collect::<Vec<Value>>(),
    })
}

/// One row of the refusal table: what the row is about, the entries to send, and
/// the ARNs the refusal must name, in the order in which each first repeats.
struct Row {
    name: &'static str,
    entries: Vec<(String, bool)>,
    repeated: Vec<String>,
}

#[tokio::test]
async fn a_batch_naming_one_resource_arn_twice_is_refused_whole() {
    let router = emulator();

    // Stored, non-prioritized resources (the ones an accepted batch could
    // actually update), a stored resource of a `priority` recommendation, and an
    // ARN naming nothing: the refusal is the request's, so it does not depend on
    // what the ARN resolves to.
    let first = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");
    let second = resource_arn("amazon-ebs-snapshots", "res-ebs-0002");
    let other_recommendation = resource_arn("iam-access-key-rotation", "res-iam-0001");
    let prioritized_recommendation = SEEDED
        .iter()
        .find(|seeded| seeded.recommendation_type == "priority")
        .expect("the seed holds a prioritized recommendation");
    let prioritized = resource_arn(
        prioritized_recommendation.check_id,
        prioritized_recommendation.resources[0].0,
    );
    let unknown = resource_arn("amazon-ebs-snapshots", "res-ebs-9999");

    // A batch at the modelled maximum of 25 entries whose FIRST ARN is repeated
    // as the last entry: 24 distinct ARNs, 25 entries.
    let mut at_max_entries: Vec<(String, bool)> = (0..MAX_ENTRIES - 1)
        .map(|index| {
            (
                resource_arn("amazon-ebs-snapshots", &format!("res-ebs-{index:04}")),
                index % 2 == 0,
            )
        })
        .collect();
    let first_of_max = at_max_entries[0].0.clone();
    at_max_entries.push((first_of_max.clone(), false));

    let rows = vec![
        Row {
            name: "the same ARN twice with conflicting isExcluded",
            entries: vec![(first.clone(), true), (first.clone(), false)],
            repeated: vec![first.clone()],
        },
        Row {
            name: "the same ARN twice with the same isExcluded (one whole entry repeated)",
            entries: vec![(first.clone(), true), (first.clone(), true)],
            repeated: vec![first.clone()],
        },
        Row {
            name: "a repeat separated by another entry",
            entries: vec![
                (first.clone(), true),
                (second.clone(), false),
                (first.clone(), false),
            ],
            repeated: vec![first.clone()],
        },
        Row {
            name: "one ARN three times, reported once",
            entries: vec![
                (first.clone(), true),
                (first.clone(), false),
                (first.clone(), true),
            ],
            repeated: vec![first.clone()],
        },
        Row {
            name: "two ARNs each repeated, in first-repeat order",
            entries: vec![
                (second.clone(), true),
                (first.clone(), true),
                (first.clone(), false),
                (second.clone(), false),
            ],
            repeated: vec![first.clone(), second.clone()],
        },
        Row {
            name: "a repeat beside entries of another recommendation",
            entries: vec![
                (other_recommendation.clone(), true),
                (second.clone(), false),
                (other_recommendation.clone(), false),
            ],
            repeated: vec![other_recommendation.clone()],
        },
        Row {
            name: "a repeated ARN naming no stored resource",
            entries: vec![(unknown.clone(), true), (unknown.clone(), false)],
            repeated: vec![unknown.clone()],
        },
        Row {
            name: "a repeated ARN naming a prioritized recommendation's resource",
            entries: vec![(prioritized.clone(), true), (prioritized.clone(), false)],
            repeated: vec![prioritized.clone()],
        },
        Row {
            name: "a repeat in a batch at the modelled maximum of 25 entries",
            entries: at_max_entries,
            repeated: vec![first_of_max],
        },
    ];

    for Row {
        name,
        entries,
        repeated,
    } in rows
    {
        let (status, errortype, response) =
            dispatch(&router, "PUT", BATCH_PATH, Some(body(&entries))).await;
        assert_eq!(
            status, 400,
            "case {name}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {name}: expected ValidationException, got {response}"
        );
        assert!(
            errortype
                .as_deref()
                .is_none_or(|header| header.starts_with("ValidationException")),
            "case {name}: expected the x-amzn-errortype header to resolve ValidationException, \
             got {errortype:?}"
        );
        let expected: Vec<&str> = repeated.iter().map(String::as_str).collect();
        assert_eq!(
            response["message"].as_str(),
            Some(expected_message(&expected).as_str()),
            "case {name}: expected the refusal to name every repeated ARN once, got {response}"
        );

        // Nothing was applied: not the repeated entries, and not the
        // well-formed entries beside them.
        let after = exclusion_statuses(&router).await;
        assert_eq!(
            after,
            seeded_statuses(),
            "case {name}: expected the stored exclusion statuses to be unchanged, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_of_distinct_resource_arns_still_applies() {
    let excluded = "excluded".to_string();
    let included = "included".to_string();

    // Rows differ along the number of entries, how many recommendations they
    // span, and whether the entries carry the same or opposite `isExcluded`
    // values — none repeats an ARN, so each one applies in full. The expectation
    // is the exclusion status of every seeded resource after the call, in the
    // order `SEEDED` lists them.
    struct Applied {
        name: &'static str,
        entries: Vec<(String, bool)>,
        expected: Vec<String>,
    }

    let first = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");
    let second = resource_arn("amazon-ebs-snapshots", "res-ebs-0002");
    let other_recommendation = resource_arn("iam-access-key-rotation", "res-iam-0001");

    let rows = vec![
        Applied {
            name: "one entry",
            entries: vec![(first.clone(), true)],
            expected: vec![
                excluded.clone(),
                excluded.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
            ],
        },
        Applied {
            name: "two resources of one recommendation, opposite flags",
            entries: vec![(first.clone(), true), (second.clone(), false)],
            expected: vec![
                excluded.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
            ],
        },
        Applied {
            name: "resources across two recommendations, same flag",
            entries: vec![
                (first.clone(), true),
                (other_recommendation.clone(), true),
            ],
            expected: vec![
                excluded.clone(),
                excluded.clone(),
                excluded.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
            ],
        },
        Applied {
            name: "distinct ARNs differing only in the flag they carry",
            entries: vec![
                (first.clone(), false),
                (second.clone(), true),
                (other_recommendation.clone(), false),
            ],
            expected: vec![
                included.clone(),
                excluded.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
                included.clone(),
            ],
        },
    ];

    for Applied {
        name,
        entries,
        expected,
    } in rows
    {
        // Each row runs on its own store, so the rows are independent of each
        // other's writes.
        let router = emulator();
        let (status, response) = call(&router, "PUT", BATCH_PATH, Some(body(&entries))).await;
        assert_eq!(
            status, 200,
            "case {name}: expected 200, got {status} with {response}"
        );
        let errors = response["batchUpdateRecommendationResourceExclusionErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name}: no error list in {response}"));
        assert!(
            errors.is_empty(),
            "case {name}: expected no per-entry error, got {errors:?}"
        );

        // Read the write back through ListRecommendationResources…
        let after: Vec<String> = exclusion_statuses(&router)
            .await
            .into_iter()
            .map(|(_, exclusion)| exclusion)
            .collect();
        assert_eq!(
            after, expected,
            "case {name}: expected the stored exclusion statuses {expected:?}, got {after:?}"
        );

        // …and through GetRecommendation's `resourcesAggregates.excludedCount`,
        // which counts the same resources.
        for seeded in SEEDED {
            let check_id = seeded.check_id;
            let resources = seeded.resources;
            let (get_status, recommendation) = call(
                &router,
                "GET",
                &format!("/v1/recommendations/{}", label(&recommendation_arn(check_id))),
                None,
            )
            .await;
            assert_eq!(
                get_status, 200,
                "case {name}: expected 200 reading {check_id}, got {get_status} with {recommendation}"
            );
            let expected_excluded = resources
                .iter()
                .filter(|(id, seeded_exclusion)| {
                    let arn = resource_arn(check_id, id);
                    // A resource this call named holds what the call asked for;
                    // one it did not holds whatever the seed stored.
                    entries
                        .iter()
                        .find(|(entry_arn, _)| *entry_arn == arn)
                        .map_or(*seeded_exclusion == "excluded", |(_, is_excluded)| {
                            *is_excluded
                        })
                })
                .count();
            assert_eq!(
                recommendation["recommendation"]["resourcesAggregates"]["excludedCount"].as_u64(),
                Some(expected_excluded as u64),
                "case {name}: expected {check_id} to count {expected_excluded} excluded \
                 resources, got {}",
                recommendation["recommendation"]["resourcesAggregates"]["excludedCount"]
            );
        }
    }
}

#[tokio::test]
async fn a_batch_of_25_distinct_arns_is_accepted_and_reported_per_entry() {
    let router = emulator();

    // The uniqueness rule is about REPEATS, not about how many entries a batch
    // holds or whether they resolve: a full 25-entry batch of distinct ARNs is
    // accepted, and the ones naming no stored resource come back as per-entry
    // errors rather than as a top-level refusal.
    let applicable = resource_arn("amazon-ebs-snapshots", "res-ebs-0001");
    let mut entries: Vec<(String, bool)> = vec![(applicable.clone(), true)];
    entries.extend((1..MAX_ENTRIES).map(|index| {
        (
            resource_arn("amazon-ebs-snapshots", &format!("res-gen-{index:04}")),
            true,
        )
    }));
    assert_eq!(
        entries.len(),
        MAX_ENTRIES,
        "the table row is built at the modelled maximum, got {} entries",
        entries.len()
    );

    let (status, response) = call(&router, "PUT", BATCH_PATH, Some(body(&entries))).await;
    assert_eq!(
        status, 200,
        "expected 200 for 25 distinct ARNs, got {status} with {response}"
    );
    let errors = response["batchUpdateRecommendationResourceExclusionErrors"]
        .as_array()
        .unwrap_or_else(|| panic!("no error list in {response}"));
    assert_eq!(
        errors.len(),
        MAX_ENTRIES - 1,
        "expected one per-entry error per ARN naming no stored resource, got {errors:?}"
    );

    let statuses = exclusion_statuses(&router).await;
    assert_eq!(
        statuses
            .iter()
            .find(|(id, _)| id == "res-ebs-0001")
            .map(|(_, exclusion)| exclusion.as_str()),
        Some("excluded"),
        "expected the one applicable entry to have been applied, got {statuses:?}"
    );
}
