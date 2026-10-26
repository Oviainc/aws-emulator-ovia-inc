Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# Enforce the documented ScheduledActionBufferTime bounds (under 3600 seconds, predictive scaling only)

_Filed: 2026-10-25T18:55:21Z_
_Operation: CreateScalingPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_scaling_instructions` is the single semantic gate both write paths run (CreateScalingPlan and UpdateScalingPlan). For `ScalingInstruction$ScheduledActionBufferTime` it checks only that the value is not negative — a bound the model ALREADY enforces with `range {min: 0}`, as the comment above the check itself admits — and skips the two conditions the same documentation paragraph states: (1) "The value must be less than the forecast interval duration of 3600 seconds (60 minutes)", and (2) "Only valid when configuring predictive scaling." A `ScheduledActionBufferTime` of 3600, 86400 or one supplied on an instruction that carries neither `PredefinedLoadMetricSpecification` nor `CustomizedLoadMetricSpecification` is therefore stored and answered 200.

The second condition is the very rule the same function already implements for the sibling member in the same paragraph family: `PredictiveScalingMaxCapacityBehavior` is refused with "only valid when configuring predictive scaling" when no load metric specification is present (state.rs:848-856), and the `predictive` flag it decides on is already computed at state.rs:835 and in scope where the buffer-time check runs. The forecast-interval figure is likewise already a named constant in this crate (`FORECAST_INTERVAL_SECONDS = 3_600`), so neither check needs a new constant.

The same omission applies to `ScalingPolicyUpdateBehavior`, whose doc ends "Only valid when configuring dynamic scaling" while `DisableDynamicScaling` is documented "Controls whether dynamic scaling by AWS Auto Scaling is disabled... The default is enabled (false)": the handler validates only that the value is in the enum (state.rs:614-620), so `ScalingPolicyUpdateBehavior` supplied together with `DisableDynamicScaling: true` is accepted. All three are symptoms of the same class on the same code path, so they belong in one fix.

## Evidence

Smithy, quoted verbatim — `smithy_query(query="doc",shape="ScalingInstruction",member="ScheduledActionBufferTime")`: "<p>The value must be less than the forecast interval duration of 3600 seconds (60 minutes).\n         The default is 300 seconds. </p>\n         <p>Only valid when configuring predictive scaling. </p>"; `smithy_query(query="shape",shape="ScalingInstruction")` gives `"ScheduledActionBufferTime": {"target": "ScheduledActionBufferTime", "range": {"min": 0}, ...}` — the only modelled constraint is the min of 0 the handler re-checks.

The only check the crate applies — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:602-613:
```
    // "The amount of time, in seconds, to buffer the run time of scheduled scaling
    // actions": the model already bounds this at 0, which is checked again here so the
    // same refusal holds however the value reaches the handler.
    if instruction
        .scheduled_action_buffer_time
        .is_some_and(|seconds| seconds < 0)
    {
        return Err(format!(
            "{}: a time in seconds cannot be negative",
            at("ScheduledActionBufferTime")
        ));
    }
```

The sibling rule the SAME function does enforce — state.rs:835 and state.rs:842-856:
```
    let predictive = predefined.is_some() || customized.is_some();
...
        // "Only valid when configuring predictive scaling."
        if !predictive {
            return Err(format!(
                "{}: only valid when configuring predictive scaling, which requires a load \
                 metric specification",
                at("PredictiveScalingMaxCapacityBehavior")
            ));
        }
```

The 3600 figure already exists in this crate — src/ops/get_scaling_plan_resource_forecast_data.rs:409-415: "/// The granularity of forecast data, in seconds. ... pub const FORECAST_INTERVAL_SECONDS: i64 = 3_600;".

The enum-only check for the dynamic-scaling member — state.rs:614-620: `if let Some(behavior) = instruction.scaling_policy_update_behavior.as_deref() { one_of(&at("ScalingPolicyUpdateBehavior"), behavior, SCALING_POLICY_UPDATE_BEHAVIOR_VALUES)?; }`, against `smithy_query(query="shape",shape="ScalingInstruction")` member doc "<p>Only valid when configuring dynamic scaling. </p>" and `DisableDynamicScaling`: "<p>The default is enabled (<code>false</code>). </p>".

Test coverage (and why a fix leaves it green): tests/all/scaling_instruction_validation.rs:470-490 is the only accepted row that sets `("ScheduledActionBufferTime", json!(0))` and it also sets `PredefinedLoadMetricSpecification`, i.e. predictive scaling is configured and the value is below 3600; tests/all/scaling_plan_lifecycle.rs:319-343 ("stack source with every optional instruction member set") uses `"ScheduledActionBufferTime": 600` with a `PredefinedLoadMetricSpecification` and `"DisableDynamicScaling": false`. No test supplies a buffer time of 3600 or more, or one without a load metric specification, so these cases are unpinned.

## Steps to Reproduce

1. Create a scaling plan whose single ScalingInstruction is valid (autoscaling / autoScalingGroup/my-asg / autoscaling:autoScalingGroup:DesiredCapacity, MinCapacity 1, MaxCapacity 10, one TargetTrackingConfiguration) and ALSO sets "ScheduledActionBufferTime": 86400 while carrying no PredefinedLoadMetricSpecification and no CustomizedLoadMetricSpecification.
2. Observe HTTP 200 with ScalingPlanVersion 1 instead of a 400 ValidationException, although the member is documented as "must be less than the forecast interval duration of 3600 seconds" and "Only valid when configuring predictive scaling".
3. DescribeScalingPlans the created plan: the instruction is stored with the out-of-range buffer time.
4. For contrast, send the same instruction with "PredictiveScalingMaxCapacityBehavior": "SetMaxCapacityToForecastCapacity" and no load metric specification: that member IS refused 400 with "only valid when configuring predictive scaling, which requires a load metric specification" (state.rs:848-856).
5. Repeat step 1 through UpdateScalingPlan on an Active plan — the same helper is called (update_scaling_plan.rs:176-178), so the same input is accepted there too.
