// SPDX-License-Identifier: CC0-1.0

//! What the service COMPUTES for a `ScalingInstruction` member the request left out.
//!
//! ONE rule is pinned here: an optional member the model documents a default value for is
//! read with that documented value wherever the service computes with it, so an
//! instruction that leaves the member out and one that spells the documented value out
//! describe the same instruction and are answered the same way — while what the stored
//! instruction echoes back is still exactly what was submitted, the member included or
//! absent as it was sent.
//!
//! Rows differ along what the rule generalises over: the member, over EVERY member of
//! `ScalingInstruction` whose documentation states a default (an integer of seconds, two
//! enums and a boolean), crossed with instruction shapes differing in capacity band and
//! target value, and each pair is compared over every modelled `ForecastDataType` as well
//! as over the resource answer the same plan reports.
//!
//! The harness (router, dispatch, instruction builder) is reused from
//! [`crate::scaling_plan_lifecycle`], the data-point reader from
//! [`crate::forecast_datapoints`] and the plan version from [`crate::forecast_data`];
//! nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS, FORECAST_INTERVAL_SECONDS,
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::forecast_data::ONLY_VERSION;
use crate::forecast_datapoints::{assert_close, points};
use crate::scaling_plan_lifecycle::{instruction, now_secs, stack_source, Emulator};

/// The Auto Scaling group every plan here scales.
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// The plan created WITHOUT the member under test.
const OMITTED_PLAN: &str = "omitted";

/// The plan created WITH the member set to the value the model documents as its default.
const EXPLICIT_PLAN: &str = "explicit";

/// How many forecast intervals the window spans: more than a day, so the forecast varies
/// over it and a pair is compared on many intervals rather than one.
const WINDOW_PERIODS: i64 = 26;

/// How many intervals before the current one the window starts, so the whole window lies
/// inside the span forecast data exists over whatever the serving clock reads.
const WINDOW_START_PERIODS_AGO: i64 = 12;

/// The interval-aligned window every row asks its forecasts over.
fn window() -> (i64, i64) {
    let start = now_secs().div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS
        - WINDOW_START_PERIODS_AGO * FORECAST_INTERVAL_SECONDS;
    (start, start + WINDOW_PERIODS * FORECAST_INTERVAL_SECONDS)
}

/// One member of `ScalingInstruction` whose documentation states a default value.
#[derive(Debug, Clone)]
struct Member {
    /// The member, as the wire names it.
    name: &'static str,
    /// The value its documentation states for an instruction that leaves it out.
    documented_default: Value,
}

/// EVERY member of `ScalingInstruction` whose documentation states a default, with the
/// value it states. Source: `smithy_query(query="shape",shape="ScalingInstruction")` and
/// `smithy_query(query="doc",shape="ScalingInstruction",member="<Member>")` — the sentences
/// are, member by member:
///
/// * `ScheduledActionBufferTime` — "The default is 300 seconds.";
/// * `PredictiveScalingMode` — "The default value is `ForecastAndScale`.";
/// * `PredictiveScalingMaxCapacityBehavior` — "The default value is
///   `SetForecastCapacityToMaxCapacity`.";
/// * `ScalingPolicyUpdateBehavior` — "The default value is `KeepExternalPolicies`.";
/// * `DisableDynamicScaling` — "The default is enabled (`false`)."
///
/// The remaining optional members (`PredefinedLoadMetricSpecification`,
/// `CustomizedLoadMetricSpecification` and `PredictiveScalingMaxCapacityBuffer`) state no
/// default, so the rule says nothing about them and they are not rows.
fn members() -> Vec<Member> {
    vec![
        Member {
            name: "ScheduledActionBufferTime",
            documented_default: json!(DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS),
        },
        Member {
            name: "PredictiveScalingMode",
            documented_default: json!("ForecastAndScale"),
        },
        Member {
            name: "PredictiveScalingMaxCapacityBehavior",
            documented_default: json!("SetForecastCapacityToMaxCapacity"),
        },
        Member {
            name: "ScalingPolicyUpdateBehavior",
            documented_default: json!("KeepExternalPolicies"),
        },
        Member {
            name: "DisableDynamicScaling",
            documented_default: json!(false),
        },
    ]
}

/// One instruction shape the members are driven on: the band and target value every value
/// of the answer is computed from, so a pair is compared on more than one forecast curve.
#[derive(Debug, Clone, Copy)]
struct Shape {
    label: &'static str,
    min_capacity: i64,
    max_capacity: i64,
    target: f64,
}

/// The shapes: a wide band whose forecast leaves it, and a narrow band whose own minimum
/// capacity floors the scheduled-action minimum.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            label: "a wide band",
            min_capacity: 2,
            max_capacity: 10,
            target: 40.0,
        },
        Shape {
            label: "a narrow band floored by its own minimum",
            min_capacity: 9,
            max_capacity: 10,
            target: 50.0,
        },
    ]
}

impl Shape {
    /// This shape's instruction, carrying a load metric specification so predictive scaling
    /// is configured (which is what the predictive members require) and leaving dynamic
    /// scaling on (which is what the dynamic member requires), plus `member` when given.
    fn instruction(&self, member: Option<&Member>) -> Value {
        let mut value = instruction(
            FORECAST_SERVICE_NAMESPACE,
            RESOURCE,
            FORECAST_SCALABLE_DIMENSION,
            self.min_capacity,
            self.max_capacity,
            "ASGAverageCPUUtilization",
            self.target,
        );
        value["PredefinedLoadMetricSpecification"] =
            json!({ "PredefinedLoadMetricType": "ASGTotalCPUUtilization" });
        if let Some(member) = member {
            value[member.name] = member.documented_default.clone();
        }
        value
    }
}

/// An emulator holding both plans of one pair: the instruction without the member, and the
/// same instruction with the member set to its documented default. Each plan brings its own
/// application source, one plan being allowed per source.
async fn emulator_with(shape: Shape, member: &Member) -> Emulator {
    let emulator = Emulator::new();
    for (index, (plan, held)) in [
        (OMITTED_PLAN, None),
        (EXPLICIT_PLAN, Some(member)),
    ]
    .into_iter()
    .enumerate()
    {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": plan,
                    "ApplicationSource": stack_source(index + 1),
                    "ScalingInstructions": [shape.instruction(held)],
                }),
            )
            .await;
    }
    emulator.make_all_due();
    emulator
}

/// The resource answer of one plan, with the plan's own name dropped from each resource and
/// from the name of each policy that identifies the plan it belongs to: what is left is
/// everything the service computed from the instruction itself.
async fn resources(emulator: &Emulator, plan: &str) -> Value {
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": plan, "ScalingPlanVersion": ONLY_VERSION }),
        )
        .await;
    let listed = body
        .get("ScalingPlanResources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {plan}: expected a ScalingPlanResources list, got {body}"))
        .clone();
    assert!(
        !listed.is_empty(),
        "case {plan}: expected the plan's resource to be reported, got {body}"
    );
    Value::Array(
        listed
            .into_iter()
            .map(|mut resource| {
                if let Some(object) = resource.as_object_mut() {
                    object.remove("ScalingPlanName");
                }
                for policy in resource["ScalingPolicies"]
                    .as_array_mut()
                    .into_iter()
                    .flatten()
                {
                    let named = policy["PolicyName"].as_str().unwrap_or_default().to_string();
                    let own = named
                        .strip_prefix(&format!("{plan}/"))
                        .unwrap_or(&named)
                        .to_string();
                    policy["PolicyName"] = json!(own);
                }
                resource
            })
            .collect(),
    )
}

/// The echoed member of one stored plan's single instruction, as `DescribeScalingPlans`
/// reports it.
async fn echoed(emulator: &Emulator, plan: &str, member: &str) -> Option<Value> {
    let body = emulator
        .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [plan] }))
        .await;
    body["ScalingPlans"][0]["ScalingInstructions"][0]
        .get(member)
        .cloned()
}

#[tokio::test]
async fn an_absent_member_is_computed_with_the_default_its_documentation_states() {
    let window = window();

    for member in members() {
        for shape in shapes() {
            let case = format!("{} with {}", shape.label, member.name);
            let emulator = emulator_with(shape, &member).await;

            // Source: `smithy_query(query="shape",shape="ForecastDataType")`, mirrored as
            // `FORECAST_DATA_TYPE_VALUES` — every modelled forecast data type, so a member
            // that moves WHEN a data point is reported and one that moves WHAT it carries
            // are both caught.
            for data_type in FORECAST_DATA_TYPE_VALUES {
                let omitted = points(&emulator, OMITTED_PLAN, RESOURCE, data_type, window).await;
                let explicit = points(&emulator, EXPLICIT_PLAN, RESOURCE, data_type, window).await;
                assert!(
                    !omitted.is_empty(),
                    "case {case:?} / {data_type}: a predictive instruction at its \
                     documented defaults has a forecast over {window:?}, got nothing"
                );
                let omitted_instants: Vec<i64> = omitted.iter().map(|(at, _)| *at).collect();
                let explicit_instants: Vec<i64> = explicit.iter().map(|(at, _)| *at).collect();
                assert_eq!(
                    omitted_instants, explicit_instants,
                    "case {case:?} / {data_type}: the member absent and the member at the \
                     value its documentation states describe the same instruction, so the \
                     same instants are reported, expected {explicit_instants:?}, got \
                     {omitted_instants:?}"
                );
                for (index, (at, value)) in omitted.iter().enumerate() {
                    assert_close(
                        *value,
                        explicit[index].1,
                        &format!("{case:?} / {data_type} at {at}"),
                    );
                }
            }

            // The same holds for everything the service computes about the resource itself,
            // which is where a member with no effect on a forecast shows.
            let omitted = resources(&emulator, OMITTED_PLAN).await;
            let explicit = resources(&emulator, EXPLICIT_PLAN).await;
            assert_eq!(
                omitted, explicit,
                "case {case:?}: the two plans describe the same resource, expected \
                 {explicit}, got {omitted}"
            );

            // …while the stored member is echoed exactly as it was submitted: a default the
            // service COMPUTES with is not a value it writes into the instruction.
            let stored = echoed(&emulator, OMITTED_PLAN, member.name).await;
            assert_eq!(
                stored, None,
                "case {case:?}: an instruction created without the member reports it \
                 absent, got {stored:?}"
            );
            let stored = echoed(&emulator, EXPLICIT_PLAN, member.name).await;
            assert_eq!(
                stored,
                Some(member.documented_default.clone()),
                "case {case:?}: an instruction created with the member reports it back, \
                 expected {:?}, got {stored:?}",
                member.documented_default
            );
        }
    }
}

#[tokio::test]
async fn the_buffer_an_absent_buffer_time_is_read_with_is_the_one_its_documentation_states() {
    // The observable the rule turns on for the member whose default is a quantity rather
    // than a mode: the run time of each scheduled scaling action of an instruction that
    // stores no buffer time sits the documented number of seconds before the forecast
    // interval it provisions for ("if the forecast says to add capacity at 10:00 AM, and
    // the buffer time is 5 minutes, then the run time … will be 9:55 AM"), so it is NOT on
    // the interval boundary. Driven on each scheduled-action data type and each shape.
    let window = window();
    let member = Member {
        name: "ScheduledActionBufferTime",
        documented_default: json!(DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS),
    };
    let into_interval =
        (FORECAST_INTERVAL_SECONDS - DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS) % FORECAST_INTERVAL_SECONDS;

    for shape in shapes() {
        let emulator = emulator_with(shape, &member).await;
        for data_type in ["ScheduledActionMinCapacity", "ScheduledActionMaxCapacity"] {
            let got = points(&emulator, OMITTED_PLAN, RESOURCE, data_type, window).await;
            assert!(
                !got.is_empty(),
                "case {} / {data_type}: expected scheduled-action data points over \
                 {window:?}, got nothing",
                shape.label
            );
            for (at, _) in got {
                let offset = at.rem_euclid(FORECAST_INTERVAL_SECONDS);
                assert_eq!(
                    offset, into_interval,
                    "case {} / {data_type} at {at}: a scheduled scaling action of an \
                     instruction storing no buffer time runs \
                     {DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS} seconds before the interval \
                     it provisions for, so it falls {into_interval} seconds into an \
                     interval, got {offset}",
                    shape.label
                );
            }
        }
    }
}
