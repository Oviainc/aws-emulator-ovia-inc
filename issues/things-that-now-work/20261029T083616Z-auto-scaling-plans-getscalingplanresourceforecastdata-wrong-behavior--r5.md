Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: ede2de3f7102f867619bb62880127888c08c3e6a

# The scheduled-action forecast types report a different number of data points than the load/capacity types over the same window, although forecast_instants' own doc states the count cannot depend on the buffer shift

_Filed: 2026-10-29T08:36:16Z_
_Operation: GetScalingPlanResourceForecastData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ede2de3f7102f867619bb62880127888c08c3e6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`forecast_instants` selects reported instants by walking interval boundaries, offset by the buffer `shift_secs`, and keeping those inside the half-open window `[StartTime, EndTime)`. Because the progression `{k * 3600 - shift}` has a different phase for each shift, the number of its members that land in a window whose width is not a whole multiple of the forecast interval depends on the shift. The doc immediately above the function asserts the opposite: "A window therefore carries the same NUMBER of data points whatever the shift; only where inside each interval they sit moves."

The observable consequence: for one instruction and one window, `LoadForecast`/`CapacityForecast` (shift 0) and `ScheduledActionMinCapacity`/`ScheduledActionMaxCapacity` (shift = `ScheduledActionBufferTime`, default 300) return lists of different lengths, in either direction.

With `H` an hour boundary and a 300-second buffer:

* window `[H, H + 3900)` — shift 0 yields the two instants `H` and `H + 3600`; shift 300 yields only `H + 3300` (`H + 6900` is past the end). 2 points vs 1.
* window `[H + 3300, H + 7200)` — shift 0 yields only `H + 3600`; shift 300 yields `H + 3300` and `H + 6900`. 1 point vs 2.

So either the doc's claim is wrong about what the code does, or the selection rule is wrong about which scheduled actions belong to a window; the two cannot both stand. Nothing in the model relates the counts, so the code and its own doc are the only two sides here.

## Evidence

The doc that states the rule — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:518-525:
```
/// The instants returned are the ones the data points are REPORTED at — interval boundary
/// minus `shift_secs` — so the window and the span forecast data exists over both select
/// by the instant a `Datapoint` actually carries: inside the half-open window
/// `[StartTime, EndTime)` — the start is "inclusive" and the end "exclusive" — and inside
/// the span over which the service has forecast data at all, no further back than
/// [`MAX_FORECAST_HISTORY_DAYS`] ("Data points are available for up to 56 days") and no
/// further ahead than [`FORECAST_HORIZON_DAYS`]. A window therefore carries the same NUMBER
/// of data points whatever the shift; only where inside each interval they sit moves.
```

The code under it, which makes the count phase-dependent — same file, lines 541-555:
```
    // The first interval boundary whose reported instant (boundary - shift) is at or after
    // the start of the served span.
    let first_boundary = first.saturating_add(shift_secs);
    let mut boundary =
        first_boundary.div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS;
    if boundary < first_boundary {
        boundary = boundary.saturating_add(FORECAST_INTERVAL_SECONDS);
    }
    let mut at = boundary.saturating_sub(shift_secs);
    let mut instants = Vec::new();
    while at < end && at <= latest {
        instants.push(at);
        at = at.saturating_add(FORECAST_INTERVAL_SECONDS);
    }
    instants
```
and the shift the two scheduled-action types carry, same file, lines 780-786:
```
    fn reported_shift(&self, kind: ForecastKind) -> i64 {
        if kind.is_scheduled_action() {
            self.scheduled_action_buffer_seconds
        } else {
            0
        }
    }
```

Test coverage: the suite that pins the buffer rule only ever asks over an INTERVAL-ALIGNED window, which is exactly the case where the counts do coincide — tests/all/scheduled_action_buffer_time.rs:69-73:
```
/// The interval-aligned window every row below asks its forecast over.
fn window() -> (i64, i64) {
    let start = current_interval_start() - WINDOW_START_PERIODS_AGO * FORECAST_INTERVAL_SECONDS;
    (start, start + WINDOW_PERIODS * FORECAST_INTERVAL_SECONDS)
}
```
and its count assertion is made against that aligned window only (same file, :295-301: "a window carries one scheduled scaling action per forecast interval whatever the buffer"). No test drives a window whose width is not a whole number of intervals, so no existing test pins the divergence either way.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "shift-count", a CloudFormation stack ApplicationSource, and one instruction for ServiceNamespace "autoscaling", ResourceId "autoScalingGroup/my-asg", ScalableDimension "autoscaling:autoScalingGroup:DesiredCapacity", MinCapacity 2, MaxCapacity 8, one TargetTrackingConfiguration tracking ASGAverageCPUUtilization at 40, PredefinedLoadMetricSpecification ASGTotalCPUUtilization, and ScheduledActionBufferTime 300. Let it settle to Active.
2. Pick H = an hour boundary a few hours in the past (epoch seconds divisible by 3600, inside the 56-day history span).
3. GetScalingPlanResourceForecastData with StartTime = H, EndTime = H + 3900, ForecastDataType = "CapacityForecast": two Datapoints, at H and H + 3600.
4. Repeat the same window with ForecastDataType = "ScheduledActionMinCapacity": one Datapoint, at H + 3300 — one fewer point for the same window and the same instruction.
5. Now use StartTime = H + 3300, EndTime = H + 7200. "CapacityForecast" returns one Datapoint (H + 3600) while "ScheduledActionMinCapacity" returns two (H + 3300 and H + 6900) — the divergence runs the other way.
6. Compare both observations with the rule stated at get_scaling_plan_resource_forecast_data.rs:524-525 ("A window therefore carries the same NUMBER of data points whatever the shift").
