Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# Compute forecast Datapoints from the stored scaling plan instead of always answering an empty list

_Filed: 2026-10-24T06:54:30Z_
_Operation: GetScalingPlanResourceForecastData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler answers every accepted request with `Datapoints: []`. It never looks the named plan up: the stored plan, its predictive-scaling load metric specification, its MinCapacity/MaxCapacity and the requested `ForecastDataType` have no influence whatsoever on the response, so the 200 is independent of state.

Expected (real service): for a plan whose scaling instruction configures predictive scaling, the response carries forecast data points over the requested `[StartTime, EndTime)` window at the forecast interval (one point per hour), with values that differ per `ForecastDataType` — `LoadForecast` (forecast load metric), `CapacityForecast` (forecast capacity), `ScheduledActionMinCapacity` (documented as "the larger of two values: the capacity forecast or the minimum capacity in the scaling instruction") and `ScheduledActionMaxCapacity` (determined by `PredictiveScalingMaxCapacityBehavior`/`PredictiveScalingMaxCapacityBuffer`). In particular `ScheduledActionMinCapacity`/`ScheduledActionMaxCapacity` are derivable from the stored instruction alone and are never empty for an active predictive-scaling plan. A request naming a plan this service does not hold is also indistinguishable today from one naming a stored plan.

Actual (emulator): `let datapoints: Vec<Datapoint> = Vec::new();` — a canned answer; the state lock is taken only to settle the clock, and the plan is deliberately not consulted.

## Evidence

Smithy model: `GetScalingPlanResourceForecastDataResponse$Datapoints` is required, `Datapoint{Timestamp, Value}`; `ForecastDataType` documents the four kinds including "ScheduledActionMinCapacity: ... calculated as the larger of two values: the capacity forecast or the minimum capacity in the scaling instruction" (OPERATIONS.md line 53 / smithy_query doc). Handler: src/ops/get_scaling_plan_resource_forecast_data.rs lines ~212-232 return an empty vector with a comment that no plan is looked up. Tests (tests/all/forecast_data.rs: `every_modelled_forecast_data_type_reports_an_empty_forecast`) assert the empty list, i.e. they lock in the canned answer rather than any state-derived value.

## Steps to Reproduce

1. CreateScalingPlan with one instruction for `autoscaling:autoScalingGroup:DesiredCapacity` / `autoScalingGroup/my-asg`, MinCapacity 2, MaxCapacity 10, a PredefinedLoadMetricSpecification {PredefinedLoadMetricType:"ASGTotalCPUUtilization"} and PredictiveScalingMode `ForecastAndScale`.
2. Let the plan settle to `Active`.
3. Call GetScalingPlanResourceForecastData with ScalingPlanName/Version=1, ServiceNamespace `autoscaling`, ResourceId `autoScalingGroup/my-asg`, ScalableDimension `autoscaling:autoScalingGroup:DesiredCapacity`, ForecastDataType `ScheduledActionMinCapacity`, and a 2-day window.
4. Observe `{"Datapoints":[]}`; repeat with ForecastDataType `LoadForecast` and with a plan name that was never created — all three answers are byte-identical, proving nothing is computed from state.
