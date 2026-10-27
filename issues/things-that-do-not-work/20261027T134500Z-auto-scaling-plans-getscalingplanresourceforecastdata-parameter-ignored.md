Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 9c3b4064a1735206e464a3fb30963f5d1ed2eddb

# ScheduledActionBufferTime never moves the scheduled-action forecast data points, so it affects no response

_Filed: 2026-10-27T13:45:00Z_
_Operation: GetScalingPlanResourceForecastData | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `9c3b4064a1735206e464a3fb30963f5d1ed2eddb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ScalingInstruction$ScheduledActionBufferTime` is documented as shifting the run time of each scheduled scaling action earlier by that many seconds, and the two scheduled-action `ForecastDataType`s are documented as reporting a capacity "for each scheduled scaling action". The forecast handler computes the instants of ALL FOUR forecast data types from one shared helper (`forecast_instants`) that takes only the request window and the serving clock — it never looks at the stored instruction — so `ScheduledActionMinCapacity` and `ScheduledActionMaxCapacity` data points are emitted at exactly the same whole-hour boundaries as `LoadForecast`/`CapacityForecast`, whatever buffer time the plan was created with.

The consequence is that `ScheduledActionBufferTime` influences nothing a caller can observe other than being echoed back by `DescribeScalingPlans`: a plan created with `ScheduledActionBufferTime: 600` and an otherwise identical plan created without the member return byte-identical `Datapoints` for both scheduled-action data types. The crate reads the member in exactly one place — its own validation — and in no computation. The validation it does apply is itself derived from the same paragraph ("The value must be less than the forecast interval duration of 3600 seconds (60 minutes)"), i.e. the crate already accepts that the buffer is a sub-period shift of the scheduled action, but then never applies the shift.

## Evidence

The model sentence that states the behaviour (`smithy_query(query="doc",shape="ScalingInstruction",member="ScheduledActionBufferTime")`):

> "The amount of time, in seconds, to buffer the run time of scheduled scaling actions when scaling out. For example, if the forecast says to add capacity at 10:00 AM, and the buffer time is 5 minutes, then the run time of the corresponding scheduled scaling action will be 9:55 AM. ... The value must be less than the forecast interval duration of 3600 seconds (60 minutes)."

and what a scheduled-action data point is (`smithy_query(query="doc",shape="GetScalingPlanResourceForecastDataRequest",member="ForecastDataType")`):

> "`ScheduledActionMinCapacity`: The minimum capacity for each scheduled scaling action. ... `ScheduledActionMaxCapacity`: The maximum capacity for each scheduled scaling action."

with `Datapoint$Timestamp` (`smithy_query(query="shape",shape="Datapoint")`): "The time stamp for the data point in UTC format."

The handler derives every kind's instants from the window alone — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:729-752:
```
fn forecast(
    instruction: &ScalingInstruction,
    data_type: &str,
    start_time: ::aws_smithy_types::DateTime,
    end_time: ::aws_smithy_types::DateTime,
    now: ::std::time::SystemTime,
) -> Vec<Datapoint> {
    let Some(kind) = ForecastKind::of(data_type) else { ... };
    let Some(forecast) = ResourceForecast::of(instruction) else { ... };
    if kind.is_scheduled_action() && !forecast.creates_scheduled_actions {
        return Vec::new();
    }
    forecast_instants(start_time, end_time, now)
        .into_iter()
        .map(|at| Datapoint {
            timestamp: Some(::aws_smithy_types::DateTime::from_secs(at)),
            value: Some(forecast.value_at(kind, hour_of_day(at))),
        })
        .collect()
}
```
`forecast_instants` (same file:508-532) takes `(start_time, end_time, now)` only — the instruction is not a parameter, so no per-instruction offset can reach a timestamp. `ResourceForecast` (same file:606-664) carries `min_capacity`, `max_capacity`, `load_per_unit`, `max_capacity_behavior`, `max_capacity_buffer`, `creates_scheduled_actions` — and no buffer TIME.

The only read of the member in the whole crate is its validation — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:996-1014:
```
    if let Some(seconds) = instruction.scheduled_action_buffer_time {
        let member = at("ScheduledActionBufferTime");
        ...
        // "The value must be less than the forecast interval duration of 3600 seconds
        // (60 minutes)."
        if i64::from(seconds) >= FORECAST_INTERVAL_SECONDS {
```
(grep for `scheduled_action_buffer_time` across `src/` returns only `shared_types.rs:39` (the field) and `state.rs:996-997`.)

Test coverage: no test drives a forecast for an instruction carrying `ScheduledActionBufferTime` — grep for `ScheduledActionBufferTime` in `tests/` matches only the lifecycle round-trip (tests/all/scaling_plan_lifecycle.rs:341) and the validation suites (tests/all/scaling_mode_conditional_members.rs, tests/all/scaling_instruction_validation.rs:491, tests/all/predictive_scaling_predicate.rs:113); tests/all/forecast_datapoints.rs sets it on no case, so its assertion that the scheduled-action points "cover the same intervals" (tests/all/forecast_datapoints.rs:486-501 and :531-546) pins only the zero/absent-buffer case.

## Steps to Reproduce

1. CreateScalingPlan "buffered" with one scaling instruction for `autoscaling` / `autoScalingGroup/my-asg` / `autoscaling:autoScalingGroup:DesiredCapacity`, MinCapacity 1, MaxCapacity 10, one TargetTrackingConfiguration (ASGAverageCPUUtilization, TargetValue 40), PredefinedLoadMetricSpecification ASGTotalCPUUtilization (so predictive scaling is configured), and `ScheduledActionBufferTime: 600`.
2. CreateScalingPlan "unbuffered" with the identical instruction but no `ScheduledActionBufferTime` (and its own ApplicationSource, since one plan is allowed per source).
3. Let both plans settle to Active.
4. Call GetScalingPlanResourceForecastData on "buffered" with ForecastDataType=ScheduledActionMinCapacity over an hour-aligned window; note every `Datapoints[*].Timestamp` sits exactly on an hour boundary (HH:00:00).
5. Call the same request against "unbuffered"; the `Datapoints` are identical, timestamps included — the 600-second buffer moved no scheduled action's reported run time (expected HH:50:00 per the documented 10:00 AM -> 9:55 AM example).
6. Confirm the member is otherwise inert: DescribeScalingPlans echoes `ScheduledActionBufferTime: 600`, and no other response differs between the two plans.
