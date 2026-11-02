Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.superseded-scaling-activity-resolves-to-overridden
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.superseded-scaling-activity-resolves-to-overridden|run#19|AssertionError|assert oldest["StatusCode"] == "Overridden", (
Issue-Commit: 1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61

# A superseded scaling activity resolves to Successful instead of Overridden, so a displaced capacity change is indistinguishable from a fulfilled one

> _Note: this issue was identified against the code base at commit `1fd13cac8c9d59f908c3acc103dd89ab8a3a1d61`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
One net-new root cause, grounded in a failing behavioral check whose id carries the service token `application-auto-scaling`.

Cause 1 of 1: a scaling activity whose capacity was displaced before it was fulfilled is reported with StatusCode 'Successful' instead of 'Overridden', so the activity history loses the distinction between a change that actually took effect and one that a later change overtook.

AWS: Smithy models ScalingActivity.StatusCode (required) as the enum ScalingActivityStatusCode with values Pending, InProgress, Successful, Overridden, Unfulfilled, Failed, documented as "Indicates the status of the scaling activity". 'Overridden' exists precisely for the activity whose intended capacity was replaced by a subsequent scaling decision before the first one was confirmed; recorded behavior is that when a second capacity change displaces an in-flight one, the earlier (oldest) activity resolves to StatusCode 'Overridden' while the displacing activity resolves to 'Successful'. The terminal status is service-computed, not request-supplied, so the rule is: an activity that reached fulfilment reports 'Successful'; an activity whose target capacity was superseded before fulfilment reports 'Overridden'.

Emulator: the oldest (displaced) activity is reported with StatusCode 'Successful'. Replay: `AssertionError: the superseded activity resolved to 'Successful', not 'Overridden'; an activity whose capacity was displaced before fulfilment was confirmed is Overridden, and reporting it Successful loses the distinction entirely`. HTTP status and envelope shape are otherwise fine — only the StatusCode value diverges, and the enum member 'Overridden' appears never to be produced at all.

Why: DescribeScalingActivities is the only audit record of what Application Auto Scaling did. A client reconciling requested versus applied capacity (or alerting on scaling churn / lost adjustments) reads 'Successful' for a change that never took effect, and concludes the resource reached a capacity it never reached. Any branch keyed on the 'Overridden' member is dead code against this emulator.

Note: this check id is listed in the Issue-Trace-Ids of the open issue 20261024T115439Z-application-auto-scaling-has-no-enforcement-or-scheduling.md (its cause 3, alarm-breach ingestion), but under the replay key `_poll_until#7|Timeout` — the check no longer times out, it now reaches the status assertion, so the status-resolution defect reported here is a distinct, net-new cause that issue does not describe and the replay no longer marks owned.

Fix: the scaling-activity state machine / activity recorder of this service (the terminal-status resolution that marks a displaced, not-yet-fulfilled activity 'Overridden' when a later activity takes over its scalable target + dimension), surfaced through the DescribeScalingActivities handler.

Traces: application-auto-scaling.superseded-scaling-activity-resolves-to-overridden

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), service application-auto-scaling, check `superseded-scaling-activity-resolves-to-overridden` — verdict mismatch.

```
File "check_application_auto_scaling_superseded_scaling_activity_resolves_to_overridden.py", line 183, in run
    assert oldest["StatusCode"] == "Overridden", (
AssertionError: the superseded activity resolved to 'Successful', not 'Overridden'; an activity whose capacity was displaced before fulfilment was confirmed is Overridden, and reporting it Successful loses the distinction entirely
```

Expected (AWS): the oldest of the two activities for the scalable target reports StatusCode 'Overridden'; the displacing one reports 'Successful'.
Actual (emulator): both report 'Successful'.

Smithy evidence: com.amazonaws.applicationautoscaling#ScalingActivityStatusCode = [Pending, InProgress, Successful, Overridden, Unfulfilled, Failed]; com.amazonaws.applicationautoscaling#ScalingActivity.StatusCode is required and targets that enum.

Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/describe_scaling_activities.rs and this service's scaling-activity state module.

## Steps to Reproduce
Boot the emulator and drive with botocore in one region.

1. dynamodb.create_table with BillingMode=PROVISIONED and a small ProvisionedThroughput (e.g. ReadCapacityUnits=1); wait for TableStatus=ACTIVE.
2. application-autoscaling.register_scalable_target(ServiceNamespace='dynamodb', ResourceId='table/<t>', ScalableDimension='dynamodb:table:ReadCapacityUnits', MinCapacity=1, MaxCapacity=20).
3. Cause two capacity changes in quick succession on the same target+dimension so the second displaces the first before the first is confirmed (e.g. two back-to-back scaling decisions / scheduled or policy-driven capacity changes to different values).
4. application-autoscaling.describe_scaling_activities(ServiceNamespace='dynamodb', ResourceId='table/<t>', ScalableDimension='dynamodb:table:ReadCapacityUnits') and read the OLDEST entry's StatusCode.

AWS reports 'Overridden' for that oldest, displaced activity (and 'Successful' for the one that displaced it). The emulator reports 'Successful' for both.
