Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ac318820ce0cc5f19770024ed132bf63c4007170

# Forecast capacity leaves the instruction's MinCapacity/MaxCapacity band, so ScheduledActionMinCapacity is reported above ScheduledActionMaxCapacity

_Filed: 2026-10-27T12:43:51Z_
_Operation: GetScalingPlanResourceForecastData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ac318820ce0cc5f19770024ed132bf63c4007170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The forecast is built as a fixed daily cycle of ±50% (`DIURNAL_AMPLITUDE = 0.5`) around the MIDPOINT of the instruction's capacity band (`(MinCapacity + MaxCapacity) / 2`). The doc comment directly above that midpoint asserts the resulting forecast stays inside the band ("so the load it forecasts is one the instruction's own `MinCapacity` and `MaxCapacity` bracket"), but the arithmetic breaks that claim for every instruction whose `MaxCapacity` is less than three times its `MinCapacity`:

* peak (hour 12 UTC, factor 1.5): `capacity = ceil(0.75 * (Min + Max))` — for Min=6, Max=10 that is 12, i.e. 2 units ABOVE MaxCapacity;
* trough (hour 0 UTC, factor 0.5): `capacity = ceil(0.25 * (Min + Max))` — for Min=6, Max=10 that is 4, i.e. 2 units BELOW MinCapacity.

Because the two scheduled-action data types are derived from that same unclamped capacity, one read of the same resource at the same instant answers a scheduled-action MINIMUM capacity that is larger than the scheduled-action MAXIMUM capacity: `ScheduledActionMinCapacity = max(capacity, MinCapacity) = 12` while `ScheduledActionMaxCapacity` under the default `PredictiveScalingMaxCapacityBehavior` returns `MaxCapacity = 10` — the behaviour whose documented meaning the same file quotes as "the maximum capacity is enforced as a hard limit". A caller that reads all four `ForecastDataType`s for one hour therefore gets a self-contradictory answer (min 12 > max 10) for an entirely ordinary instruction, and a `CapacityForecast` the plan's own band forbids.

This is a different gap from the two already-fixed wrong-behavior issues on this operation (one was "always answers an empty list", the other the sub-second `EndTime` data point).

## Evidence

The claim the code breaks — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:666-671:
```
    /// The capacity the forecast cycles around: the middle of the band the instruction
    /// declares, so the load it forecasts is one the instruction's own `MinCapacity` and
    /// `MaxCapacity` bracket.
    fn baseline_capacity(&self) -> f64 {
        (self.min_capacity + self.max_capacity) / 2.0
    }
```

The cycle that leaves that bracket — same file:444 and :461-464:
```
const DIURNAL_AMPLITUDE: f64 = 0.5;
...
fn diurnal_factor(hour: i64) -> f64 {
    let phase = (hour as f64 - PEAK_HOUR_UTC) / HOURS_PER_DAY as f64 * ::std::f64::consts::TAU;
    1.0 + DIURNAL_AMPLITUDE * phase.cos()
}
```
so `factor` spans [0.5, 1.5], and nothing clamps the product — same file:673-682:
```
    fn load(&self, hour: i64) -> f64 {
        self.baseline_capacity() * self.load_per_unit * diurnal_factor(hour)
    }

    /// The capacity forecast for `hour`: the whole units that hold every target tracking
    /// configuration at its target under the forecast load.
    fn capacity(&self, hour: i64) -> f64 {
        whole_units(self.load(hour) / self.load_per_unit)
    }
```
(`capacity(hour)` reduces to `whole_units(baseline * factor)`; `load_per_unit` cancels.)

The two scheduled-action values that then contradict each other — same file:684-710:
```
    /// "The minimum capacity for each scheduled scaling action … calculated as the larger
    /// of two values: the capacity forecast or the minimum capacity in the scaling
    /// instruction."
    fn scheduled_action_min_capacity(&self, hour: i64) -> f64 {
        self.capacity(hour).max(self.min_capacity)
    }

    /// ...
    /// * `SetForecastCapacityToMaxCapacity`, the documented default — "the maximum
    ///   capacity is enforced as a hard limit".
    fn scheduled_action_max_capacity(&self, hour: i64) -> f64 {
        match self.max_capacity_behavior.as_deref() {
            Some(BEHAVIOR_TO_FORECAST_CAPACITY) => self.max_capacity.max(self.capacity(hour)),
            Some(crate::state::BEHAVIOR_ABOVE_FORECAST_CAPACITY) => self
                .max_capacity
                .max(whole_units(self.capacity(hour) * (1.0 + self.max_capacity_buffer))),
            _ => self.max_capacity,
        }
    }
```
The model sentence the comment quotes (`smithy_query(query="shape",shape="ScalingInstruction")`, member `PredictiveScalingMaxCapacityBehavior`): "`SetForecastCapacityToMaxCapacity` - AWS Auto Scaling cannot scale resource capacity higher than the maximum capacity. The maximum capacity is enforced as a hard limit." and `MaxCapacity`: "The maximum capacity of the resource. The exception to this upper limit is if you specify a non-default setting for PredictiveScalingMaxCapacityBehavior." — here no non-default setting is given, yet the reported scheduled-action minimum exceeds the limit.

The chosen-amplitude comment (same file:433-443) justifies only that a CYCLE is chosen ("CHOSEN, not documented… a daily cycle of this amplitude around the baseline load the instruction describes"); it says nothing about values being allowed to leave the band the sibling comment at :666-668 promises.

Test coverage: nothing pins the current out-of-band values. tests/all/forecast_datapoints.rs:692-698 already uses a fixture in the broken regime —
```
        (
            "MinCapacity",
            Case {
                min_capacity: 6,
                ..base.clone()
            },
        ),
```
(with `max_capacity: 10` from `base` at :674-683, so Max < 3 × Min) — but that test only asserts the LoadForecast values differ from the baseline (:726-733). The per-point assertions at tests/all/forecast_datapoints.rs:530-551 re-derive both scheduled-action values from the response's OWN capacity value (`capacity_value.max(case.min_capacity as f64)` / `case.scheduled_action_max_capacity(capacity_value)`), so no assertion requires the capacity forecast to exceed MaxCapacity.

## Steps to Reproduce

1. CreateScalingPlan with one scaling instruction for `autoscaling` / `autoScalingGroup/my-asg` / `autoscaling:autoScalingGroup:DesiredCapacity`, MinCapacity 6, MaxCapacity 10, one TargetTrackingConfiguration (ASGAverageCPUUtilization, TargetValue 40), and PredefinedLoadMetricSpecification ASGTotalCPUUtilization (so predictive scaling is configured). Leave PredictiveScalingMaxCapacityBehavior unset (the documented default).
2. Wait for the plan to settle to Active.
3. Call GetScalingPlanResourceForecastData for that resource with ForecastDataType=CapacityForecast over a window covering 12:00 UTC; observe the data point value 12, which is above the instruction's MaxCapacity of 10 (baseline (6+10)/2 = 8, peak factor 1.5 → ceil(12)).
4. Call it again with ForecastDataType=ScheduledActionMinCapacity for the same window: the 12:00 UTC point is 12.
5. Call it again with ForecastDataType=ScheduledActionMaxCapacity for the same window: the 12:00 UTC point is 10 (get_scaling_plan_resource_forecast_data.rs:708, the default-behaviour arm). The minimum capacity reported for that scheduled action (12) exceeds the maximum reported for the same action (10).
6. Repeat step 3 for a window covering 00:00 UTC: the CapacityForecast point is 4, below the instruction's MinCapacity of 6 — the bracket the comment at :666-668 claims is broken on both sides.
