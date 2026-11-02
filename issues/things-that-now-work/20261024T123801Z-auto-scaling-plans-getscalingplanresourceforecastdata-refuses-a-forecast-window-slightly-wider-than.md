Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-forecast-window-is-rejected-before-plan-lookup
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: auto-scaling-plans|GetScalingPlanResourceForecastData|ValidationException
Issue-Commit: 3e20db6d733c4a1cf0674d75bed6ce3984e4f3c1

# GetScalingPlanResourceForecastData refuses a forecast window slightly wider than seven days that AWS accepts

> _Note: this issue was identified against the code base at commit `3e20db6d733c4a1cf0674d75bed6ce3984e4f3c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetScalingPlanResourceForecastData

## Description
One cause.

Cause 1 of 1: the documented seven-day maximum forecast-window width is enforced as an exact instant-precision comparison, so a window only marginally wider than seven days is refused with ValidationException instead of being served. Failing check: auto-scaling-plans.asp-forecast-window-is-rejected-before-plan-lookup.

AWS: Smithy (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice.json) — GetScalingPlanResourceForecastData takes required StartTime/EndTime (TimestampType) and binds exactly two errors: ValidationException (HTTP 400, client) and InternalServiceException (HTTP 500). GetScalingPlanResourceForecastDataRequest$EndTime documents "The maximum time duration between the start and end time is seven days"; the model states no precision for that measurement. Recorded behavior: the real service accepts a window of exactly 7 days AND a window of 7 days plus one minute (both answer 200 with an empty Datapoints list for a plan that does not exist), and refuses clearly wider windows — 10 days, and a 60-day window spanning the present — with HTTP 400 ValidationException, message "The maximum allowed time range is 7 days". So the ceiling is NOT a strict second/nanosecond-precision comparison on (EndTime - StartTime). Hypothesis: AWS measures the span after truncating the endpoints to a coarser granularity (day or hour bucket), so a window a minute past seven days still falls inside the allowed count.

Emulator: validate_forecast_window compares whole nanoseconds exactly — a window of 7 days + 1 minute is refused with HTTP 400 ValidationException "The maximum allowed time range is 7 days". The replayed check calls GetScalingPlanResourceForecastData for StartTime=now (truncated to the hour) and EndTime=now+7d1m and the call raises ValidationException instead of returning 200 with Datapoints: [].

Why: a client that asks for a forecast window it computed from a loose seven-day bound (e.g. a day-aligned start plus seven days plus the scheduling slack, or a window produced by adding 7 days to a now that advanced between computing the endpoints) gets a hard 400 from the emulator where AWS serves the request. The window boundary is exactly the kind of thing callers probe, so the emulator rejects a class of requests that succeed in production and turns a working caller into a validation failure.

Note: this is NOT the same defect as the open issue "Compute forecast Datapoints from the stored scaling plan instead of always answering an empty list" (20261024T065430Z-auto-scaling-plans-getscalingplanresourceforecastdata-wrong-behavior.md), which concerns the empty Datapoints body of ACCEPTED requests. This issue is about an accepted-vs-refused boundary: the check's positive controls (absent plan + valid window, every ForecastDataType, and a window of exactly 7 days) all pass today with an empty list, and the ordering claim — window validation before plan lookup — also holds. Only the width ceiling's precision diverges.

Fix: the forecast-window validation of this operation's handler — the maximum-width relation in validate_forecast_window in services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs (see Traces for the line). It must admit windows marginally over seven days as AWS does rather than compare the raw instant difference; the unit test in tests/all/forecast_data.rs that locks the exact-precision behavior will need to move with it. The documentation block in that file (lines ~9-15 and ~296-302) asserts the opposite — "a window even one second past seven days is refused" — and is the belief to correct.

Traces: failing check auto-scaling-plans.asp-forecast-window-is-rejected-before-plan-lookup (group predictive-scaling-forecast). Code: get_scaling_plan_resource_forecast_data.rs line 318 (`if end - start > days_in_nanos(MAX_FORECAST_WINDOW_DAYS)`), with MAX_FORECAST_WINDOW_DAYS at line 252 and epoch_nanos at line 269.

## Evidence
Expected (recorded against real AWS, per the check): GetScalingPlanResourceForecastData(ScalingPlanName=<a name that does not exist>, ScalingPlanVersion=1, ServiceNamespace="autoscaling", ResourceId="autoScalingGroup/<name>", ScalableDimension="autoscaling:autoScalingGroup:DesiredCapacity", ForecastDataType="LoadForecast", StartTime=now, EndTime=now+7 days+1 minute) -> HTTP 200, {"Datapoints": []}. A 10-day window and a 60-day window over the same inputs -> HTTP 400, Error.Code "ValidationException", message (example) "The maximum allowed time range is 7 days".

Actual (emulator): the 7-days-plus-one-minute window -> HTTP 400 ValidationException. Replay output:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetScalingPlanResourceForecastData operation: The maximum allowed time range is 7 days
raised from the unguarded call `over = forecast(now, now + dt.timedelta(days=7, minutes=1))()`, whose assertion message states the expectation: "real AWS accepts it, so the ceiling is not a strict second-precision comparison".

All other assertions in the check pass: the absent-plan/valid-window positive control returns 200 with [], every ForecastDataType behaves the same, the reversed and zero-width windows are refused 400 ValidationException "Start time must be before end time", the 10-day and 60-day windows are refused 400 "The maximum allowed time range is 7 days", and the exactly-7-day window returns 200 with []. So the status codes, the error code and the precedence of window validation over plan lookup are all correct; only the width ceiling's precision is wrong.

Code proof (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:308-330): the window is converted to whole nanoseconds (epoch_nanos) and refused when `end - start > days_in_nanos(7)`, i.e. the instant difference is compared exactly; the surrounding doc comment deliberately states "a window even one second past seven days is refused".

## Steps to Reproduce
1. Point an autoscaling-plans client at the emulator. No setup is needed — the call does not require an existing plan.
2. Pick now = the current UTC time truncated to the hour. Call GetScalingPlanResourceForecastData with ScalingPlanName = any name that was never created, ScalingPlanVersion = 1, ServiceNamespace = "autoscaling", ResourceId = "autoScalingGroup/<that name>", ScalableDimension = "autoscaling:autoScalingGroup:DesiredCapacity", ForecastDataType = "LoadForecast", StartTime = now, EndTime = now + 7 days. Observe HTTP 200 with {"Datapoints": []} (the ceiling admits exactly 7 days) — this is the control.
3. Repeat the identical call with EndTime = now + 7 days + 1 minute. The emulator answers HTTP 400 ValidationException "The maximum allowed time range is 7 days"; real AWS answers HTTP 200 with {"Datapoints": []}.
4. Repeat with EndTime = now + 10 days and with StartTime = now - 30 days / EndTime = now + 30 days: both must stay refused with HTTP 400 ValidationException (they already are), so the fix must widen the ceiling's tolerance, not remove it.
5. Replay auto-scaling-plans.asp-forecast-window-is-rejected-before-plan-lookup to confirm.
