Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected
Issue-Key: application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window|run#28|AssertionError|assert all(math.isnan(float(v)) for v in load["Values"] + cap["Values"]), (
Issue-Key: application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected|run#12|AssertionError|assert msg == (f"No predictive scaling policy with the name {step} found. "
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Application Auto Scaling GetPredictiveScalingForecast fabricates numeric forecasts for a target with no load history and misreports the non-predictive-policy refusal

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPredictiveScalingForecast

## Description
Two causes in GetPredictiveScalingForecast. Both must be closed before this issue is done.

Cause 1 of 2: a scalable target with no load history is answered with numeric forecast values.
AWS: Smithy: GetPredictiveScalingForecast returns LoadForecast (LoadForecasts) and CapacityForecast over the requested StartTime/EndTime window. Recorded behavior: for an ECS service with no load history the hourly slots are present but their Values are NaN — AWS does not invent a number it has no data for.
Emulator: every slot in LoadForecast.Values and CapacityForecast.Values is a finite number.
Why: a client that branches on "no forecast available yet" (NaN) instead scales on fabricated numbers, which is exactly the decision predictive scaling exists to make.
Fix: the GetPredictiveScalingForecast handler / forecast computation in the application-autoscaling crate (services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/get_predictive_scaling_forecast.rs) — emit NaN-valued slots when there is no observed load.
Traces: application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window.

Cause 2 of 2: the refusal for a policy that is not a predictive-scaling policy carries a different constraint sentence.
AWS: Smithy: GetPredictiveScalingForecast models ValidationException (httpError 400, awsQueryError code ValidationException) and PolicyName is a required member (length 1-256, pattern ^\\p{Print}+$). Recorded: the ValidationException message is built from the request and ends with the remediation sentence "Verify the policy name and try again." after the "Value '<policyName>' at 'policyName' failed to satisfy constraint: Member must name a PredictiveScaling policy" clause.
Emulator: returns "Value '<policyName>' at 'policyName' failed to satisfy constraint: Member must name a PredictiveScaling policy, and that policy has policy type StepScaling" — it appends the found policy type and drops the remediation sentence.
Why: the message is the only machine-visible discriminator for this refusal (the code is the generic ValidationException), so a client matching AWS's constraint text does not recognise it.
Fix: the GetPredictiveScalingForecast handler's policy-type validation message.
Traces: application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected.

## Evidence
Replay output (both verdict mismatch):
- application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window — `assert all(math.isnan(float(v)) for v in load["Values"] + cap["Values"])` -> AssertionError: "a service with no load history produced a numeric forecast".
- application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected — AssertionError: rejection message was "Value 'sim2real-behav-aas-step-<id>' at 'policyName' failed to satisfy constraint: Member must name a PredictiveScaling policy, and that policy has policy type StepScaling"; the check requires the recorded form that ends with "Verify the policy name and try again."

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-run the two checks in trace_ids.
2. RegisterScalableTarget for an ECS service (ServiceNamespace=ecs, ScalableDimension=ecs:service:DesiredCount), PutScalingPolicy with a PredictiveScaling policy, then GetPredictiveScalingForecast over a future window: all returned Values are numeric where AWS returns NaN.
3. PutScalingPolicy with PolicyType=StepScaling and call GetPredictiveScalingForecast naming that policy: compare the ValidationException message with AWS's recorded constraint text.
