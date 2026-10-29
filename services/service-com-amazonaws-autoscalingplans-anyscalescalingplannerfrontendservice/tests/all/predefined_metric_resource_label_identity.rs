// SPDX-License-Identifier: CC0-1.0

//! The metric a target tracking configuration tracks is identified by EVERY modelled
//! part that says which metric it is — for a predefined metric, the metric type together
//! with the resource label it is measured on.
//!
//! `PredefinedScalingMetricSpecification$ResourceLabel`
//! (`smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="ResourceLabel")`)
//! "identifies the resource associated with the metric type", built "by appending the
//! final portion of the load balancer ARN and the final portion of the target group ARN
//! into a single value, separated by a forward slash (/)" — so one metric type over two
//! labels measures two different target groups, and that is two metrics. Three rules of
//! this service are decided on that identity and all three are driven here over the wire
//! path:
//!
//! * "Each of these structures must specify a unique scaling metric and a target value
//!   for the metric" (`smithy_query(query="shape",shape="ScalingInstruction")`), which
//!   both write paths enforce — so two labels in one instruction are accepted and stored,
//!   while the same label twice is refused;
//! * the pairing `UpdateScalingPlan` keeps a tracked metric's target value by — so a
//!   configuration re-pointed at another label is a metric no policy is in force for and
//!   takes the target value the request asks for;
//! * the `PolicyName` a `ScalingPlanResource` reports, which is a function of the plan,
//!   the resource and the metric — so two labels get two names, and re-pointing a label
//!   renames the policy.
//!
//! The rows below differ along what the rule generalises over: which portion of the label
//! differs (target group name, target group id, load balancer), a label against no label
//! at all, the metric TYPE with no label in play, where a separator falls inside the
//! label, a label holding the escape character, and labels at the modelled 1023-character
//! maximum differing in one character.
//!
//! The harness (router, dispatch, error-body readers, the lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`] and the plan readers from
//! [`crate::identity_key_separators`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::identity_key_separators::{customized_metric, describe_one, instruction_tracking};
use crate::scaling_plan_lifecycle::{error_code, error_message, stack_source, Emulator};

/// The plan every case writes to.
const PLAN: &str = "label-identity";

/// The documented maximum length of a `ResourceLabel`
/// (`smithy_query(query="shape",shape="PredefinedScalingMetricSpecification")`: len
/// 1..1023 on both `ResourceLabel` members).
const MAX_RESOURCE_LABEL: usize = 1023;

/// The phrase the uniqueness rule is refused with, as
/// [`crate::set_identity_multiplicity`] reads it.
const UNIQUE_METRIC_PHRASE: &str =
    "each target tracking configuration must specify a unique scaling metric";

/// A resource label in the documented
/// `app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>`
/// format.
fn alb_label(load_balancer: &str, target_group: &str, target_group_id: &str) -> String {
    format!("app/{load_balancer}/abc123/targetgroup/{target_group}/{target_group_id}")
}

/// A resource label exactly `length` characters long whose last character is `last`: the
/// documented format padded out in the target-group id.
fn label_of_length(length: usize, last: char) -> String {
    let prefix = "app/my-lb/abc123/targetgroup/tg-a/";
    let padding = length.saturating_sub(prefix.len() + last.len_utf8());
    format!("{prefix}{}{last}", "i".repeat(padding))
}

/// One predefined metric specification: the modelled metric type, and the resource label
/// where the row carries one.
#[derive(Debug, Clone)]
struct Specification {
    /// A `ScalingMetricType` value
    /// (`smithy_query(query="shape",shape="ScalingMetricType")`).
    metric_type: &'static str,
    resource_label: Option<String>,
}

impl Specification {
    fn new(metric_type: &'static str, resource_label: Option<String>) -> Self {
        Self {
            metric_type,
            resource_label,
        }
    }

    /// One target tracking configuration over this specification, tracking to `target`.
    fn configuration(&self, target: f64) -> Value {
        let mut specification = json!({ "PredefinedScalingMetricType": self.metric_type });
        if let Some(label) = &self.resource_label {
            specification["ResourceLabel"] = json!(label);
        }
        json!({
            "PredefinedScalingMetricSpecification": specification,
            "TargetValue": target
        })
    }
}

/// Two predefined specifications, and whether the two name ONE metric or two.
struct MetricPair {
    case: &'static str,
    first: Specification,
    second: Specification,
    same_metric: bool,
}

/// The ALB metric type, the only one a resource label may be specified for.
const ALB: &str = "ALBRequestCountPerTarget";

/// The table this suite spans.
fn metric_pairs() -> Vec<MetricPair> {
    let alb = |label: String| Specification::new(ALB, Some(label));
    vec![
        MetricPair {
            case: "two target groups of one load balancer",
            first: alb(alb_label("my-lb", "tg-a", "111")),
            second: alb(alb_label("my-lb", "tg-b", "222")),
            same_metric: false,
        },
        MetricPair {
            case: "one target group name under another target group id",
            first: alb(alb_label("my-lb", "tg-a", "111")),
            second: alb(alb_label("my-lb", "tg-a", "222")),
            same_metric: false,
        },
        MetricPair {
            case: "one target group behind another load balancer",
            first: alb(alb_label("my-lb", "tg-a", "111")),
            second: alb(alb_label("other-lb", "tg-a", "111")),
            same_metric: false,
        },
        MetricPair {
            case: "the same label on both sides",
            first: alb(alb_label("my-lb", "tg-a", "111")),
            second: alb(alb_label("my-lb", "tg-a", "111")),
            same_metric: true,
        },
        MetricPair {
            case: "a labelled metric against the same type with no label",
            first: alb(alb_label("my-lb", "tg-a", "111")),
            second: Specification::new(ALB, None),
            same_metric: false,
        },
        MetricPair {
            case: "the same type with no label on either side",
            first: Specification::new(ALB, None),
            second: Specification::new(ALB, None),
            same_metric: true,
        },
        MetricPair {
            case: "two metric types, neither labelled",
            first: Specification::new("ASGAverageCPUUtilization", None),
            second: Specification::new("ASGAverageNetworkIn", None),
            same_metric: false,
        },
        MetricPair {
            case: "one metric type twice, neither labelled",
            first: Specification::new("ASGAverageCPUUtilization", None),
            second: Specification::new("ASGAverageCPUUtilization", None),
            same_metric: true,
        },
        MetricPair {
            case: "the same label text split differently by the separator",
            first: alb("app/my-lb/abc123/targetgroup/tg-a1/2".to_string()),
            second: alb("app/my-lb/abc123/targetgroup/tg-a/12".to_string()),
            same_metric: false,
        },
        MetricPair {
            case: "a label holding the escape character, written once and twice",
            first: alb("app/my-lb/abc123/targetgroup/tg\\-a/111".to_string()),
            second: alb("app/my-lb/abc123/targetgroup/tg\\\\-a/111".to_string()),
            same_metric: false,
        },
        MetricPair {
            case: "two labels at the 1023-character bound differing in the last character",
            first: alb(label_of_length(MAX_RESOURCE_LABEL, 'a')),
            second: alb(label_of_length(MAX_RESOURCE_LABEL, 'b')),
            same_metric: false,
        },
        MetricPair {
            case: "one label at the 1023-character bound on both sides",
            first: alb(label_of_length(MAX_RESOURCE_LABEL, 'a')),
            second: alb(label_of_length(MAX_RESOURCE_LABEL, 'a')),
            same_metric: true,
        },
    ]
}

/// Create `PLAN` over `configurations` and let the clock apply it.
async fn create_plan(emulator: &Emulator, configurations: Vec<Value>) -> (u16, Value) {
    let answered = emulator
        .call(
            "CreateScalingPlan",
            &json!({
                "ScalingPlanName": PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [instruction_tracking(configurations)],
            }),
        )
        .await;
    emulator.make_all_due();
    answered
}

/// The `(ResourceLabel, TargetValue)` of every predefined configuration the one
/// instruction of `PLAN` holds, in the order they are reported.
async fn stored_metrics(emulator: &Emulator) -> Vec<(Option<String>, f64)> {
    let plan = describe_one(emulator, PLAN).await;
    plan["ScalingInstructions"][0]["TargetTrackingConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|configuration| {
            configuration
                .get("PredefinedScalingMetricSpecification")
                .is_some()
        })
        .map(|configuration| {
            let specification = &configuration["PredefinedScalingMetricSpecification"];
            (
                specification
                    .get("ResourceLabel")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                configuration["TargetValue"].as_f64().unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn two_predefined_configurations_are_one_metric_only_when_their_labels_match_too() {
    const FIRST_TARGET: f64 = 1_000.0;
    const SECOND_TARGET: f64 = 2_000.0;

    for pair in metric_pairs() {
        let case = pair.case;
        let emulator = Emulator::new();
        let (status, body) = create_plan(
            &emulator,
            vec![
                pair.first.configuration(FIRST_TARGET),
                pair.second.configuration(SECOND_TARGET),
            ],
        )
        .await;

        if pair.same_metric {
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: two configurations tracking one metric must be refused, \
                 got {status} with {body:?}"
            );
            assert!(
                error_message(&body).contains(UNIQUE_METRIC_PHRASE),
                "case {case:?}: expected the uniqueness refusal to say \
                 {UNIQUE_METRIC_PHRASE:?}, got {body:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: two configurations tracking different metrics must be \
             accepted, got {status} with {body:?}"
        );
        let stored = stored_metrics(&emulator).await;
        assert_eq!(
            stored,
            vec![
                (pair.first.resource_label.clone(), FIRST_TARGET),
                (pair.second.resource_label.clone(), SECOND_TARGET),
            ],
            "case {case:?}: both configurations must be stored as sent, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn an_update_keeps_the_target_value_only_of_the_metric_actually_in_force() {
    // "The target value in force for a scaling metric the resource already tracks is
    // kept": a re-sent configuration naming the same metric keeps it, while a
    // configuration re-pointed at another resource label tracks a metric no policy is in
    // force for and takes the value requested.
    const IN_FORCE_TARGET: f64 = 1_000.0;
    const REQUESTED_TARGET: f64 = 500.0;
    const UNTRACKED_TARGET: f64 = 12.5;

    for pair in metric_pairs() {
        let case = pair.case;
        let emulator = Emulator::new();
        let (status, body) =
            create_plan(&emulator, vec![pair.first.configuration(IN_FORCE_TARGET)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the plan under test must be created, got {status} with {body:?}"
        );

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": PLAN,
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": [instruction_tracking(vec![
                        pair.second.configuration(REQUESTED_TARGET),
                        // A metric the resource does not track yet, which must take the
                        // target value the request asks for whatever the labels do.
                        customized_metric("MyApp", "backlog", &[("Fleet", "orders")],
                            UNTRACKED_TARGET),
                    ])],
                }),
            )
            .await;
        emulator.make_all_due();

        let expected = if pair.same_metric {
            IN_FORCE_TARGET
        } else {
            REQUESTED_TARGET
        };
        let stored = stored_metrics(&emulator).await;
        assert_eq!(
            stored,
            vec![(pair.second.resource_label.clone(), expected)],
            "case {case:?}: the stored configuration must carry the requested label, and \
             the target value in force only for the metric actually in force, got \
             {stored:?}"
        );
    }
}

/// The `(PolicyName, ResourceLabel, TargetValue)` of every policy `PLAN` reports for the
/// one resource it covers.
async fn reported_policies(
    emulator: &Emulator,
    case: &str,
) -> Vec<(String, Option<String>, f64)> {
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 }),
        )
        .await;
    let resources = body["ScalingPlanResources"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no ScalingPlanResources in {body}"));
    assert_eq!(
        resources.len(),
        1,
        "case {case:?}: the plan covers one resource, got {resources:?}"
    );
    resources[0]["ScalingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|policy| {
            let configuration = &policy["TargetTrackingConfiguration"];
            let specification = &configuration["PredefinedScalingMetricSpecification"];
            (
                policy["PolicyName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                specification
                    .get("ResourceLabel")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                configuration["TargetValue"].as_f64().unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_policy_is_named_after_the_resource_label_its_metric_identifies() {
    const FIRST_TARGET: f64 = 1_000.0;
    const SECOND_TARGET: f64 = 2_000.0;

    for pair in metric_pairs() {
        if pair.same_metric {
            // Two configurations of one metric never reach a resource: the write path
            // refuses them, which the uniqueness case above pins.
            continue;
        }
        let case = pair.case;
        let emulator = Emulator::new();

        // Both metrics on one resource: one policy each, under two names.
        create_plan(
            &emulator,
            vec![
                pair.first.configuration(FIRST_TARGET),
                pair.second.configuration(SECOND_TARGET),
            ],
        )
        .await;
        let both = reported_policies(&emulator, case).await;
        assert_eq!(
            both.len(),
            2,
            "case {case:?}: one policy per configuration must be reported, got {both:?}"
        );
        assert_ne!(
            both[0].0, both[1].0,
            "case {case:?}: two metrics of one resource need two policy names, got {both:?}"
        );

        // Re-pointing the one configuration at the other metric renames the policy and
        // reports the configuration the request asked for.
        let emulator = Emulator::new();
        create_plan(&emulator, vec![pair.first.configuration(FIRST_TARGET)]).await;
        let before = reported_policies(&emulator, case).await;
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: the created plan must report its one policy, got {before:?}"
        );
        assert_eq!(
            (before[0].1.clone(), before[0].2),
            (pair.first.resource_label.clone(), FIRST_TARGET),
            "case {case:?}: the created plan must report its one configuration as sent, \
             got {before:?}"
        );

        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": PLAN,
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": [instruction_tracking(vec![
                        pair.second.configuration(SECOND_TARGET),
                    ])],
                }),
            )
            .await;
        emulator.make_all_due();

        let after = reported_policies(&emulator, case).await;
        assert_eq!(
            after.len(),
            1,
            "case {case:?}: the updated plan must report its one policy, got {after:?}"
        );
        assert_eq!(
            (after[0].1.clone(), after[0].2),
            (pair.second.resource_label.clone(), SECOND_TARGET),
            "case {case:?}: the re-pointed policy must report the requested label and \
             target value, got {after:?}"
        );
        assert_ne!(
            after[0].0, before[0].0,
            "case {case:?}: a policy minted for another metric needs another name, got \
             {after:?} after {before:?}"
        );
        // And the name the other metric gets is the name that metric has wherever it is
        // written: the same configuration, created rather than updated into place.
        let elsewhere = Emulator::new();
        create_plan(&elsewhere, vec![pair.second.configuration(SECOND_TARGET)]).await;
        let created = reported_policies(&elsewhere, case).await;
        assert_eq!(
            created.len(),
            1,
            "case {case:?}: the other metric's plan must report its one policy, got \
             {created:?}"
        );
        assert_eq!(
            created[0].0, after[0].0,
            "case {case:?}: the policy name is a function of the plan, the resource and \
             the metric, so it must not depend on how the configuration got there, got \
             {created:?} vs {after:?}"
        );
    }
}
