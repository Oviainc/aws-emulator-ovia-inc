Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# Forecast window drops the data point at the last whole second before a sub-second EndTime

_Filed: 2026-10-25T18:55:21Z_
_Operation: GetScalingPlanResourceForecastData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`forecast_instants` truncates `EndTime` to whole seconds with `end_time.secs()` and then keeps points while `at < end`. For an `EndTime` that sits INSIDE a second (an `awsJson1_1` `TimestampType` is fractional epoch seconds, so this is reachable), the forecast interval boundary sitting at `end_time.secs()` is excluded even though it is strictly before `EndTime` and therefore inside the half-open window `[StartTime, EndTime)` the function's own doc comment describes.

The comment immediately above the line states the opposite of what the line does: "A point at exactly `EndTime` is outside the window; one at the last whole second before an `EndTime` inside a second is inside it." The crate already owns the helper that implements exactly that rule — `ceil_secs` ("The first whole second at or after a modelled `TimestampType`") — and the start of the window is computed with it one line earlier (`let first = ceil_secs(start_time).max(earliest);`); the end hand-inlines `end_time.secs()` instead. With `ceil_secs(end_time)` both halves of the stated rule hold (whole-second `EndTime` is unchanged, so a point exactly at `EndTime` stays excluded).

Concretely, a request for `[H0, H1 + 0.5s)` over a plan with a predictive instruction returns only the point at H0; the point at H1 — strictly earlier than `EndTime` — is lost, so the last hour of a sub-second window silently carries no forecast.

## Evidence

The contradiction, both sides in one file — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:531-533 and :540:
```
    // A point at exactly `EndTime` is outside the window; one at the last whole second
    // before an `EndTime` inside a second is inside it.
    let end = end_time.secs();
...
    while at < end && at <= latest {
```
With `EndTime = H1 + 0.5s`, `end == H1`, so the candidate `at == H1` fails `at < end` and is dropped — the comment says it is "inside it".

The helper the crate already has, used for the other endpoint — same file:504-512 and :530:
```
/// The first whole second at or after a modelled `TimestampType`, so a start inside a
/// second does not pull in a data point that sits before it.
fn ceil_secs(at: ::aws_smithy_types::DateTime) -> i64 {
    if at.subsec_nanos() > 0 {
        at.secs().saturating_add(1)
    } else {
        at.secs()
    }
}
...
    let first = ceil_secs(start_time).max(earliest);
```

The window rule the function documents — same file:516-518: "so the points sit on the interval boundaries inside the half-open window `[StartTime, EndTime)` — the start is \"inclusive\" and the end \"exclusive\"". The handler itself compares the two request timestamps at full nanosecond precision (`epoch_nanos`, same file:282-284, used by `validate_forecast_window`), so the sub-second part of `EndTime` is otherwise honoured.

Test coverage: tests/all/forecast_data.rs:476-571 (`the_sub_second_part_of_a_window_adds_no_forecast_period_to_its_width`) drives sub-second endpoints but asserts only `Verdict::Accepted/Refused...` envelopes, never which Datapoints come back; tests/all/forecast_datapoints.rs:81-97 builds every window from whole-second `i64` offsets (`current_interval_start()`, forecast_datapoints.rs:76-78). No test pins the content of a window whose `EndTime` is inside a second, so the off-by-one is unpinned.

## Steps to Reproduce

1. CreateScalingPlan with one instruction for autoscaling / autoScalingGroup/my-asg / autoscaling:autoScalingGroup:DesiredCapacity that configures predictive scaling (e.g. PredefinedLoadMetricSpecification ASGTotalNetworkIn) and one TargetTrackingConfiguration; wait for the plan to become Active.
2. Let H0 be the start of the current clock hour and H1 = H0 + 3600. Call GetScalingPlanResourceForecastData with ForecastDataType CapacityForecast, StartTime H0 and EndTime H1 + 0.5 (fractional epoch seconds, e.g. 1761000000.5).
3. Observe a single Datapoint, at H0. Per the handler's own comment and the documented half-open window, the point at H1 is strictly before EndTime and should be present.
4. Repeat with EndTime = H1 + 1 (whole second): two Datapoints (H0 and H1) come back, showing the sub-second part alone removes the last point.
