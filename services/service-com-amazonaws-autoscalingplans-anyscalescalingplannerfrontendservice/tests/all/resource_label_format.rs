// SPDX-License-Identifier: CC0-1.0

//! A `ResourceLabel` is accepted only in the format its own documentation states.
//!
//! Both members that carry one — `PredefinedScalingMetricSpecification$ResourceLabel`
//! and `PredefinedLoadMetricSpecification$ResourceLabel`
//! (`smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="ResourceLabel")`,
//! and the identical paragraph on the load-metric sibling) — document the same value:
//! "You create the resource label by appending the final portion of the load balancer ARN
//! and the final portion of the target group ARN into a single value, separated by a
//! forward slash (/). The format is
//! app/<load-balancer-name>/<load-balancer-id>/targetgroup/<target-group-name>/<target-group-id>".
//! A label "identifies the resource associated with the metric type", so a value not in
//! that format identifies no target group and is refused with the modelled
//! `ValidationException` (HTTP 400), on every path that writes one; a label in that
//! format is stored and reported back verbatim.
//!
//! The rows below differ along what the format generalises over: how many segments the
//! value has (none, too few, exactly six, too many), which segment is empty, where the
//! two markers sit and how they are spelled, whether a separator leads or trails, and how
//! long the names and ids are — up to the modelled 1023-character maximum. Each row is
//! driven through both members and through both write paths (`CreateScalingPlan` and
//! `UpdateScalingPlan`), which share one validator.
//!
//! The harness (router, dispatch, error-body readers, the lifecycle clock) is reused from
//! [`crate::scaling_plan_lifecycle`] and the instruction/plan readers from
//! [`crate::identity_key_separators`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    ALB_LOAD_METRIC_TYPE, ALB_SCALING_METRIC_TYPE, RESOURCE_LABEL_FORMAT,
};

use crate::identity_key_separators::{describe_one, instruction_tracking};
use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan every case writes to.
const PLAN: &str = "label-format";

/// The modelled maximum length of either `ResourceLabel`
/// (`smithy_query(query="shape",shape="PredefinedScalingMetricSpecification,PredefinedLoadMetricSpecification")`:
/// `length {min: 1, max: 1023}` on both).
const MAX_RESOURCE_LABEL: usize = 1023;

/// The target value every labelled configuration tracks to.
const TARGET_VALUE: f64 = 1_000.0;

/// A label exactly `length` characters long: the documented format with its target-group
/// id padded out.
fn label_of_length(length: usize) -> String {
    let prefix = "app/my-lb/abc123/targetgroup/tg-a/";
    format!("{prefix}{}", "i".repeat(length.saturating_sub(prefix.len())))
}

/// One spelling of a resource label, and whether the documented format admits it.
struct Row {
    label: &'static str,
    value: String,
    accepted: bool,
}

fn accepted(label: &'static str, value: impl Into<String>) -> Row {
    Row {
        label,
        value: value.into(),
        accepted: true,
    }
}

fn refused(label: &'static str, value: impl Into<String>) -> Row {
    Row {
        label,
        value: value.into(),
        accepted: false,
    }
}

/// The table every test in this suite spans.
fn rows() -> Vec<Row> {
    vec![
        // The worked example the members' own documentation gives.
        accepted(
            "the documented worked example",
            "app/EC2Co-EcsEl-1TKLTMITMM0EO/f37c06a68c1748aa/targetgroup/EC2Co-Defau-LDNM7Q3ZH1ZN/\
             6d4ea56ca2d6a18d",
        ),
        accepted(
            "one-character names and ids",
            "app/a/b/targetgroup/c/d",
        ),
        accepted(
            "names holding the characters a load balancer and a target group name may hold",
            "app/my-lb-01/0a1b2c3d4e5f6071/targetgroup/my-tg-02/0f1e2d3c4b5a6978",
        ),
        accepted(
            "a label at the modelled 1023-character maximum",
            label_of_length(MAX_RESOURCE_LABEL),
        ),
        refused("no separator at all", "not-a-label"),
        refused("the load balancer portion alone", "app/my-lb/abc123"),
        refused("the target group portion alone", "targetgroup/tg-a/111"),
        refused(
            "the two portions in the other order",
            "targetgroup/tg-a/111/app/my-lb/abc123",
        ),
        refused(
            "the load balancer marker misspelled",
            "application/my-lb/abc123/targetgroup/tg-a/111",
        ),
        refused(
            "the target group marker misspelled",
            "app/my-lb/abc123/target-group/tg-a/111",
        ),
        refused(
            "the whole load balancer ARN rather than its final portion",
            "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/my-lb/abc123/\
             targetgroup/tg-a/111",
        ),
        refused(
            "a segment too many, past the target group id",
            "app/my-lb/abc123/targetgroup/tg-a/111/222",
        ),
        refused(
            "a segment too few, the load balancer id missing",
            "app/my-lb/targetgroup/tg-a/111",
        ),
        refused(
            "an empty load balancer name",
            "app//abc123/targetgroup/tg-a/111",
        ),
        refused(
            "an empty load balancer id",
            "app/my-lb//targetgroup/tg-a/111",
        ),
        refused(
            "an empty target group name",
            "app/my-lb/abc123/targetgroup//111",
        ),
        refused(
            "an empty target group id",
            "app/my-lb/abc123/targetgroup/tg-a/",
        ),
        refused(
            "a leading separator",
            "/app/my-lb/abc123/targetgroup/tg-a/111",
        ),
        refused("separators only", "/////"),
        refused(
            "the markers in their places but nothing else",
            "app/targetgroup",
        ),
    ]
}

/// The two members that carry a resource label.
#[derive(Clone, Copy, Debug)]
enum Member {
    /// `TargetTrackingConfiguration$PredefinedScalingMetricSpecification`.
    Scaling,
    /// `ScalingInstruction$PredefinedLoadMetricSpecification`.
    Load,
}

impl Member {
    /// One scaling instruction carrying `label` on this member.
    fn instruction(self, label: &str) -> Value {
        match self {
            Member::Scaling => instruction_tracking(vec![json!({
                "PredefinedScalingMetricSpecification": {
                    "PredefinedScalingMetricType": ALB_SCALING_METRIC_TYPE,
                    "ResourceLabel": label
                },
                "TargetValue": TARGET_VALUE
            })]),
            Member::Load => {
                let mut instruction = instruction_tracking(vec![json!({
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "ASGAverageCPUUtilization"
                    },
                    "TargetValue": 40.0
                })]);
                instruction["PredefinedLoadMetricSpecification"] = json!({
                    "PredefinedLoadMetricType": ALB_LOAD_METRIC_TYPE,
                    "ResourceLabel": label
                });
                instruction
            }
        }
    }

    /// The wire path of the specification the label sits in, as the refusal names it.
    fn path(self) -> &'static str {
        match self {
            Member::Scaling => {
                "ScalingInstructions[0].TargetTrackingConfigurations[0]\
                 .PredefinedScalingMetricSpecification"
            }
            Member::Load => "ScalingInstructions[0].PredefinedLoadMetricSpecification",
        }
    }

    /// The label the stored plan reports on this member, read back through
    /// `DescribeScalingPlans`.
    async fn stored_label(self, emulator: &Emulator) -> Option<String> {
        let plan = describe_one(emulator, PLAN).await;
        let instruction = &plan["ScalingInstructions"][0];
        let specification = match self {
            Member::Scaling => {
                &instruction["TargetTrackingConfigurations"][0]
                    ["PredefinedScalingMetricSpecification"]
            }
            Member::Load => &instruction["PredefinedLoadMetricSpecification"],
        };
        specification
            .get("ResourceLabel")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The one refusal a label outside the documented format is answered with.
fn expected_message(member: Member, value: &str) -> String {
    format!(
        "{}.ResourceLabel: Value '{value}' failed to satisfy constraint: Member must be \
         in the format {RESOURCE_LABEL_FORMAT}",
        member.path()
    )
}

#[tokio::test]
async fn create_takes_a_resource_label_only_in_the_documented_format() {
    for row in rows() {
        for member in [Member::Scaling, Member::Load] {
            let case = row.label;
            let emulator = Emulator::new();
            let (status, body) = emulator
                .call(
                    "CreateScalingPlan",
                    &json!({
                        "ScalingPlanName": PLAN,
                        "ApplicationSource": stack_source(1),
                        "ScalingInstructions": [member.instruction(&row.value)],
                    }),
                )
                .await;

            if row.accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}/{member:?}: a label in the documented format must be \
                     accepted, got {status} with {body:?}"
                );
                emulator.make_all_due();
                let stored = member.stored_label(&emulator).await;
                assert_eq!(
                    stored.as_deref(),
                    Some(row.value.as_str()),
                    "case {case:?}/{member:?}: the stored plan must report the label as \
                     sent, got {stored:?}"
                );
                continue;
            }

            assert_eq!(
                (status, error_code(&body), error_message(&body)),
                (
                    400,
                    "ValidationException",
                    expected_message(member, &row.value).as_str()
                ),
                "case {case:?}/{member:?}: a label outside the documented format must be \
                 refused, got {status} with {body:?}"
            );
            // A refused create stores nothing: the plan it would have made does not exist.
            let described = emulator
                .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [PLAN] }))
                .await;
            assert_eq!(
                described.get("ScalingPlans"),
                Some(&json!([])),
                "case {case:?}/{member:?}: a refused create must store no plan, got \
                 {described}"
            );
        }
    }
}

#[tokio::test]
async fn update_applies_the_same_format_rule_to_the_plan_in_force() {
    for row in rows() {
        for member in [Member::Scaling, Member::Load] {
            let case = row.label;
            let emulator = Emulator::new();
            emulator
                .ok(
                    "CreateScalingPlan",
                    json!({
                        "ScalingPlanName": PLAN,
                        "ApplicationSource": stack_source(2),
                        "ScalingInstructions": simple_instructions(),
                    }),
                )
                .await;
            emulator.make_all_due();

            let (status, body) = emulator
                .call(
                    "UpdateScalingPlan",
                    &json!({
                        "ScalingPlanName": PLAN,
                        "ScalingPlanVersion": 1,
                        "ScalingInstructions": [member.instruction(&row.value)],
                    }),
                )
                .await;
            emulator.make_all_due();
            let stored = member.stored_label(&emulator).await;

            if row.accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}/{member:?}: a label in the documented format must be \
                     accepted on update, got {status} with {body:?}"
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(row.value.as_str()),
                    "case {case:?}/{member:?}: the updated plan must report the label as \
                     sent, got {stored:?}"
                );
                continue;
            }

            assert_eq!(
                (status, error_code(&body), error_message(&body)),
                (
                    400,
                    "ValidationException",
                    expected_message(member, &row.value).as_str()
                ),
                "case {case:?}/{member:?}: a label outside the documented format must be \
                 refused on update, got {status} with {body:?}"
            );
            // A refused update leaves the plan in force exactly as it was: the
            // instruction it was created with carries no label at all.
            assert_eq!(
                stored, None,
                "case {case:?}/{member:?}: a refused update must not reach the stored \
                 plan, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_policy_reports_only_a_label_in_the_documented_format() {
    // The `ScalingPolicy` a `ScalingPlanResource` mints carries the configuration the
    // instruction was accepted with, so the identity of the tracked resource it reports
    // is a label in the documented format for every plan that exists.
    for row in rows().into_iter().filter(|row| row.accepted) {
        let case = row.label;
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": PLAN,
                    "ApplicationSource": stack_source(3),
                    "ScalingInstructions": [Member::Scaling.instruction(&row.value)],
                }),
            )
            .await;
        emulator.make_all_due();

        let body = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 }),
            )
            .await;
        let reported: Vec<Option<String>> = body["ScalingPlanResources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|resource| {
                resource["ScalingPolicies"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .map(|policy| {
                policy["TargetTrackingConfiguration"]["PredefinedScalingMetricSpecification"]
                    ["ResourceLabel"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            reported,
            vec![Some(row.value.clone())],
            "case {case:?}: the minted policy must report the label the instruction \
             carries, got {reported:?}"
        );
    }
}
