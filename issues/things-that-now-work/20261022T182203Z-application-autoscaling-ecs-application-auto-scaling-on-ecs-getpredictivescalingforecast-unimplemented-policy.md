Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected application-autoscaling__ecs.customized-metric-target-tracking-alarms-watch-the-customer-metric
Issue-Key: application-autoscaling__ecs|GetPredictiveScalingForecast|NotImplemented
Issue-Key: application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected|run#11|AssertionError|assert code == "ValidationException", f"forecasting a step policy gave {code!r}"
Issue-Key: application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling__ecs.customized-metric-target-tracking-alarms-watch-the-customer-metric|run#15|AssertionError|assert watched == (spec["Namespace"], spec["MetricName"], spec["Dimensions"],
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Application Auto Scaling on ECS: GetPredictiveScalingForecast unimplemented, policy-type and step-interval conflicts accepted, customized-metric alarms drop Statistic/Unit (4 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPredictiveScalingForecast

## Description
Four independent root causes from the ECS-namespace Application Auto Scaling checks. (The check ids carry the composite leading token 'application-autoscaling__ecs'; the operations are Application Auto Scaling's, exercised against ECS resources.)

Cause 1 of 4: GetPredictiveScalingForecast is an unimplemented scaffold stub.
AWS: the operation is modeled (Smithy: GetPredictiveScalingForecast, required ServiceNamespace/ResourceId/ScalableDimension/PolicyName/StartTime/EndTime; output LoadForecast + CapacityForecast + UpdateTime; errors InternalServiceException 500, ValidationException 400). For a PredictiveScaling policy it returns hourly forecast slots covering the requested window; for a policy that is NOT predictive (a StepScaling policy) it is refused with HTTP 400 ValidationException.
Emulator: both calls fail with error code NotImplemented and message 'operation GetPredictiveScalingForecast is scaffolded but not implemented in this emulator', so neither the forecast nor the modeled refusal is reachable. The report does NOT mark these checks as already filed.
Why: a client cannot read a predictive forecast at all, and its error handling for a non-predictive policy sees an unmodeled NotImplemented instead of ValidationException.
Fix: implement services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/get_predictive_scaling_forecast.rs (forecast slots for a predictive policy, ValidationException otherwise).
Traces: application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window, application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected

Cause 2 of 4: PutScalingPolicy accepts changing a policy's type under the same name.
AWS: recorded behavior - re-putting an existing policy name with a different PolicyType is refused with HTTP 400 ValidationException (Smithy: PutScalingPolicy declares ValidationException).
Emulator: HTTP 200 with a PolicyARN and freshly minted TargetTracking alarms, silently replacing the policy's type.
Why: a typo or a bad deploy rewrites a live policy's semantics instead of failing, and the previous policy's alarms/behavior are lost.
Fix: PutScalingPolicy handler, existing-policy type-change precondition.
Traces: application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected

Cause 3 of 4: PutScalingPolicy accepts overlapping step-adjustment intervals.
AWS: recorded behavior - a StepScaling configuration whose StepAdjustment intervals overlap is refused with HTTP 400 ValidationException.
Emulator: HTTP 200 with a PolicyARN and an empty Alarms list.
Why: an ambiguous step policy is stored, and which adjustment a breach picks is undefined - a client never learns its policy is invalid.
Fix: PutScalingPolicy validation, step-adjustment interval checking.
Traces: application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected

Cause 4 of 4: alarms minted for a CustomizedMetricSpecification ignore the spec's Statistic and Unit.
AWS: recorded behavior - the two alarms (high and low, no provisioned-capacity pair) watch the customer's metric with the spec's Namespace, MetricName, Dimensions, Statistic AND Unit.
Emulator: namespace, metric name and dimensions are carried over, but the alarm reports Statistic 'Average' and Unit absent regardless of the spec.
Why: the alarm measures a different aggregation than the policy declared, so scaling triggers on the wrong signal.
Fix: PutScalingPolicy handler, customized-metric alarm construction (propagate Statistic and Unit).
Traces: application-autoscaling__ecs.customized-metric-target-tracking-alarms-watch-the-customer-metric

## Evidence
Replay report details:
- predictive-scaling-forecast-returns-hourly-slots-for-the-window: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetPredictiveScalingForecast operation: operation GetPredictiveScalingForecast is scaffolded but not implemented in this emulator. forecast-for-a-non-predictive-policy-rejected: "forecasting a step policy gave 'NotImplemented'" (expected ValidationException).
- changing-a-policy-type-under-the-same-name-rejected: 'expected the call to be refused, but it succeeded: {PolicyARN: arn:aws:autoscaling:...:policyName/<pol>, Alarms: [TargetTracking-service/<cluster>/<svc>-AlarmHigh-..., ...-AlarmLow-...], HTTPStatusCode 200}'.
- overlapping-step-adjustment-intervals-rejected: 'expected the call to be refused, but it succeeded: {PolicyARN: ..., Alarms: [], HTTPStatusCode: 200}'.
- customized-metric-target-tracking-alarms-watch-the-customer-metric: "alarm watches sim2real/behavioral/Backlog dims=[{'Name': 'Queue', 'Value': '<queue>'}] stat='Average' unit=None, not the customer metric" - the assertion compares (Namespace, MetricName, Dimensions, Statistic, Unit) against the submitted CustomizedMetricSpecification.
Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (get_predictive_scaling_forecast.rs, put_scaling_policy.rs).

## Steps to Reproduce
Boot the emulator; with botocore against an ECS service registered as ecs:service:DesiredCount:
1. put_scaling_policy PolicyType='StepScaling', then get_predictive_scaling_forecast(... PolicyName=<that policy> ...) -> expect ValidationException, observe NotImplemented; repeat with a PredictiveScaling policy -> expect hourly LoadForecast/CapacityForecast slots.
2. put_scaling_policy PolicyName=P PolicyType='TargetTrackingScaling', then put_scaling_policy PolicyName=P PolicyType='StepScaling' -> expect ValidationException, observe 200.
3. put_scaling_policy StepScaling with two overlapping StepAdjustment intervals -> expect ValidationException, observe 200.
4. put_scaling_policy TargetTrackingScaling with a CustomizedMetricSpecification carrying Statistic (e.g. 'Sum') and Unit (e.g. 'Count'), then cloudwatch.describe_alarms on the returned alarm names and compare Statistic/Unit with the spec.
