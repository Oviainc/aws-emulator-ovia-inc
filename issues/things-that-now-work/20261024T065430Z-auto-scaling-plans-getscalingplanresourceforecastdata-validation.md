Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# Refuse forecast requests outside the documented 56-day history horizon and with ScalingPlanVersion other than 1

_Filed: 2026-10-24T06:54:30Z_
_Operation: GetScalingPlanResourceForecastData | Class: validation_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two documented request constraints are accepted instead of refused with the modelled `ValidationException` (HTTP 400):

1. `StartTime` is documented as "The inclusive start time of the time range for the forecast data to get. The date and time can be at most 56 days before the current date and time." That is a constraint on the REQUEST, and the real service refuses a start time further back than 56 days. The handler deliberately does not compare the window against the clock (`MAX_FORECAST_HISTORY_DAYS` is treated as an availability horizon only), so a window starting a year ago answers HTTP 200 with an empty list.
2. `ScalingPlanVersion` is documented as "The version number of the scaling plan. Currently, the only valid value is 1." Any value (0, 2, -7, i64::MAX) is accepted and answered 200, because the handler never inspects the version at all.

Secondary symptom of the same class: the 7-day maximum window width is enforced only after both endpoints are snapped to 3600-second forecast periods, so a window up to ~1 hour wider than 7 days of wall time is accepted, where the real service measures "The maximum time duration between the start and end time is seven days" on the instants themselves.

## Evidence

Smithy model docs for `GetScalingPlanResourceForecastDataRequest$StartTime` ("at most 56 days before the current date and time"), `$ScalingPlanVersion` ("Currently, the only valid value is 1") and `$EndTime` ("The maximum time duration between the start and end time is seven days") — mirrored verbatim in the handler header (src/ops/get_scaling_plan_resource_forecast_data.rs lines 99-107) and in `MAX_FORECAST_HISTORY_DAYS` / `MAX_FORECAST_WINDOW_DAYS`. Handler: `validate_forecast_window` enforces only `end > start` and the period-rounded 7-day width; nothing reads `input.scaling_plan_version`. Tests tests/all/forecast_data.rs deliberately assert that a window far outside the 56-day horizon is a 200 and that period-rounded over-wide windows are accepted, i.e. they pin the current, divergent behaviour.

## Steps to Reproduce

1. Call GetScalingPlanResourceForecastData with a valid plan name, ScalingPlanVersion 1, ServiceNamespace `autoscaling`, ResourceId `autoScalingGroup/my-asg`, ScalableDimension `autoscaling:autoScalingGroup:DesiredCapacity`, ForecastDataType `CapacityForecast`, StartTime = now - 200 days, EndTime = now - 199 days → emulator answers HTTP 200 {"Datapoints":[]}; real service answers ValidationException (HTTP 400) because StartTime is more than 56 days in the past.
2. Repeat the same request with ScalingPlanVersion 2 → emulator answers HTTP 200; the only modelled valid version is 1.
3. Repeat with StartTime = T and EndTime = T + 7 days + 30 minutes → emulator accepts it because both endpoints fall in periods exactly 7 days apart.
