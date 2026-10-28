// SPDX-License-Identifier: CC0-1.0

//! Values that differ only in where a separator falls are DIFFERENT values, everywhere
//! this service decides a rule on the identity of a composite request member.
//!
//! Two rules are decided that way: "You can create one scaling plan per application
//! source" (`CreateScalingPlanRequest$ApplicationSource`), which the
//! `DescribeScalingPlans` `ApplicationSources` filter selects by as well, and "Each of
//! these structures must specify a unique scaling metric"
//! (`ScalingInstruction$TargetTrackingConfigurations`), which the update merge keeps a
//! tracked metric's target value by. Both identities are built out of the caller's own
//! text, and the model lets that text hold every punctuation character:
//! `smithy_query(query="shape",shape="TagFilter,TagValues")` constrains a tag key and a
//! tag value only to `^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\r\n\t]*$`
//! — which contains `=`, `,`, `;`, `/`, `[`, `]` and `\` — and
//! `smithy_query(query="shape",shape="MetricNamespace,MetricName,MetricDimensionName,MetricDimensionValue")`
//! shows plain strings under no pattern at all.
//!
//! So a request whose text merely SPELLS a separator is served on its own terms: a
//! source no stored plan names is created, a plan is reported only for the source it
//! actually has, and a metric no configuration tracks yet is a second metric. A set
//! written in another order is still the same set, which the rows below keep pinned.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan that claims a source first.
const OWNER: &str = "plan-a";
/// The second plan, which claims or updates onto another source.
const CHALLENGER: &str = "plan-b";

/// The refusal wording of the one-plan-per-application-source rule.
///
/// `pub(crate)` so a suite about another way of re-spelling one identity words this
/// refusal the same way rather than keeping a second copy of it.
pub(crate) fn covered_message(covered_by: &str) -> String {
    format!("The application source is already covered by the scaling plan '{covered_by}'")
}

/// Application sources no two of which name the same thing: a stack, tag-filter sets of
/// one and two filters, and — the rows the separators are at stake in — sets whose text
/// spells the separator another row's structure uses, including the escape character
/// itself, written once and twice.
fn distinct_sources() -> Vec<(&'static str, Value)> {
    vec![
        (
            "two filters",
            json!({ "TagFilters": [
                { "Key": "env", "Values": ["prod"] },
                { "Key": "team", "Values": ["x"] }
            ]}),
        ),
        (
            "one filter whose value spells those two filters",
            json!({ "TagFilters": [{ "Key": "env", "Values": ["prod;team=x"] }] }),
        ),
        (
            "one filter with two values",
            json!({ "TagFilters": [{ "Key": "tier", "Values": ["web", "api"] }] }),
        ),
        (
            "one filter whose single value spells those two values",
            json!({ "TagFilters": [{ "Key": "tier", "Values": ["api,web"] }] }),
        ),
        (
            "a key that spells a key and a value",
            json!({ "TagFilters": [{ "Key": "env=prod", "Values": ["yes"] }] }),
        ),
        (
            "the same text split differently across key and value",
            json!({ "TagFilters": [{ "Key": "env", "Values": ["prod=yes"] }] }),
        ),
        (
            "a value holding one escape character",
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a\\;b"] }] }),
        ),
        (
            "a value holding two escape characters",
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a\\\\;b"] }] }),
        ),
        ("a CloudFormation stack", stack_source(7)),
    ]
}

/// Every unordered pair of [`distinct_sources`]: two sources that must stay two.
fn source_pairs() -> Vec<(String, Value, Value)> {
    let sources = distinct_sources();
    let mut pairs = Vec::new();
    for (first, (left_label, left)) in sources.iter().enumerate() {
        for (right_label, right) in sources.iter().skip(first + 1) {
            pairs.push((
                format!("{left_label} vs {right_label}"),
                left.clone(),
                right.clone(),
            ));
        }
    }
    pairs
}

/// Create one plan over `source`, require HTTP 200, and settle it to `Active`.
pub(crate) async fn create_active_plan(emulator: &Emulator, name: &str, source: &Value) {
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
pub(crate) async fn describe_one(emulator: &Emulator, name: &str) -> Value {
    let described = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [name] }),
        )
        .await;
    described["ScalingPlans"][0].clone()
}

/// The names of the plans `DescribeScalingPlans` reports for one application source.
pub(crate) async fn names_covering(emulator: &Emulator, source: &Value) -> Vec<String> {
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
async fn a_create_accepts_a_source_that_only_looks_like_a_covered_one() {
    for (case, claimed, other) in source_pairs() {
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &claimed).await;

        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": CHALLENGER,
                    "ApplicationSource": other,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for an application source no stored plan \
             names, got {status} with {body:?}"
        );
        emulator.make_all_due();

        // Each plan is reported for ITS source and for no other.
        for (label, source, expected) in [
            ("the claimed source", &claimed, OWNER),
            ("the other source", &other, CHALLENGER),
        ] {
            let covering = names_covering(&emulator, source).await;
            assert_eq!(
                covering,
                vec![expected.to_string()],
                "case {case:?}/{label}: only the plan that has this application source \
                 may be reported for it, got {covering:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_accepts_a_source_that_only_looks_like_a_covered_one() {
    for (case, claimed, other) in source_pairs() {
        let emulator = Emulator::new();
        create_active_plan(&emulator, OWNER, &claimed).await;
        create_active_plan(&emulator, CHALLENGER, &stack_source(42)).await;

        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": CHALLENGER,
                    "ScalingPlanVersion": 1,
                    "ApplicationSource": other,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for an update onto an application source \
             no other plan names, got {status} with {body:?}"
        );
        emulator.make_all_due();

        let challenger = describe_one(&emulator, CHALLENGER).await;
        assert_eq!(
            challenger.get("ApplicationSource"),
            Some(&other),
            "case {case:?}: the updated source must be stored as sent, got {challenger:?}"
        );
    }
}

/// One tag-filter set written two ways: the same filters and values, reordered, with
/// separator characters inside the text that is reordered.
fn respelled_sources() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "two filters with separators in their values, filters reordered",
            json!({ "TagFilters": [
                { "Key": "env", "Values": ["prod;team=x"] },
                { "Key": "team", "Values": ["y,z"] }
            ]}),
            json!({ "TagFilters": [
                { "Key": "team", "Values": ["y,z"] },
                { "Key": "env", "Values": ["prod;team=x"] }
            ]}),
        ),
        (
            "one filter whose separator-bearing values are reordered",
            json!({ "TagFilters": [{ "Key": "tier", "Values": ["a,b", "c;d"] }] }),
            json!({ "TagFilters": [{ "Key": "tier", "Values": ["c;d", "a,b"] }] }),
        ),
        (
            "one filter whose value holds an escape character, values reordered",
            json!({ "TagFilters": [{ "Key": "path", "Values": ["a\\b", "c\\d"] }] }),
            json!({ "TagFilters": [{ "Key": "path", "Values": ["c\\d", "a\\b"] }] }),
        ),
    ]
}

#[tokio::test]
async fn one_source_spelled_two_ways_is_still_one_source() {
    for (case, claimed, respelled) in respelled_sources() {
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
            (
                400,
                "ValidationException",
                covered_message(OWNER).as_str()
            ),
            "case {case:?}: a set of tags written in another order is the same \
             application source, got {status} with {body:?}"
        );

        for (label, source) in [("as claimed", &claimed), ("respelled", &respelled)] {
            let covering = names_covering(&emulator, source).await;
            assert_eq!(
                covering,
                vec![OWNER.to_string()],
                "case {case:?}/{label}: both spellings must select the one plan that \
                 has the source, got {covering:?}"
            );
        }
    }
}

/// One scaling instruction over the configurations given.
///
/// The resource triple is drawn from the model's own value lists, as
/// [`crate::scaling_plan_lifecycle::instruction`] draws it; only the configurations
/// vary here.
pub(crate) fn instruction_tracking(configurations: Vec<Value>) -> Value {
    json!({
        "ServiceNamespace": "autoscaling",
        "ResourceId": "autoScalingGroup/my-asg",
        "ScalableDimension": "autoscaling:autoScalingGroup:DesiredCapacity",
        "MinCapacity": 1,
        "MaxCapacity": 10,
        "TargetTrackingConfigurations": configurations
    })
}

/// A target tracking configuration over a customized metric.
///
/// `Statistic` is a modelled `MetricStatistic` value
/// (`smithy_query(query="shape",shape="MetricStatistic")`, mirrored as
/// `METRIC_STATISTIC_VALUES`); `dimensions` is `(name, value)` pairs.
pub(crate) fn customized_metric(
    namespace: &str,
    metric_name: &str,
    dimensions: &[(&str, &str)],
    target: f64,
) -> Value {
    let dimensions: Vec<Value> = dimensions
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    json!({
        "CustomizedScalingMetricSpecification": {
            "Namespace": namespace,
            "MetricName": metric_name,
            "Statistic": "Average",
            "Dimensions": dimensions
        },
        "TargetValue": target
    })
}

/// Pairs of customized metrics, and whether the two name one metric or two.
fn metric_pairs() -> Vec<(&'static str, Value, Value, bool)> {
    vec![
        (
            "a separator inside the namespace vs inside the metric name",
            customized_metric("a/b", "c", &[], 40.0),
            customized_metric("a", "b/c", &[], 50.0),
            true,
        ),
        (
            "one dimension spelling two dimensions",
            customized_metric("MyApp", "Queue", &[("k", "v,x=y")], 40.0),
            customized_metric("MyApp", "Queue", &[("k", "v"), ("x", "y")], 50.0),
            true,
        ),
        (
            "a dimension name spelling a name and a value",
            customized_metric("MyApp", "Queue", &[("k=v", "w")], 40.0),
            customized_metric("MyApp", "Queue", &[("k", "v=w")], 50.0),
            true,
        ),
        (
            "a metric name holding the dimension brackets",
            customized_metric("MyApp", "Queue[k=v]", &[], 40.0),
            customized_metric("MyApp", "Queue", &[("k", "v")], 50.0),
            true,
        ),
        (
            "the same metric, its dimensions reordered",
            customized_metric("MyApp", "Queue", &[("k", "v"), ("x", "y")], 40.0),
            customized_metric("MyApp", "Queue", &[("x", "y"), ("k", "v")], 50.0),
            false,
        ),
    ]
}

#[tokio::test]
async fn two_metrics_that_only_look_alike_are_two_metrics() {
    for (case, first, second, distinct) in metric_pairs() {
        let emulator = Emulator::new();
        let request = json!({
            "ScalingPlanName": OWNER,
            "ApplicationSource": stack_source(1),
            "ScalingInstructions": [instruction_tracking(vec![first, second])],
        });
        let (status, body) = emulator.call("CreateScalingPlan", &request).await;
        if !distinct {
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: two configurations tracking one metric must be refused, \
                 got {status} with {body:?}"
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

#[tokio::test]
async fn an_update_keeps_the_target_value_of_the_metric_actually_in_force() {
    // The merge keeps the target value in force for a metric the resource already
    // tracks, so a metric that merely LOOKS like it must get the value the request
    // asks for rather than the one in force.
    let emulator = Emulator::new();
    let in_force = customized_metric("a/b", "c", &[], 40.0);
    let look_alike = customized_metric("a", "b/c", &[], 80.0);
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": OWNER,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [instruction_tracking(vec![in_force])],
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
                    customized_metric("a/b", "c", &[], 70.0),
                    look_alike,
                ])],
            }),
        )
        .await;
    emulator.make_all_due();

    let plan = describe_one(&emulator, OWNER).await;
    let configurations = plan["ScalingInstructions"][0]["TargetTrackingConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let targets: Vec<(String, String, f64)> = configurations
        .iter()
        .map(|configuration| {
            let metric = &configuration["CustomizedScalingMetricSpecification"];
            (
                metric["Namespace"].as_str().unwrap_or_default().to_string(),
                metric["MetricName"].as_str().unwrap_or_default().to_string(),
                configuration["TargetValue"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        targets,
        vec![
            ("a/b".to_string(), "c".to_string(), 40.0),
            ("a".to_string(), "b/c".to_string(), 80.0),
        ],
        "case {:?}: the tracked metric keeps the target value in force and the \
         look-alike takes the one requested, got {targets:?}",
        "a separator inside the namespace vs inside the metric name"
    );
}
