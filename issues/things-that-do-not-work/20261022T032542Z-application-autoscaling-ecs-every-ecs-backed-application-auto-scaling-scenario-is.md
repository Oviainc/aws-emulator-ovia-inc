Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity application-autoscaling__ecs.changing-a-policy-type-under-the-same-name-rejected application-autoscaling__ecs.customized-metric-target-tracking-alarms-watch-the-customer-metric application-autoscaling__ecs.deregister-strips-the-policy-from-a-customer-alarm-but-keeps-the-alarm application-autoscaling__ecs.forecast-for-a-non-predictive-policy-rejected application-autoscaling__ecs.overlapping-step-adjustment-intervals-rejected application-autoscaling__ecs.predictive-scaling-forecast-returns-hourly-slots-for-the-window application-autoscaling__ecs.step-scaling-adjustment-is-clamped-into-the-current-registered-bounds application-autoscaling__ecs.step-scaling-policy-round-trips-and-lists-the-alarm-that-invokes-it application-autoscaling__ecs.target-tracking-converges-to-the-target-and-stops application-autoscaling__ecs.two-policies-triggered-together-scale-by-the-largest-not-the-sum
Issue-Key: application-autoscaling__ecs|CreateService|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Every ECS-backed Application Auto Scaling scenario is unreachable: ECS CreateService answers NotImplemented, blocking all 12 application-autoscaling__ecs checks

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Cause 1 of 1: ECS CreateService is an unimplemented scaffold stub, so no ECS service exists to scale and all twelve cross-service ECS checks fail during setup.
AWS: ECS CreateService returns the created service (status, desiredCount, taskDefinition); `service/<cluster>/<service>` with scalable dimension ecs:service:DesiredCount is the ECS scalable resource that Application Auto Scaling registers, attaches step/target-tracking/predictive policies to, and moves via UpdateService. The downstream behaviours these checks assert — step-scaling clamped into the registered bounds, two policies triggered together scaling by the largest and not the sum, customized-metric alarms watching the customer metric, a never-breaching metric producing NO scaling activity, GetPredictiveScalingForecast hourly slots and its rejection for a non-predictive policy, overlapping step-adjustment intervals and a policy-type change under one name being refused, and deregister stripping the policy from a customer alarm while keeping the alarm — are all reached only after CreateService succeeds (recorded behavior).
Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateService operation: operation CreateService is scaffolded but not implemented in this emulator`. Every one of the twelve checks aborts at that first call; the Application Auto Scaling assertions are never exercised.
Why: a client cannot create an ECS service at all, so the most common Application Auto Scaling use case (scaling an ECS service) is impossible against the emulator, and none of the ECS scaling semantics can be validated.
Fix: implement the ECS CreateService operation handler (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/create_service.rs) together with the cluster/service/desiredCount state it owns and the UpdateService path Application Auto Scaling drives; then re-replay the twelve checks listed in trace_ids.
Traces: all twelve application-autoscaling__ecs checks listed in trace_ids.

## Evidence
Identical failure for all 12 checks of the application-autoscaling__ecs service token:
`botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateService operation: operation CreateService is scaffolded but not implemented in this emulator`
Expected: HTTP 200 CreateService response carrying the service with its desiredCount, after which RegisterScalableTarget/PutScalingPolicy assertions run. Per-service pass rate in the suite report: application-autoscaling__ecs 0/17 (0%), 17 failed (12 distinct checks shown; the remainder are blocked by separately filed stubs).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Call ECS CreateService (any cluster/taskDefinition/desiredCount) against the emulator — observe the NotImplemented error instead of a created service.
3. Re-replay the checks in trace_ids once CreateService is implemented; each then exercises a distinct ECS Application Auto Scaling behaviour.
