// SPDX-License-Identifier: CC0-1.0

//! One application source is claimed by at most one scaling plan, on EVERY path that
//! sets a plan's `ApplicationSource`.
//!
//! `CreateScalingPlanRequest$ApplicationSource` and
//! `UpdateScalingPlanRequest$ApplicationSource`
//! (`smithy_query(query="doc",shape="UpdateScalingPlanRequest",member="ApplicationSource")`)
//! both document "You can create one scaling plan per application source", so a write
//! that would leave two stored plans claiming one application source is refused with a
//! 400 `ValidationException` naming the plan that already covers it, and the stored plan
//! is left exactly as it was.
//!
//! The rule is asserted over a TABLE whose rows differ along what identifies an
//! application source — a CloudFormation stack ARN, one tag filter, several tag filters —
//! and, for the tag-filter rows, along the SPELLING of one identity (filters and their
//! values reordered), which names the same set of tags and therefore the same source.
//! Each row drives both write paths and both of the exclusivity rule's sides: a source
//! another plan claims is refused, a source nobody else claims (including the plan's own)
//! is accepted.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan that claims the row's application source first.
const OWNER: &str = "plan-a";
/// The plan that tries to take it over, and that must be left untouched when refused.
const CHALLENGER: &str = "plan-b";

/// One row of the table: an application source, a second spelling of that SAME source,
/// and a source nothing claims.
struct SourceCase {
    label: &'static str,
    /// What `plan-a` claims.
    claimed: Value,
    /// A spelling of `claimed` that names the same application: same stack ARN, or the
    /// same set of tag filters written in another order.
    same_source_respelled: Value,
    /// A source no stored plan claims.
    unclaimed: Value,
}

/// The rows the rule is asserted over.
fn source_cases() -> Vec<SourceCase> {
    vec![
        SourceCase {
            label: "a CloudFormation stack ARN",
            claimed: stack_source(1),
            same_source_respelled: stack_source(1),
            unclaimed: stack_source(9),
        },
        SourceCase {
            label: "one tag filter, one value",
            claimed: json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            same_source_respelled: json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            unclaimed: json!({ "TagFilters": [{ "Key": "env", "Values": ["dev"] }] }),
        },
        SourceCase {
            label: "one tag filter, its values reordered",
            claimed: json!({ "TagFilters": [{ "Key": "tier", "Values": ["web", "api"] }] }),
            same_source_respelled: json!({
                "TagFilters": [{ "Key": "tier", "Values": ["api", "web"] }]
            }),
            unclaimed: json!({ "TagFilters": [{ "Key": "tier", "Values": ["worker"] }] }),
        },
        SourceCase {
            label: "three tag filters, filters and values reordered",
            claimed: json!({
                "TagFilters": [
                    { "Key": "env", "Values": ["prod", "staging"] },
                    { "Key": "team", "Values": ["platform"] },
                    { "Key": "tier", "Values": ["web", "api", "worker"] }
                ]
            }),
            same_source_respelled: json!({
                "TagFilters": [
                    { "Key": "tier", "Values": ["worker", "api", "web"] },
                    { "Key": "env", "Values": ["staging", "prod"] },
                    { "Key": "team", "Values": ["platform"] }
                ]
            }),
            unclaimed: json!({
                "TagFilters": [
                    { "Key": "env", "Values": ["prod"] },
                    { "Key": "team", "Values": ["platform"] }
                ]
            }),
        },
    ]
}

/// The refusal wording the rule requires, for the plan that already covers the source.
fn expected_message(covered_by: &str) -> String {
    format!("The application source is already covered by the scaling plan '{covered_by}'")
}

/// Create one plan over `source` and settle it to `Active`.
async fn create_active_plan(emulator: &Emulator, name: &str, source: &Value) {
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": name,
                "ApplicationSource": source,
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
}

/// The one stored plan under `name`, as `DescribeScalingPlans` projects it.
async fn describe_one(emulator: &Emulator, name: &str) -> Value {
    let described = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [name] }),
        )
        .await;
    described["ScalingPlans"][0].clone()
}

/// The names of the plans `DescribeScalingPlans` reports for one application source.
async fn names_covering(emulator: &Emulator, source: &Value) -> Vec<String> {
    let described = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ApplicationSources": [source] }),
        )
        .await;
    described["ScalingPlans"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|plan| {
            plan["ScalingPlanName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn an_update_cannot_claim_an_application_source_another_plan_covers() {
    for case in source_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        // `plan-a` claims the row's source; `plan-b` claims an unrelated one.
        let challenger_source = stack_source(42);
        create_active_plan(&emulator, OWNER, &case.claimed).await;
        create_active_plan(&emulator, CHALLENGER, &challenger_source).await;

        // The refusal: an update onto a source another stored plan already covers.
        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": CHALLENGER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": case.same_source_respelled,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label}: expected a 400 ValidationException for a source \
             {OWNER} already covers, got {status} with {body:?}"
        );
        assert_eq!(
            error_message(&body),
            expected_message(OWNER),
            "case {label}: wrong refusal message, got {body:?}"
        );

        // The stored plan is untouched: same source, and still Active rather than
        // UpdateInProgress.
        let challenger = describe_one(&emulator, CHALLENGER).await;
        assert_eq!(
            challenger.get("ApplicationSource"),
            Some(&challenger_source),
            "case {label}: the refused update must leave the stored source alone, \
             got {challenger:?}"
        );
        assert_eq!(
            challenger.get("StatusCode"),
            Some(&json!("Active")),
            "case {label}: a refused update must not move the plan's status, \
             got {challenger:?}"
        );

        // And exactly one plan covers the source, whichever spelling it is read by.
        for (spelling, source) in [
            ("as claimed", &case.claimed),
            ("respelled", &case.same_source_respelled),
        ] {
            let covering = names_covering(&emulator, source).await;
            assert_eq!(
                covering,
                vec![OWNER.to_string()],
                "case {label}/{spelling}: exactly one plan may cover an application \
                 source, got {covering:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_accepts_every_source_no_other_plan_covers() {
    for case in source_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &case.claimed).await;

        // A plan may re-send the source it already claims: it does not conflict with
        // itself, however that source is spelled.
        for (spelling, source) in [
            ("as claimed", &case.claimed),
            ("respelled", &case.same_source_respelled),
        ] {
            emulator
                .ok(
                    "UpdateScalingPlan",
                    json!({
                        "ScalingPlanName": OWNER,
                        "ScalingPlanVersion": 1,
                        "ApplicationSource": source,
                    }),
                )
                .await;
            emulator.make_all_due();
            let owner = describe_one(&emulator, OWNER).await;
            assert_eq!(
                owner.get("ApplicationSource"),
                Some(source),
                "case {label}/{spelling}: a plan re-claiming its own source must keep it, \
                 got {owner:?}"
            );
        }

        // A source nothing covers is accepted and read back.
        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": OWNER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": case.unclaimed,
                }),
            )
            .await;
        emulator.make_all_due();
        let owner = describe_one(&emulator, OWNER).await;
        assert_eq!(
            owner.get("ApplicationSource"),
            Some(&case.unclaimed),
            "case {label}: an update onto an uncovered source must be stored, got {owner:?}"
        );

        // The source the plan has just left is free again: a second plan may take it.
        create_active_plan(&emulator, CHALLENGER, &case.claimed).await;
        let covering = names_covering(&emulator, &case.claimed).await;
        assert_eq!(
            covering,
            vec![CHALLENGER.to_string()],
            "case {label}: the released source must be claimable, got {covering:?}"
        );
    }
}

#[tokio::test]
async fn a_source_freed_by_a_delete_can_be_claimed_by_an_update() {
    // The check is against the plans actually stored, not a list that grows forever:
    // once the covering plan is gone, its source is claimable by an update.
    for case in source_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &case.claimed).await;
        create_active_plan(&emulator, CHALLENGER, &stack_source(42)).await;

        emulator
            .ok(
                "DeleteScalingPlan",
                json!({ "ScalingPlanName": OWNER, "ScalingPlanVersion": 1 }),
            )
            .await;

        // While the removal is only OWED the plan is still stored and still reported,
        // so it still covers its source and an update onto it is still refused.
        let covering = names_covering(&emulator, &case.claimed).await;
        assert_eq!(
            covering,
            vec![OWNER.to_string()],
            "case {label}: a plan whose deletion has not settled still covers its \
             source, got {covering:?}"
        );
        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": CHALLENGER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": case.claimed,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, "ValidationException", expected_message(OWNER).as_str()),
            "case {label}: a source held by a not-yet-removed plan stays covered, \
             got {status} with {body:?}"
        );

        emulator.make_all_due();
        let covering = names_covering(&emulator, &case.claimed).await;
        assert!(
            covering.is_empty(),
            "case {label}: the deleted plan must stop covering its source, got {covering:?}"
        );

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": CHALLENGER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": case.claimed,
                }),
            )
            .await;
        emulator.make_all_due();
        let covering = names_covering(&emulator, &case.claimed).await;
        assert_eq!(
            covering,
            vec![CHALLENGER.to_string()],
            "case {label}: the freed source must be claimable by an update, got {covering:?}"
        );
    }
}

#[tokio::test]
async fn a_create_and_an_update_refuse_a_covered_source_with_the_same_wording() {
    // Both write paths set `ApplicationSource`, so both answer the one rule the same
    // way: the same status, the same shape and the same sentence.
    for case in source_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &case.claimed).await;
        create_active_plan(&emulator, CHALLENGER, &stack_source(42)).await;

        let create = json!({
            "ScalingPlanName": "plan-c",
            "ApplicationSource": case.same_source_respelled,
            "ScalingInstructions": simple_instructions(),
        });
        let update = json!({
            "ScalingPlanName": CHALLENGER,
            "ScalingPlanVersion": 1,
            "ApplicationSource": case.same_source_respelled,
        });
        for (operation, request) in [("CreateScalingPlan", create), ("UpdateScalingPlan", update)] {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body), error_message(&body)),
                (400, "ValidationException", expected_message(OWNER).as_str()),
                "case {label}/{operation}: both write paths must refuse a covered \
                 application source alike, got {status} with {body:?}"
            );
        }
    }
}
