// SPDX-License-Identifier: CC0-1.0

//! One rule, asserted over a table rather than over a single request.
//!
//! A `ScalingInstruction` member whose documentation makes it valid only when a given
//! scaling mode is configured is refused (HTTP 400 `ValidationException`) on an
//! instruction that does not configure that mode, and a member whose documentation
//! states a bound is refused outside it:
//!
//! * predictive scaling is configured by exactly one load metric specification
//!   (`PredefinedLoadMetricSpecification` / `CustomizedLoadMetricSpecification`), so
//!   `ScheduledActionBufferTime` and the two `PredictiveScalingMaxCapacity…` members
//!   need one;
//! * dynamic scaling is on unless `DisableDynamicScaling` is `true` ("The default is
//!   enabled (`false`)"), so `ScalingPolicyUpdateBehavior` needs it on;
//! * `ScheduledActionBufferTime` "must be less than the forecast interval duration of
//!   3600 seconds (60 minutes)", above the modelled minimum of 0.
//!
//! Both write paths take the same instruction list, so each refused row is driven through
//! `CreateScalingPlan` AND `UpdateScalingPlan`; each accepted row is read back through
//! `DescribeScalingPlans`.
//!
//! The harness (`Emulator`, `instruction`, `stack_source`) and the instruction builders
//! (`valid_instruction`, `with`) are the sibling suites'; nothing here builds a second
//! one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::SCALING_POLICY_UPDATE_BEHAVIOR_VALUES;

use crate::scaling_instruction_validation::with;
use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// A predefined load metric specification — one of the two ways to configure predictive
/// scaling. The metric type comes from the model's own `LoadMetricType` value list
/// (`smithy_query(query="shape",shape="LoadMetricType")`, mirrored as
/// `LOAD_METRIC_TYPE_VALUES`).
fn predefined_load() -> Value {
    json!({ "PredefinedLoadMetricType": "ASGTotalNetworkIn" })
}

/// The other way: a customized load metric, whose statistic is documented as only `Sum`.
fn customized_load() -> Value {
    json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": "Sum" })
}

/// The documented default of `ScheduledActionBufferTime`: "The default is 300 seconds."
const DEFAULT_BUFFER_TIME: i64 = 300;

/// One refused row: its label, the instruction list it sends, and a fragment the refusal
/// must carry so the sentence names the member actually at fault.
struct Refused {
    label: String,
    instructions: Value,
    message_fragment: String,
}

fn refused(label: String, instructions: Value, message_fragment: String) -> Refused {
    Refused {
        label,
        instructions,
        message_fragment,
    }
}

/// The wording of the refusal for a member the instruction's modes do not allow.
fn predictive_only_fragment(member: &str) -> String {
    format!("{member}: only valid when configuring predictive scaling")
}

fn dynamic_only_fragment(member: &str) -> String {
    format!("{member}: only valid when configuring dynamic scaling")
}

/// Instruction lists whose conditional members contradict the modes they configure, or
/// whose value is outside the bound documented for it.
fn refused_rows() -> Vec<Refused> {
    let bound_fragment = format!(
        "less than the forecast interval duration of {FORECAST_INTERVAL_SECONDS} seconds"
    );
    let buffer_member = "ScalingInstructions[0].ScheduledActionBufferTime";
    let behavior_member = "ScalingInstructions[0].PredictiveScalingMaxCapacityBehavior";
    let update_behavior_member = "ScalingInstructions[0].ScalingPolicyUpdateBehavior";

    let mut rows = Vec::new();

    // At and past the forecast interval duration, with predictive scaling configured so
    // only the bound can refuse the row. The values span the interval itself, one past
    // it, a whole day, and the largest value the modelled integer carries.
    for (label, seconds, load) in [
        ("the forecast interval itself", FORECAST_INTERVAL_SECONDS, predefined_load()),
        ("one second past the interval", FORECAST_INTERVAL_SECONDS + 1, predefined_load()),
        ("a whole day of buffer time", 86_400, customized_load()),
        ("the largest modelled integer", i64::from(i32::MAX), predefined_load()),
    ] {
        let member = if load.get("MetricName").is_some() {
            "CustomizedLoadMetricSpecification"
        } else {
            "PredefinedLoadMetricSpecification"
        };
        rows.push(refused(
            format!("buffer time {seconds}: {label}"),
            json!([with(&[
                (member, load),
                ("ScheduledActionBufferTime", json!(seconds)),
            ])]),
            bound_fragment.clone(),
        ));
    }

    // Below the modelled minimum of 0 (`range {min: 0}`), which is refused by the
    // modelled constraint, in that constraint's own wording.
    rows.push(refused(
        "buffer time one below the modelled minimum".to_string(),
        json!([with(&[
            ("PredefinedLoadMetricSpecification", predefined_load()),
            ("ScheduledActionBufferTime", json!(-1)),
        ])]),
        "failed to satisfy constraint: Member must have value greater than or equal to 0"
            .to_string(),
    ));

    // In range, but on an instruction that configures no predictive scaling at all: the
    // values span the modelled minimum, the documented default and the top of the range.
    for seconds in [0, DEFAULT_BUFFER_TIME, FORECAST_INTERVAL_SECONDS - 1] {
        rows.push(refused(
            format!("buffer time {seconds} without a load metric specification"),
            json!([with(&[("ScheduledActionBufferTime", json!(seconds))])]),
            predictive_only_fragment(buffer_member),
        ));
    }

    // The sibling predictive-scaling members, over the same rule: each is refused
    // without a load metric specification, whatever its own value.
    rows.push(refused(
        "a max-capacity behavior without a load metric specification".to_string(),
        json!([with(&[(
            "PredictiveScalingMaxCapacityBehavior",
            json!("SetMaxCapacityToForecastCapacity")
        )])]),
        predictive_only_fragment(behavior_member),
    ));

    // Dynamic scaling turned off, with the member that is only valid while it is on —
    // one row per modelled enum value, so the refusal is about the mode and not the value.
    for behavior in SCALING_POLICY_UPDATE_BEHAVIOR_VALUES {
        rows.push(refused(
            format!("{behavior} while dynamic scaling is disabled"),
            json!([with(&[
                ("ScalingPolicyUpdateBehavior", json!(behavior)),
                ("DisableDynamicScaling", json!(true)),
            ])]),
            dynamic_only_fragment(update_behavior_member),
        ));
    }

    // The two conditions on different members at once: the first one the handler reaches
    // still names a member actually at fault.
    rows.push(refused(
        "a disabled-dynamic update behavior beside an out-of-range buffer time".to_string(),
        json!([with(&[
            ("PredefinedLoadMetricSpecification", predefined_load()),
            ("ScheduledActionBufferTime", json!(FORECAST_INTERVAL_SECONDS)),
            ("ScalingPolicyUpdateBehavior", json!("ReplaceExternalPolicies")),
            ("DisableDynamicScaling", json!(true)),
        ])]),
        bound_fragment,
    ));

    rows
}

/// Instruction lists whose conditional members match the modes they configure, each of
/// which AWS applies.
fn accepted_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();

    // A buffer time inside the documented range, on an instruction that configures
    // predictive scaling: the modelled minimum, one second, the documented default and
    // one below the forecast interval, through both load metric specifications.
    for (seconds, load, member) in [
        (0, predefined_load(), "PredefinedLoadMetricSpecification"),
        (1, customized_load(), "CustomizedLoadMetricSpecification"),
        (
            DEFAULT_BUFFER_TIME,
            predefined_load(),
            "PredefinedLoadMetricSpecification",
        ),
        (
            FORECAST_INTERVAL_SECONDS - 1,
            customized_load(),
            "CustomizedLoadMetricSpecification",
        ),
    ] {
        rows.push((
            format!("buffer time {seconds} with a {member}"),
            json!([with(&[
                (member, load),
                ("ScheduledActionBufferTime", json!(seconds)),
            ])]),
        ));
    }

    // Predictive scaling and dynamic scaling are independent modes: a buffer time is
    // still valid while dynamic scaling is off.
    rows.push((
        "a buffer time while dynamic scaling is disabled".to_string(),
        json!([with(&[
            ("PredefinedLoadMetricSpecification", predefined_load()),
            ("ScheduledActionBufferTime", json!(DEFAULT_BUFFER_TIME)),
            ("DisableDynamicScaling", json!(true)),
        ])]),
    ));

    // The dynamic-scaling member, once per modelled value, with dynamic scaling left on
    // both ways: explicitly and by its documented default.
    for behavior in SCALING_POLICY_UPDATE_BEHAVIOR_VALUES {
        rows.push((
            format!("{behavior} with dynamic scaling explicitly enabled"),
            json!([with(&[
                ("ScalingPolicyUpdateBehavior", json!(behavior)),
                ("DisableDynamicScaling", json!(false)),
            ])]),
        ));
        rows.push((
            format!("{behavior} with dynamic scaling left at its default"),
            json!([with(&[("ScalingPolicyUpdateBehavior", json!(behavior))])]),
        ));
    }

    // Dynamic scaling off with no member that depends on it, and no conditional member
    // at all: the controls that show the rule refuses a combination, not a member.
    rows.push((
        "dynamic scaling disabled on its own".to_string(),
        json!([with(&[("DisableDynamicScaling", json!(true))])]),
    ));
    rows.push((
        "no conditional member at all".to_string(),
        json!(simple_instructions()),
    ));

    rows
}

#[tokio::test]
async fn create_refuses_a_conditional_member_the_instructions_modes_do_not_allow() {
    for row in refused_rows() {
        let label = &row.label;
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": row.instructions,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let actual = error_message(&body);
        assert!(
            actual.contains(&row.message_fragment),
            "case {label:?}: expected a message containing {:?}, got {actual:?}",
            row.message_fragment
        );

        // A refused create stores nothing.
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": ["my-plan"] }))
            .await;
        assert_eq!(
            described.get("ScalingPlans"),
            Some(&json!([])),
            "case {label:?}: a refused create must store no plan, got {described}"
        );
    }
}

#[tokio::test]
async fn update_refuses_the_same_conditional_members_create_refuses() {
    for row in refused_rows() {
        let label = &row.label;
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        emulator.make_all_due();

        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": "my-plan",
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": row.instructions,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let actual = error_message(&body);
        assert!(
            actual.contains(&row.message_fragment),
            "case {label:?}: expected a message containing {:?}, got {actual:?}",
            row.message_fragment
        );

        // The stored plan still carries what the create wrote.
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": ["my-plan"] }))
            .await;
        assert_eq!(
            described["ScalingPlans"][0].get("ScalingInstructions"),
            Some(&json!(simple_instructions())),
            "case {label:?}: a refused update must leave the stored instructions alone, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn conditional_members_matching_their_modes_are_created_and_read_back() {
    for (label, instructions) in accepted_rows() {
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
        assert_eq!(
            created.get("ScalingPlanVersion"),
            Some(&json!(1)),
            "case {label:?}: expected version 1, got {created}"
        );

        // The round trip: what the create accepted is what a read reports.
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": ["my-plan"] }))
            .await;
        let plan = &described["ScalingPlans"][0];
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&instructions),
            "case {label:?}: the instructions did not round-trip, got {plan}"
        );

        // The same list is accepted by the other write path too.
        emulator.make_all_due();
        emulator
            .ok(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
    }
}
