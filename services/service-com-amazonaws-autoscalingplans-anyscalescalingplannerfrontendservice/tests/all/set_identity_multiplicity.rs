// SPDX-License-Identifier: CC0-1.0

//! A request member that names a SET identifies the same thing however many times it
//! writes one of that set's entries.
//!
//! Two rules of this service are decided on the identity of such a member.
//! `ApplicationSource$TagFilters` is "A set of tags (up to 50)"
//! (`smithy_query(query="shape",shape="ApplicationSource")`), each filter carrying "The
//! tag values (0 to 20)" (`smithy_query(query="shape",shape="TagFilter")`), and that
//! identity decides "You can create one scaling plan per application source" on both
//! write paths and selects plans in the `DescribeScalingPlans` `ApplicationSources`
//! filter. `CustomizedScalingMetricSpecification$Dimensions` is the set of dimensions a
//! CloudWatch metric was published with
//! (`smithy_query(query="shape",shape="CustomizedScalingMetricSpecification")`), and
//! that identity decides "Each of these structures must specify a unique scaling metric
//! and a target value for the metric" (`ScalingInstruction$TargetTrackingConfigurations`)
//! and pairs a re-sent configuration with the one in force in the update merge.
//!
//! None of those list shapes carries `@uniqueItems`
//! (`smithy_query(query="shape",shape="TagValues,MetricDimensions")`), so a repeated
//! entry reaches the handler untouched — and since a set is the entries it holds, not
//! how often each is written, the repeated spelling must decide every one of those rules
//! exactly as the single spelling does. The rows below re-spell one set by repeating
//! entries (and by reordering them at the same time), including entries whose text holds
//! a separator character, and each row is driven in BOTH directions so neither spelling
//! is privileged.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`] and the identity-key helpers from
//! [`crate::identity_key_separators`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::identity_key_separators::{
    covered_message, create_active_plan, customized_metric, describe_one, instruction_tracking,
    names_covering,
};
use crate::scaling_plan_lifecycle::{error_code, error_message, simple_instructions, Emulator};

/// The plan that claims a source first.
const OWNER: &str = "plan-a";
/// The plan that tries to claim the same source under another spelling.
const CHALLENGER: &str = "plan-b";

/// One tag-filter set written two ways: entries stated once, and the same entries with
/// some of them repeated.
fn repeated_sources() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "one filter, the filter entry written twice",
            json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            json!({ "TagFilters": [
                { "Key": "env", "Values": ["prod"] },
                { "Key": "env", "Values": ["prod"] }
            ]}),
        ),
        (
            "one filter, a tag value written twice",
            json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            json!({ "TagFilters": [{ "Key": "env", "Values": ["prod", "prod"] }] }),
        ),
        (
            "one filter with two values, each repeated and the values reordered",
            json!({ "TagFilters": [{ "Key": "tier", "Values": ["web", "api"] }] }),
            json!({ "TagFilters": [{ "Key": "tier", "Values": ["api", "web", "api", "web"] }] }),
        ),
        (
            "two filters, one written three times and the filters reordered",
            json!({ "TagFilters": [
                { "Key": "env", "Values": ["prod"] },
                { "Key": "team", "Values": ["x"] }
            ]}),
            json!({ "TagFilters": [
                { "Key": "team", "Values": ["x"] },
                { "Key": "env", "Values": ["prod"] },
                { "Key": "env", "Values": ["prod"] },
                { "Key": "env", "Values": ["prod"] }
            ]}),
        ),
        (
            "a repeated value whose text holds a separator character",
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a;b=c"] }] }),
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a;b=c", "a;b=c"] }] }),
        ),
        (
            "a repeated value whose text holds the escape character",
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a\\;b"] }] }),
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a\\;b", "a\\;b"] }] }),
        ),
    ]
}

/// Every row of [`repeated_sources`] in both directions: the spelling a plan claims
/// first, and the other spelling of that same set.
fn spelling_pairs() -> Vec<(String, Value, Value)> {
    let mut pairs = Vec::new();
    for (label, stated, repeated) in repeated_sources() {
        pairs.push((
            format!("{label} (claimed as stated)"),
            stated.clone(),
            repeated.clone(),
        ));
        pairs.push((format!("{label} (claimed as repeated)"), repeated, stated));
    }
    pairs
}

#[tokio::test]
async fn a_create_refuses_a_source_respelled_with_repeated_entries() {
    for (case, claimed, respelled) in spelling_pairs() {
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &claimed).await;

        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": CHALLENGER,
                    "ApplicationSource": respelled,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, "ValidationException", covered_message(OWNER).as_str()),
            "case {case:?}: a set of tags that merely repeats an entry is the same \
             application source, got {status} with {body:?}"
        );

        // And only the one plan that has the source is reported for it, read by either
        // spelling.
        for (label, source) in [("as claimed", &claimed), ("respelled", &respelled)] {
            let covering = names_covering(&emulator, source).await;
            assert_eq!(
                covering,
                vec![OWNER.to_string()],
                "case {case:?}/{label}: both spellings must select the one plan that \
                 covers the set, got {covering:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_refuses_a_source_respelled_with_repeated_entries() {
    for (case, claimed, respelled) in spelling_pairs() {
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &claimed).await;
        let challenger_source = json!({ "TagFilters": [{ "Key": "other", "Values": ["set"] }] });
        create_active_plan(&emulator, CHALLENGER, &challenger_source).await;

        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": CHALLENGER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": respelled,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body), error_message(&body)),
            (400, "ValidationException", covered_message(OWNER).as_str()),
            "case {case:?}: an update onto a repeated spelling of a covered set must be \
             refused, got {status} with {body:?}"
        );
        let challenger = describe_one(&emulator, CHALLENGER).await;
        assert_eq!(
            challenger.get("ApplicationSource"),
            Some(&challenger_source),
            "case {case:?}: the refused update must leave the stored source alone, \
             got {challenger:?}"
        );
    }
}

#[tokio::test]
async fn a_plan_may_re_send_its_own_source_with_repeated_entries() {
    // The other side of the rule: the plan that already covers the set does not
    // conflict with itself, however the set is spelled, and the source is stored as
    // the request words it.
    for (case, claimed, respelled) in spelling_pairs() {
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &claimed).await;

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": OWNER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": respelled,
                }),
            )
            .await;
        emulator.make_all_due();
        let owner = describe_one(&emulator, OWNER).await;
        assert_eq!(
            owner.get("ApplicationSource"),
            Some(&respelled),
            "case {case:?}: a plan re-claiming its own set must keep it as sent, \
             got {owner:?}"
        );
    }
}

/// The `(Name, Value)` pairs one `CustomizedScalingMetricSpecification$Dimensions` set
/// names.
type DimensionSet = Vec<(&'static str, &'static str)>;

/// Two dimension sets, and whether they name ONE metric or two: the first rows re-spell
/// one set by repeating entries, the last two name genuinely different sets.
fn dimension_pairs() -> Vec<(&'static str, DimensionSet, DimensionSet, bool)> {
    vec![
        (
            "one dimension written twice",
            vec![("a", "1")],
            vec![("a", "1"), ("a", "1")],
            true,
        ),
        (
            "two dimensions, one repeated and the set reordered",
            vec![("a", "1"), ("b", "2")],
            vec![("b", "2"), ("a", "1"), ("b", "2")],
            true,
        ),
        (
            "a repeated dimension whose text holds a separator character",
            vec![("k", "v,x=y")],
            vec![("k", "v,x=y"), ("k", "v,x=y")],
            true,
        ),
        (
            "no dimensions at all, on both sides",
            vec![],
            vec![],
            true,
        ),        (
            "a dimension added, not repeated",
            vec![("a", "1")],
            vec![("a", "1"), ("b", "2")],
            false,
        ),
        (
            "the same dimension name with another value",
            vec![("a", "1")],
            vec![("a", "1"), ("a", "2")],
            false,
        ),
    ]
}

/// The phrase the uniqueness rule is refused with.
const UNIQUE_METRIC_PHRASE: &str =
    "each target tracking configuration must specify a unique scaling metric";

#[tokio::test]
async fn two_configurations_whose_dimension_sets_match_track_one_metric() {
    for (case, first, second, same_metric) in dimension_pairs() {
        let emulator = Emulator::new();
        let request = json!({
            "ScalingPlanName": OWNER,
            "ApplicationSource": json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            "ScalingInstructions": [instruction_tracking(vec![
                customized_metric("MyApp", "Queue", &first, 40.0),
                customized_metric("MyApp", "Queue", &second, 50.0),
            ])],
        });
        let (status, body) = emulator.call("CreateScalingPlan", &request).await;
        if same_metric {
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: two configurations tracking one metric must be refused, \
                 got {status} with {body:?}"
            );
            assert!(
                error_message(&body).contains(UNIQUE_METRIC_PHRASE),
                "case {case:?}: expected the uniqueness refusal to say {UNIQUE_METRIC_PHRASE:?}, \
                 got {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: two configurations tracking different metrics must be \
             accepted, got {status} with {body:?}"
        );
        emulator.make_all_due();
        let plan = describe_one(&emulator, OWNER).await;
        let configurations = plan["ScalingInstructions"][0]["TargetTrackingConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            configurations.len(),
            2,
            "case {case:?}: both configurations must be stored, got {plan:?}"
        );
    }
}

/// The target values of the configurations one plan's single instruction holds, in the
/// order they are reported.
async fn stored_target_values(emulator: &Emulator, name: &str) -> Vec<f64> {
    let plan = describe_one(emulator, name).await;
    plan["ScalingInstructions"][0]["TargetTrackingConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|configuration| configuration["TargetValue"].as_f64().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn an_update_pairs_a_configuration_whose_dimension_set_is_respelled() {
    // "The target value in force for a metric the resource already tracks is kept": a
    // re-sent configuration naming the SAME dimension set with a repeated entry is that
    // same metric, so its target value stays, while a configuration for a metric the
    // resource does not track yet takes the value requested.
    const IN_FORCE_TARGET: f64 = 40.0;
    const REQUESTED_TARGET: f64 = 70.0;
    const NEW_METRIC_TARGET: f64 = 80.0;

    for (case, stated, repeated, same_metric) in dimension_pairs() {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": OWNER,
                    "ApplicationSource": json!({
                        "TagFilters": [{ "Key": "env", "Values": ["prod"] }]
                    }),
                    "ScalingInstructions": [instruction_tracking(vec![customized_metric(
                        "MyApp",
                        "Queue",
                        &stated,
                        IN_FORCE_TARGET,
                    )])],
                }),
            )
            .await;
        emulator.make_all_due();

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": OWNER,
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": [instruction_tracking(vec![
                        customized_metric("MyApp", "Queue", &repeated, REQUESTED_TARGET),
                        customized_metric("MyApp", "Other", &[], NEW_METRIC_TARGET),
                    ])],
                }),
            )
            .await;
        emulator.make_all_due();

        let expected_first = if same_metric {
            IN_FORCE_TARGET
        } else {
            REQUESTED_TARGET
        };
        let targets = stored_target_values(&emulator, OWNER).await;
        assert_eq!(
            targets,
            vec![expected_first, NEW_METRIC_TARGET],
            "case {case:?}: the metric already tracked keeps the target value in force \
             and a metric not tracked yet takes the requested one, got {targets:?}"
        );
    }
}
