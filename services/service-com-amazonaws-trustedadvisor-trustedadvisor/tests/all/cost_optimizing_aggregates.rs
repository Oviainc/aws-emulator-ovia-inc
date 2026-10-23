// SPDX-License-Identifier: CC0-1.0

//! `pillarSpecificAggregates` on a recommendation read, over HTTP.
//!
//! The rule this suite pins is about every recommendation-reporting read, not
//! about one call: **a recommendation that optimizes the cost pillar reports
//! `pillarSpecificAggregates.costOptimizing`, whose two `@required` members are
//! totals over that recommendation's own resources — `estimatedMonthlySavings`
//! is the sum of the per-resource saving the check reports in its "Estimated
//! Monthly Savings" metadata column, and `estimatedPercentMonthlySavings` is
//! that sum as a percentage of those resources' monthly spend (zero when
//! nothing is being spent on them) — and a recommendation that optimizes no
//! cost reports the member not at all.**
//!
//! So the tests are TABLES rather than one recorded call. The rows span:
//!
//! * every seeded recommendation — cost-only pillars, cost beside another
//!   pillar, and two non-cost pillar sets — at one, two and three resources,
//!   with zero-saving resources among non-zero ones;
//! * both reads that carry the member (`GetRecommendation` and
//!   `ListRecommendations`, filtered and unfiltered), which must agree;
//! * the store changing under the read: a saving column removed, the spend
//!   taken away (the zero-spend branch), and the cost pillar added to a
//!   recommendation whose check reports no savings column at all.
//!
//! The expected figures are written out by hand below; their source is the
//! `SEED` in `src/state.rs` (the saving is each resource's "Estimated Monthly
//! Savings" metadata column, the spend its `estimated_monthly_spend`), so a
//! response is compared against stated values and not against the store it came
//! from.
//!
//! Member names, their `@required`-ness and the one pillar the structure models
//! come from the model:
//! `smithy_query(query="shape",shape="RecommendationPillarSpecificAggregates,RecommendationCostOptimizingAggregates",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`.

use serde_json::Value;
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::{service_state, SupportPlan};
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{call, emulator_on_plan, label, recommendation_arn, resource_arn};

/// Doubles are compared with a tolerance: the figures travel as JSON numbers
/// and the percentage is a quotient, so neither is expected bit-for-bit.
const EPSILON: f64 = 1e-9;

/// The check metadata heading a Trusted Advisor cost check reports a resource's
/// estimated monthly saving in.
const SAVINGS_HEADING: &str = "Estimated Monthly Savings";

/// The pillar whose aggregate the member carries.
const COST_PILLAR: &str = "cost_optimizing";

/// What each seeded recommendation must report for `pillarSpecificAggregates`:
/// `None` for one that optimizes no cost, otherwise the expected
/// `(estimatedMonthlySavings, estimatedPercentMonthlySavings)`.
///
/// Source: the `SEED` in `src/state.rs`. The savings are the resources'
/// "Estimated Monthly Savings" columns (EC2: 72.00 + 0.00 + 148.00; RDS:
/// 131.40 + 0.00) and the percentages are those sums over the same resources'
/// monthly spend (EC2: 144.00 + 36.00 + 260.00; RDS: 146.00 + 72.00).
const EXPECTED: &[(&str, Option<(f64, f64)>)] = &[
    // Pillars `["fault_tolerance"]`, two resources, one of them excluded.
    ("amazon-ebs-snapshots", None),
    // Pillars `["security"]`, a single resource.
    ("iam-access-key-rotation", None),
    // Pillars `["cost_optimizing"]`, three resources, one saving nothing.
    (
        "low-utilization-ec2-instances",
        Some((220.00, 220.00 / 440.00 * 100.0)),
    ),
    // Pillars `["cost_optimizing", "performance"]`: the cost pillar beside
    // another one still reports the aggregate.
    (
        "rds-idle-db-instances",
        Some((131.40, 131.40 / 218.00 * 100.0)),
    ),
];

/// A mounted emulator whose account is entitled to the API, plus the engine it
/// was built on so a test can move the store under a read.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    emulator_on_plan(SupportPlan::Business)
}

/// `GetRecommendation` for one seeded check's recommendation.
async fn get_recommendation(router: &MultiServiceRouter, check_id: &str) -> Value {
    let arn = recommendation_arn(check_id);
    let (status, body) = call(
        router,
        "GET",
        &format!("/v1/recommendations/{}", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {check_id}: expected 200 from GetRecommendation, got {status} with {body}"
    );
    body["recommendation"].clone()
}

/// Assert the `pillarSpecificAggregates` value of one recommendation body
/// against the expectation for its pillars.
fn assert_cost_aggregate(case: &str, recommendation: &Value, expected: Option<(f64, f64)>) {
    let aggregates = &recommendation["pillarSpecificAggregates"];
    let Some((savings, percent)) = expected else {
        assert!(
            aggregates.is_null(),
            "case {case}: expected no pillarSpecificAggregates for a recommendation \
             without the cost pillar, got {aggregates}"
        );
        return;
    };
    let cost = &aggregates["costOptimizing"];
    for (member, expected) in [
        ("estimatedMonthlySavings", savings),
        ("estimatedPercentMonthlySavings", percent),
    ] {
        let actual = cost[member].as_f64();
        assert!(
            actual.is_some_and(|actual| (actual - expected).abs() < EPSILON),
            "case {case}: expected costOptimizing.{member} {expected}, got {:?}",
            cost[member]
        );
    }
}

#[tokio::test]
async fn the_cost_aggregate_is_reported_exactly_for_the_cost_pillar_with_its_resource_totals() {
    let (router, _engine) = emulator();

    for (check_id, expected) in EXPECTED {
        // The single read.
        let recommendation = get_recommendation(&router, check_id).await;
        assert_cost_aggregate(&format!("GetRecommendation {check_id}"), &recommendation, *expected);

        // And the same recommendation as a listing row: a cost-reporting client
        // totalling savings over a list must see the same figures.
        let (status, listed) = call(&router, "GET", "/v1/recommendations", None).await;
        assert_eq!(
            status, 200,
            "case {check_id}: expected 200 from ListRecommendations, got {status} with {listed}"
        );
        let arn = recommendation_arn(check_id);
        let summary = listed["recommendationSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {check_id}: no recommendationSummaries in {listed}"))
            .iter()
            .find(|summary| summary["arn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {check_id}: {arn} is missing from {listed}"));
        assert_cost_aggregate(&format!("ListRecommendations {check_id}"), summary, *expected);
    }
}

#[tokio::test]
async fn the_reported_savings_sum_the_resources_savings_column() {
    let (router, _engine) = emulator();

    // Every column heading, per check id, as `ListChecks` reports it: the
    // savings column is found by its heading, which is what names the figure.
    let (status, checks) = call(&router, "GET", "/v1/checks", None).await;
    assert_eq!(status, 200, "expected 200 from ListChecks, got {status} with {checks}");
    let savings_columns: Vec<(String, Option<String>)> = checks["checkSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no checkSummaries in {checks}"))
        .iter()
        .map(|check| {
            let id = check["id"].as_str().expect("a check id").to_string();
            let column = check["metadata"]
                .as_object()
                .unwrap_or_else(|| panic!("case {id}: no metadata headings in {check}"))
                .iter()
                .find(|(_, heading)| heading.as_str() == Some(SAVINGS_HEADING))
                .map(|(key, _)| key.clone());
            (id, column)
        })
        .collect();

    // Both list reads: unfiltered, and filtered to the cost pillar (the read a
    // cost-reporting client makes).
    for query in ["", "?pillar=cost_optimizing"] {
        let (status, listed) = call(&router, "GET", &format!("/v1/recommendations{query}"), None).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected 200 from ListRecommendations, got {status} with {listed}"
        );

        for summary in listed["recommendationSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {query:?}: no recommendationSummaries in {listed}"))
        {
            let arn = summary["arn"].as_str().expect("a recommendation arn");
            let case = format!("{query:?} {arn}");
            let optimizes_cost = summary["pillars"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: no pillars in {summary}"))
                .iter()
                .any(|pillar| pillar.as_str() == Some(COST_PILLAR));

            let aggregates = &summary["pillarSpecificAggregates"];
            if !optimizes_cost {
                assert!(
                    aggregates.is_null(),
                    "case {case}: expected no pillarSpecificAggregates without the cost \
                     pillar, got {aggregates}"
                );
                continue;
            }

            // The savings this recommendation's own resources report, read back
            // over the wire: the aggregate and the resource list are the same
            // facts totalled twice.
            let (status, resources) = call(
                &router,
                "GET",
                &format!("/v1/recommendations/{}/resources", label(arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from ListRecommendationResources, got {status} \
                 with {resources}"
            );
            let check_id = summary["checkArn"]
                .as_str()
                .and_then(|check_arn| check_arn.split(":check/").nth(1))
                .expect("a check arn");
            let column = savings_columns
                .iter()
                .find(|(id, _)| id == check_id)
                .map(|(_, column)| column.clone())
                .unwrap_or_else(|| panic!("case {case}: {check_id} is not in the check catalog"));
            let expected: f64 = resources["recommendationResourceSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: no resource summaries in {resources}"))
                .iter()
                .map(|resource| {
                    column
                        .as_deref()
                        .and_then(|column| resource["metadata"][column].as_str())
                        .and_then(|value| value.parse::<f64>().ok())
                        .unwrap_or(0.0)
                })
                .sum();

            let cost = &aggregates["costOptimizing"];
            let savings = cost["estimatedMonthlySavings"].as_f64();
            assert!(
                savings.is_some_and(|savings| (savings - expected).abs() < EPSILON),
                "case {case}: expected estimatedMonthlySavings {expected} (the sum of the \
                 resources' {SAVINGS_HEADING} columns), got {:?}",
                cost["estimatedMonthlySavings"]
            );
            // A saving is part of what is being spent, so its share of that
            // spend is a percentage.
            let percent = cost["estimatedPercentMonthlySavings"].as_f64();
            assert!(
                percent.is_some_and(|percent| (0.0..=100.0).contains(&percent)),
                "case {case}: expected estimatedPercentMonthlySavings within 0..=100, got {:?}",
                cost["estimatedPercentMonthlySavings"]
            );
        }
    }
}

#[tokio::test]
async fn the_cost_aggregate_follows_the_store_rather_than_being_a_stored_constant() {
    // Each row moves the store the way something outside this API would (no
    // modelled operation edits a check's columns, a resource's metadata or what
    // a resource costs) and states what the next read must then report. The
    // third row is the presence rule on its own: the member is keyed to the
    // pillar, not to the data, so a cost recommendation whose check reports no
    // savings column reports the aggregate at zero rather than omitting it.
    //
    // `low-utilization-ec2-instances` totals 220.00 of savings over 440.00 of
    // spend when untouched (see `EXPECTED`).
    type Mutate = fn(&engine::InMemoryEngine);
    let cases: &[(&str, &str, Mutate, (f64, f64))] = &[
        (
            "a resource's savings column removed",
            "low-utilization-ec2-instances",
            |engine| {
                let state = service_state(engine).expect("this service's state");
                let mut state = state.lock();
                let arn = resource_arn("low-utilization-ec2-instances", "res-ec2-0003");
                let resource = state
                    .recommendation_resources
                    .get_mut(&arn)
                    .expect("the seeded resource");
                // Column 3 of this check's headings is the savings column.
                resource.metadata.remove("3");
            },
            // 148.00 of the savings is gone; the spend is untouched.
            (72.00, 72.00 / 440.00 * 100.0),
        ),
        (
            "no spend to take a percentage of",
            "low-utilization-ec2-instances",
            |engine| {
                let state = service_state(engine).expect("this service's state");
                let mut state = state.lock();
                for resource in state.recommendation_resources.values_mut() {
                    resource.estimated_monthly_spend = None;
                }
            },
            (220.00, 0.0),
        ),
        (
            "the cost pillar on a check with no savings column",
            "amazon-ebs-snapshots",
            |engine| {
                let state = service_state(engine).expect("this service's state");
                let mut state = state.lock();
                let arn = recommendation_arn("amazon-ebs-snapshots");
                state
                    .recommendations
                    .get_mut(&arn)
                    .expect("the seeded recommendation")
                    .pillars
                    .push(COST_PILLAR.to_string());
            },
            (0.0, 0.0),
        ),
    ];

    for (case, check_id, mutate, expected) in cases {
        // A fresh emulator per row, so each row starts from the seeded store.
        let (router, engine) = emulator();
        mutate(&engine);
        let recommendation = get_recommendation(&router, check_id).await;
        assert_cost_aggregate(case, &recommendation, Some(*expected));
    }
}
