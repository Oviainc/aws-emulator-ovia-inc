Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# An absent ScheduledActionBufferTime buffers nothing instead of the documented 300-second default, while the two sibling predictive-scaling members DO get their documented defaults

_Filed: 2026-10-28T15:46:58Z_
_Operation: GetScalingPlanResourceForecastData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ScalingInstruction$ScheduledActionBufferTime` documents a default: "The value must be less than the forecast interval duration of 3600 seconds (60 minutes). The default is 300 seconds." The forecast handler instead treats an omitted member as a ZERO buffer (`map_or(0, i64::from)`), so for a plan whose instruction does not carry the member the two scheduled-action forecast data types (`ScheduledActionMinCapacity`, `ScheduledActionMaxCapacity`) are stamped on the forecast interval boundaries themselves rather than 300 seconds before them — the run time the same paragraph describes ("if the forecast says to add capacity at 10:00 AM, and the buffer time is 5 minutes, then the run time of the corresponding scheduled scaling action will be 9:55 AM").

This is also inconsistent inside the very same constructor: the two neighbouring predictive-scaling members ARE given their documented defaults when absent. `creates_scheduled_actions` applies `PredictiveScalingMode`'s documented default (`ForecastAndScale`) to an absent member, and `scheduled_action_max_capacity`'s fall-through `_ =>` branch applies `PredictiveScalingMaxCapacityBehavior`'s documented default (`SetForecastCapacityToMaxCapacity`, "the maximum capacity is enforced as a hard limit") to an absent member. Only the buffer time's documented default is dropped.

The code's own justification for the divergence does not follow from the model: "the member is held and reported back exactly as it was stored, so an instruction carrying none buffers none" — what `DescribeScalingPlans` echoes for the member says nothing about which buffer the service USES when the member is absent, and the model sentence states that value outright. The consequence is that an instruction created without the member and one created with `ScheduledActionBufferTime: 300` — which the model says are the same instruction — produce different `Datapoints` timestamps.

## Evidence

Model sentence that states the behaviour (`smithy_query(query="doc",shape="ScalingInstruction",member="ScheduledActionBufferTime")`):

> "The amount of time, in seconds, to buffer the run time of scheduled scaling actions when scaling out. For example, if the forecast says to add capacity at 10:00 AM, and the buffer time is 5 minutes, then the run time of the corresponding scheduled scaling action will be 9:55 AM. ... The value must be less than the forecast interval duration of 3600 seconds (60 minutes). **The default is 300 seconds.** Only valid when configuring predictive scaling."

The handler drops that default — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:687-691:
```
            creates_scheduled_actions: instruction.predictive_scaling_mode.as_deref()
                != Some(MODE_FORECAST_ONLY),
            scheduled_action_buffer_seconds: instruction
                .scheduled_action_buffer_time
                .map_or(0, i64::from),
```
and states the divergence in its own field doc — same file:647-650:
```
    /// capacities are reported at. Zero where the instruction carries no buffer time: the
    /// member is held and reported back exactly as it was stored, so an instruction
    /// carrying none buffers none.
    scheduled_action_buffer_seconds: i64,
```

The two SIBLING members in the same constructor do honour their documented defaults — same file:687-688 (`PredictiveScalingMode` "The default value is ForecastAndScale") and same file:755-761:
```
    fn scheduled_action_max_capacity(&self, hour: i64) -> f64 {
        match self.max_capacity_behavior.as_deref() {
            Some(BEHAVIOR_TO_FORECAST_CAPACITY) => self.max_capacity.max(self.capacity(hour)),
            Some(crate::state::BEHAVIOR_ABOVE_FORECAST_CAPACITY) => self
                .max_capacity
                .max(whole_units(self.capacity(hour) * (1.0 + self.max_capacity_buffer))),
            _ => self.max_capacity,
        }
```
whose own doc names that fall-through as "`SetForecastCapacityToMaxCapacity`, the documented default" (same file:752-754).

Test coverage: tests/all/scheduled_action_buffer_time.rs drives `None` as one of its buffer rows (line 117) and pins the zero-shift reading with the same non-sequitur reason — tests/all/scheduled_action_buffer_time.rs:263-265:
```
            // An absent member buffers nothing: the instruction is reported back exactly as
            // it was stored, so it carries no buffer time to shift anything by.
            let seconds = buffer.unwrap_or(0);
```
That stated reason is exactly what the quoted model sentence contradicts. The fixed issue 20261027T134500Z-auto-scaling-plans-getscalingplanresourceforecastdata-parameter-ignored made a PRESENT buffer time shift the data points; it quoted only the 3600-second bound and never the default, so the absent-member case is not covered by it.

## Steps to Reproduce

1. CreateScalingPlan "defaulted" with one scaling instruction for ServiceNamespace `autoscaling`, ResourceId `autoScalingGroup/my-asg`, ScalableDimension `autoscaling:autoScalingGroup:DesiredCapacity`, MinCapacity 2, MaxCapacity 10, one TargetTrackingConfiguration (ASGAverageCPUUtilization, TargetValue 40), PredefinedLoadMetricSpecification ASGTotalCPUUtilization (so predictive scaling is configured) and NO `ScheduledActionBufferTime`.
2. CreateScalingPlan "explicit" with the identical instruction plus `ScheduledActionBufferTime: 300` (and its own ApplicationSource, since one plan is allowed per source).
3. Let both plans settle to Active.
4. Call GetScalingPlanResourceForecastData on "defaulted" with ForecastDataType=ScheduledActionMinCapacity over an hour-aligned window inside the last 56 days.
5. Observe every `Datapoints[*].Timestamp` sits exactly on an hour boundary (HH:00:00).
6. Call the same request against "explicit": its timestamps sit at HH:55:00, 300 seconds earlier.
7. Per "The default is 300 seconds", the two plans describe the same instruction, so both answers should carry the HH:55:00 run times; instead the defaulted plan reports an unbuffered scheduled action.
