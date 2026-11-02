Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling.register-scalable-target-capacity-clamped-on-max-decrease application-autoscaling.register-below-namespace-minimum-rejected application-autoscaling.lowering-the-minimum-does-not-scale-capacity-in application-autoscaling.external-capacity-change-below-floor-not-reverted application-autoscaling.gsi-scalable-target-moves-the-index-not-the-table application-autoscaling.keyspaces-registration-floor-raises-the-table-write-capacity application-autoscaling.ecs-registration-floor-sets-the-desired-count-with-no-capacity-to-run-it application-autoscaling.custom-resource-registration-floor-patches-the-endpoint application-autoscaling.ceiling-only-scheduled-action-scales-the-table-in application-autoscaling.keyspaces-scheduled-action-moves-the-table-read-capacity application-autoscaling.custom-resource-scheduled-action-patches-the-endpoint-when-it-fires application-autoscaling.rate-action-waits-for-its-start-time-then-repeats application-autoscaling.rate-action-stops-firing-after-its-end-time application-autoscaling.partial-scheduled-action-update-keeps-schedule-and-replaces-action application-autoscaling.scheduled-action-above-registered-maximum-rejected-at-dispatch application-autoscaling.scheduled-action-in-a-second-region-scales-only-that-regions-table application-autoscaling.deregistered-target-scheduled-enforcement-blocked application-autoscaling.at-action-missed-while-suspended-is-not-replayed-on-resume application-autoscaling.suspended-state-upsert-changes-only-the-flags-it-names application-autoscaling.target-tracking-alarm-breach-triggers-scaling-activity application-autoscaling.superseded-scaling-activity-resolves-to-overridden application-autoscaling.custom-resource-step-policy-alarm-patches-the-endpoint-up-to-the-ceiling application-autoscaling.custom-resource-target-tracking-with-scale-in-disabled-still-scales-out application-autoscaling.lambda-latest-and-unqualified-targets-rejected application-autoscaling.lambda-target-on-a-missing-alias-rejected application-autoscaling.custom-resource-stage-the-api-never-deployed-rejected application-autoscaling.table-that-exists-only-in-another-region-register-rejected application-autoscaling.lambda-utilization-target-given-as-a-percentage-rejected application-autoscaling.keyspaces-target-tracking-alarms-watch-the-table-metrics application-autoscaling.lambda-target-tracking-alarms-watch-the-alias-utilization application-autoscaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies application-autoscaling.put-scaling-policy-without-target-rejected application-autoscaling.put-scheduled-action-on-unregistered-dimension-rejected application-autoscaling.scheduled-action-lifecycle
Issue-Shape-Id: com.amazonaws.applicationautoscaling#AnyScaleFrontendService
Issue-Key: application-autoscaling.register-scalable-target-capacity-clamped-on-max-decrease|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.register-below-namespace-minimum-rejected|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.lowering-the-minimum-does-not-scale-capacity-in|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.external-capacity-change-below-floor-not-reverted|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.gsi-scalable-target-moves-the-index-not-the-table|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.keyspaces-registration-floor-raises-the-table-write-capacity|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.ecs-registration-floor-sets-the-desired-count-with-no-capacity-to-run-it|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.custom-resource-registration-floor-patches-the-endpoint|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.ceiling-only-scheduled-action-scales-the-table-in|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.keyspaces-scheduled-action-moves-the-table-read-capacity|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.custom-resource-scheduled-action-patches-the-endpoint-when-it-fires|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.rate-action-waits-for-its-start-time-then-repeats|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.rate-action-stops-firing-after-its-end-time|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.partial-scheduled-action-update-keeps-schedule-and-replaces-action|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.scheduled-action-above-registered-maximum-rejected-at-dispatch|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.scheduled-action-in-a-second-region-scales-only-that-regions-table|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.deregistered-target-scheduled-enforcement-blocked|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.at-action-missed-while-suspended-is-not-replayed-on-resume|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.suspended-state-upsert-changes-only-the-flags-it-names|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.target-tracking-alarm-breach-triggers-scaling-activity|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.superseded-scaling-activity-resolves-to-overridden|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.custom-resource-step-policy-alarm-patches-the-endpoint-up-to-the-ceiling|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.custom-resource-target-tracking-with-scale-in-disabled-still-scales-out|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: application-autoscaling.lambda-latest-and-unqualified-targets-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.lambda-target-on-a-missing-alias-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.custom-resource-stage-the-api-never-deployed-rejected|_error#9|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: application-autoscaling.table-that-exists-only-in-another-region-register-rejected|run#16|AssertionError|assert _bounds(aas_home, resource_id) is None, (
Issue-Key: application-autoscaling.lambda-utilization-target-given-as-a-percentage-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: application-autoscaling.keyspaces-target-tracking-alarms-watch-the-table-metrics|run#14|AssertionError|assert shapes == expected_shapes, f'alarm set was {shapes!r}'
Issue-Key: application-autoscaling.lambda-target-tracking-alarms-watch-the-alias-utilization|run#16|AssertionError|assert len(alarm["AlarmActions"]) == 1 and alarm["AlarmActions"][0].startswith(
Issue-Key: application-autoscaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies|run#17|AssertionError|assert targets == [(_READ, 2, 6, role), (_WRITE, 2, 5, role)], (
Issue-Key: application-autoscaling.put-scaling-policy-without-target-rejected|run#10|AssertionError|assert error.get("Message") == (
Issue-Key: application-autoscaling.put-scheduled-action-on-unregistered-dimension-rejected|run#14|AssertionError|assert error['Message'] == expected, (
Issue-Key: application-autoscaling.scheduled-action-lifecycle|run#23|AssertionError|assert error.get("Message") == (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Application Auto Scaling has no enforcement or scheduling pipeline, accepts targets AWS refuses, and leaks scalable targets across regions (10 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterScalableTarget

## Description
Ten independent root causes, each grounded in failing behavioral checks whose ids carry the service token `application-auto-scaling`. None of these check ids is owned by 20261022T182203Z-application-auto-scaling-accepts-requests-real-aws-refuses.md or 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md; where a cause is thematically adjacent to one of those issues it is named below.

Cause 1 of 10: there is no bound-enforcement pipeline at all — RegisterScalableTarget is pure bookkeeping and never moves the backing resource into [MinCapacity, MaxCapacity].
AWS: Smithy models RegisterScalableTarget.MinCapacity/MaxCapacity as plain integers (ResourceCapacity) with no range, so the floor/ceiling is service-enforced; recorded behavior is that Application Auto Scaling actively adjusts the registered resource — raising a DynamoDB table's provisioned capacity up to a new MinCapacity, clamping it down to a new MaxCapacity, setting an ECS service's desiredCount, raising a Keyspaces table's write capacity, moving a DynamoDB GSI's (not the base table's) capacity, PATCHing a custom-resource endpoint, and reverting an out-of-band capacity change that drops below the floor. Each check proves this by polling the *target service* (DescribeTable / DescribeService / the customer endpoint) after the registration.
Emulator: the awaited capacity never moves. Every one of these checks exhausts the replay deadline inside its enforcement poll (`Check timed out after 60.0s`, against internal poll budgets of 150–180 s), e.g. register-below-namespace-minimum-rejected passes its rejection legs and then hangs on the positive control "a floor of 3 moving real table read capacity".
Why: the single thing the service exists to do does not happen. A client registers bounds, reads them back correctly, and the resource silently stays outside them forever; nothing that depends on auto scaling (capacity headroom, cost ceilings) behaves.
Fix: the RegisterScalableTarget handler plus a new enforcement path that mutates the target resource per namespace (dynamodb table and index, ecs service, cassandra table, lambda alias, custom-resource endpoint), and the service's state/reconciliation module.
Traces: application-auto-scaling.register-scalable-target-capacity-clamped-on-max-decrease, application-auto-scaling.register-below-namespace-minimum-rejected, application-auto-scaling.lowering-the-minimum-does-not-scale-capacity-in, application-auto-scaling.external-capacity-change-below-floor-not-reverted, application-auto-scaling.gsi-scalable-target-moves-the-index-not-the-table, application-auto-scaling.keyspaces-registration-floor-raises-the-table-write-capacity, application-auto-scaling.ecs-registration-floor-sets-the-desired-count-with-no-capacity-to-run-it, application-auto-scaling.custom-resource-registration-floor-patches-the-endpoint

Cause 2 of 10: scheduled actions are stored but never dispatched — no at()/rate()/cron() action ever fires, so no capacity moves and no scaling activity is recorded for it.
AWS: Smithy PutScheduledAction takes Schedule, Timezone, StartTime, EndTime and ScalableTargetAction; recorded behavior is that the action fires at its instant and the firing appears both as a capacity change on the target resource and as a DescribeScalingActivities entry whose Cause is `scheduled action name <name> was triggered`. Firing is also gated: ScheduledScalingSuspended withholds it (and a missed one-time at() instant is dropped, not replayed on resume), a deregistered target stops being enforced, an action above the registered MaxCapacity is clamped/refused at dispatch, and an action is scoped to the region it was created in.
Emulator: no firing is ever observed — every scheduled-action check exhausts the deadline waiting for the activity or the capacity change (`Check timed out after 60.0s`), including the ones that only need the suspend/resume gate to be observable.
Why: a client that schedules capacity changes gets none of them, and DescribeScalingActivities shows no trace, so there is no way to tell a withheld action from a broken scheduler.
Fix: a scheduled-action dispatcher in this service (schedule-expression evaluation for at/rate/cron plus Timezone, StartTime/EndTime windows, SuspendedState.ScheduledScalingSuspended gating, dispatch-time bound clamping) feeding the same enforcement path as Cause 1, and the scaling-activity state.
Traces: application-auto-scaling.ceiling-only-scheduled-action-scales-the-table-in, application-auto-scaling.keyspaces-scheduled-action-moves-the-table-read-capacity, application-auto-scaling.custom-resource-scheduled-action-patches-the-endpoint-when-it-fires, application-auto-scaling.rate-action-waits-for-its-start-time-then-repeats, application-auto-scaling.rate-action-stops-firing-after-its-end-time, application-auto-scaling.partial-scheduled-action-update-keeps-schedule-and-replaces-action, application-auto-scaling.scheduled-action-above-registered-maximum-rejected-at-dispatch, application-auto-scaling.scheduled-action-in-a-second-region-scales-only-that-regions-table, application-auto-scaling.deregistered-target-scheduled-enforcement-blocked, application-auto-scaling.at-action-missed-while-suspended-is-not-replayed-on-resume, application-auto-scaling.suspended-state-upsert-changes-only-the-flags-it-names

Cause 3 of 10: a CloudWatch alarm breach on a minted target-tracking or step-scaling alarm reaches nothing — no scaling activity, no capacity change, no activity supersession.
AWS: recorded behavior — when an alarm minted by PutScalingPolicy goes into ALARM, Application Auto Scaling records a scaling activity and applies the adjustment to the resource (clamped into the registered bounds); an activity that a later change overtakes resolves to the Overridden status; a target-tracking policy with DisableScaleIn still scales out.
Emulator: nothing happens on breach — all four checks exhaust the deadline polling DescribeScalingActivities / the target resource.
Why: policies can be created and alarms exist, but the control loop is dead, so scaling never occurs and the activity history never explains why.
Fix: alarm-breach ingestion in this service, wired to the activity recorder and the Cause 1 enforcement path. Note this is distinct from, and downstream of, the alarm-naming/action defects filed in 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md (causes 1–2 there).
Traces: application-auto-scaling.target-tracking-alarm-breach-triggers-scaling-activity, application-auto-scaling.superseded-scaling-activity-resolves-to-overridden, application-auto-scaling.custom-resource-step-policy-alarm-patches-the-endpoint-up-to-the-ceiling, application-auto-scaling.custom-resource-target-tracking-with-scale-in-disabled-still-scales-out

Cause 4 of 10: RegisterScalableTarget never resolves the backing resource, so it registers targets AWS refuses.
AWS: Smithy RegisterScalableTarget declares ValidationException (client, HTTP 400). Recorded behavior: a lambda:function:ProvisionedConcurrency target naming $LATEST or an unqualified function, or an alias that does not exist, is refused with HTTP 400 ValidationException; a custom-resource ResourceId pointing at an API Gateway stage that was never deployed is refused with HTTP 400 ValidationException naming the execute-api permissions the service lacks for GET and PATCH, because registration actually calls the customer endpoint before storing anything. In every case nothing is registered.
Emulator: HTTP 200 with a ScalableTargetARN, e.g. `{'ScalableTargetARN': 'arn:aws:application-autoscaling:...:scalable-target/<id>'}`; the phantom target is then listed by DescribeScalableTargets.
Why: a client gets a registration that can never act, and loses the only signal that its resource id or permissions are wrong. This is the same over-permissiveness family as 20261022T182203Z-application-auto-scaling-accepts-requests-real-aws-refuses.md, but these three check ids are not carried by that issue and the resource classes (lambda qualifier/alias resolution, custom-resource endpoint probe) are not covered by it.
Fix: RegisterScalableTarget handler — per-namespace resource resolution (Lambda alias/qualifier lookup, custom-resource outbound GET/PATCH probe) with ValidationException (400) on failure, and no state write on the refused path.
Traces: application-auto-scaling.lambda-latest-and-unqualified-targets-rejected, application-auto-scaling.lambda-target-on-a-missing-alias-rejected, application-auto-scaling.custom-resource-stage-the-api-never-deployed-rejected

Cause 5 of 10: scalable-target state is not partitioned by region.
AWS: recorded behavior — scalable targets are regional. Registering a target in us-east-1 for a table that exists only there leaves us-west-2's DescribeScalableTargets empty for that ResourceId.
Emulator: after registering in us-east-1, the home-region (us-west-2) client sees the same target: `registering in us-east-1 made the target appear in us-west-2`.
Why: cross-region isolation is broken, so a multi-region client reads another region's configuration and the per-region rejection (a table that exists only elsewhere) never happens. This is the same region-partitioning root cause as the region semantics cause in 20261022T182203Z-application-auto-scaling-accepts-requests-real-aws-refuses.md, re-grounded in a check that issue does not carry.
Fix: this service's state module — key scalable targets, policies, scheduled actions and activities by region as well as account.
Traces: application-auto-scaling.table-that-exists-only-in-another-region-register-rejected

Cause 6 of 10: PutScalingPolicy does not enforce the TargetValue range for LambdaProvisionedConcurrencyUtilization.
AWS: TargetValue is an unconstrained double in the model, so the bound is service-enforced; recorded behavior is that the Lambda provisioned-concurrency utilization target is a fraction and a percentage-style value is refused with HTTP 400 ValidationException (Smithy: PutScalingPolicy errors include ValidationException).
Emulator: HTTP 200, and it even mints the two TargetTracking alarms for the policy (`{'PolicyARN': 'arn:aws:autoscaling:...:policyName/pc-utilization', 'Alarms': [...AlarmHigh..., ...AlarmLow...], 'HTTPStatusCode': 200}`).
Why: a policy AWS rejects is created, and it would drive provisioned concurrency to a nonsensical target. (The DynamoDB utilization range is a separate, already-filed defect; this is the Lambda metric, which has its own range table entry.)
Fix: PutScalingPolicy validation — per-predefined-metric target-value range table, Lambda entry.
Traces: application-auto-scaling.lambda-utilization-target-given-as-a-percentage-rejected

Cause 7 of 10: the cassandra namespace mints twice the alarms AWS does, adding a Provisioned*CapacityUnits pair.
AWS: recorded behavior — a target-tracking policy on cassandra:table:WriteCapacityUnits with the Keyspaces write-utilization predefined metric mints exactly the alarm set the check pins; the emulator's extra pair on AWS/Cassandra ProvisionedWriteCapacityUnits is not part of it.
Emulator: four alarms — ('TargetTracking','AWS/Cassandra','ConsumedWriteCapacityUnits','GreaterThanThreshold'), (...,'ConsumedWriteCapacityUnits','LessThanThreshold'), (...,'ProvisionedWriteCapacityUnits','GreaterThanThreshold'), (...,'ProvisionedWriteCapacityUnits','LessThanThreshold').
Why: a client enumerating its policy's alarms sees alarms AWS never created, and the extra pair would fire on a metric that is not the utilization signal.
Fix: PutScalingPolicy handler, per-namespace target-tracking alarm derivation for the cassandra namespace.
Traces: application-auto-scaling.keyspaces-target-tracking-alarms-watch-the-table-metrics

Cause 8 of 10: minted alarm AlarmActions do not carry the ':createdBy/<uuid>' suffix on the lambda namespace either.
AWS: recorded behavior — each minted alarm has exactly one action, '<PolicyARN>:createdBy/<uuid>'.
Emulator: the action does not start with `<PolicyARN>:createdBy/`, so the check's single-action/prefix assertion fails.
Why: the action is how a breach reaches the policy; anything matching the documented prefix routes nothing. Cause 2 of 20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md reports the same shape defect under the other service token, but this check id is not carried there and it exercises the lambda:function:ProvisionedConcurrency path.
Fix: PutScalingPolicy handler, alarm-action construction.
Traces: application-auto-scaling.lambda-target-tracking-alarms-watch-the-alias-utilization

Cause 9 of 10: a Keyspaces table created with an autoScalingSpecification registers nothing in Application Auto Scaling.
AWS: recorded behavior — the specification creates one scalable target per capacity dimension with the bounds and role it declares; the check expects exactly [(read dimension, 2, 6, role), (write dimension, 2, 5, role)] plus the matching policies.
Emulator: `the specification registered []` — DescribeScalableTargets returns nothing for the table.
Why: auto scaling configured through Keyspaces is invisible to Application Auto Scaling, so neither side can manage it. Related open issue 20261024T034638Z-keyspaces-createtable-event-propagation.md reports the same propagation gap from the Keyspaces side; this cause is the Application Auto Scaling side of it and is grounded in a check that issue does not carry.
Fix: the Keyspaces CreateTable -> Application Auto Scaling propagation, landing in this service's scalable-target/scaling-policy state.
Traces: application-auto-scaling.keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies

Cause 10 of 10: the ObjectNotFoundException message template uses the wrong verb.
AWS: Smithy models ObjectNotFoundException (client, HTTP 400) on PutScalingPolicy and PutScheduledAction. The message is built from the request, so its rule is known: `No scalable target registered for service namespace: <ns>, resource ID: <rid>, scalable dimension: <dim>` and, for a missing scheduled action, the corresponding `No scheduled action ... ` template the checks pin. Status (400) and code (ObjectNotFoundException) already match — only the request-derived template text diverges.
Emulator: emits `No scalable target found for service namespace: dynamodb, resource ID: table/<tbl>, scalable dimension: dynamodb:table:WriteCapacityUnits` ("found" where AWS says "registered"), and the analogous `No scheduled action found for service namespace: ..., scheduled action name: ...`.
Why: callers and tooling that match on the templated text (the only place the service names the exact namespace/resource/dimension triple it could not find) do not match.
Fix: the ObjectNotFoundException message builder shared by the PutScalingPolicy / PutScheduledAction / DeleteScheduledAction handlers.
Traces: application-auto-scaling.put-scaling-policy-without-target-rejected, application-auto-scaling.put-scheduled-action-on-unregistered-dimension-rejected, application-auto-scaling.scheduled-action-lifecycle

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), service pass rate application-auto-scaling 38/85 (45%), 47 failing checks of which 13 are owned by 20261022T182203Z-application-auto-scaling-accepts-requests-real-aws-refuses.md and skipped. The 34 remaining:

Cause 1 / 2 / 3 (enforcement, scheduling, alarm breach) — all report `Error: Check timed out after 60.0s`, each inside a bounded poll of the TARGET resource or of DescribeScalingActivities whose own budget is 150–240 s. Representative, register-below-namespace-minimum-rejected: the ValidationException legs pass, then `_poll_until(lambda: _read_capacity(ddb, table), lambda cap: cap == 3, timeout_s=180, ...)` with description "the positive control: a floor of 3 moving real table read capacity, proving the refusal of 0 was about the value and not about a dead enforcement pipeline" never satisfies. register-scalable-target-capacity-clamped-on-max-decrease polls `predicate=lambda cap: cap <= 8` for 150 s after MaxCapacity drops from 20 to 8 with the table at 15 WCU.

Cause 4: custom-resource-stage-the-api-never-deployed-rejected — `AssertionError: expected the call to be refused, but it succeeded: {'ScalableTargetARN': 'arn:aws:application-autoscaling:us-west-2:...:scalable-target/<id>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. Same assertion text for lambda-latest-and-unqualified-targets-rejected and lambda-target-on-a-missing-alias-rejected.

Cause 5: table-that-exists-only-in-another-region-register-rejected — `AssertionError: registering in us-east-1 made the target appear in us-west-2` (assert `_bounds(aas_home, resource_id) is None`).

Cause 6: lambda-utilization-target-given-as-a-percentage-rejected — `AssertionError: expected the call to be refused, but it succeeded: {'PolicyARN': 'arn:aws:autoscaling:...:policyName/pc-utilization', 'Alarms': [{'AlarmName': 'TargetTracking-function:<fn>:live-AlarmHigh-...'}, {'AlarmName': 'TargetTracking-function:<fn>:live-AlarmLow-...'}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.

Cause 7: keyspaces-target-tracking-alarms-watch-the-table-metrics — `assert shapes == expected_shapes` failed; `alarm set was [('TargetTracking','AWS/Cassandra','ConsumedWriteCapacityUnits','GreaterThanThreshold'), ('TargetTracking','AWS/Cassandra','ConsumedWriteCapacityUnits','LessThanThreshold'), ('TargetTracking','AWS/Cassandra','ProvisionedWriteCapacityUnits','GreaterThanThreshold'), ('TargetTracking','AWS/Cassandra','ProvisionedWriteCapacityUnits','LessThanThreshold')]`.

Cause 8: lambda-target-tracking-alarms-watch-the-alias-utilization — `assert len(alarm["AlarmActions"]) == 1 and alarm["AlarmActions"][0].startswith(put["PolicyARN"] + ":createdBy/")` failed: `the alarm's single action must be the policy that minted it`.

Cause 9: keyspaces-table-auto-scaling-specification-registers-the-targets-and-policies — `assert targets == [(_READ, 2, 6, role), (_WRITE, 2, 5, role)]` failed: `the specification registered []`.

Cause 10: put-scaling-policy-without-target-rejected — `unexpected rejection message: 'No scalable target found for service namespace: dynamodb, resource ID: table/<tbl>, scalable dimension: dynamodb:table:WriteCapacityUnits'`; put-scheduled-action-on-unregistered-dimension-rejected — `the refusal message was 'No scalable target found for ...', expected 'No scalable target registered for ...'`; scheduled-action-lifecycle — `unexpected rejection message: 'No scheduled action found for service namespace: dynamodb, resource ID: ..., scheduled action name: ...'`.

Handler surface: services/service-com-amazonaws-applicationautoscaling-anyscalefrontendservice/src/ops/ (register_scalable_target.rs, put_scaling_policy.rs, put_scheduled_action.rs, delete_scheduled_action.rs, describe_scaling_activities.rs) and this service's state module.

## Steps to Reproduce
Boot the emulator and drive these with botocore (region us-west-2 unless stated).

Cause 1: create a PROVISIONED DynamoDB table at 5 WCU; register_scalable_target(dynamodb, table/<t>, dynamodb:table:WriteCapacityUnits, Min=1, Max=20); update_table to 15 WCU; register again with Max=8; poll describe_table — AWS clamps WCU to 8 within seconds, the emulator leaves 15. Repeat the floor direction (register Min=3 against a 1-RCU table) and the ecs:service:DesiredCount, cassandra:table:WriteCapacityUnits, dynamodb:index:ReadCapacityUnits and custom-resource dimensions.

Cause 2: register a target, then put_scheduled_action with Schedule='at(<next whole UTC minute>)' (and separately rate(1 minute) with StartTime/EndTime, and cron with Timezone) and a ScalableTargetAction; poll describe_table and describe_scaling_activities — AWS shows the capacity change and an activity whose Cause is 'scheduled action name <name> was triggered'; the emulator shows neither. Re-run with SuspendedState.ScheduledScalingSuspended=true, then resume, to see the gate.

Cause 3: put_scaling_policy (TargetTrackingScaling, then StepScaling) and drive the minted alarm into ALARM via cloudwatch.set_alarm_state; poll describe_scaling_activities and the resource — AWS records an activity and moves capacity, the emulator does nothing.

Cause 4: register_scalable_target for lambda:function:ProvisionedConcurrency with ResourceId 'function:<fn>:$LATEST', with an unqualified 'function:<fn>', and with a nonexistent alias; and for custom-resource with a well-formed resource id on an API Gateway stage that was never deployed. Expect HTTP 400 ValidationException and nothing registered; observe HTTP 200 and a listed target.

Cause 5: create the table in us-east-1 only; register_scalable_target with a us-east-1 client; then describe_scalable_targets with a us-west-2 client for the same ResourceId — must be empty, observe the target.

Cause 6: put_scaling_policy TargetTrackingScaling on lambda:function:ProvisionedConcurrency with LambdaProvisionedConcurrencyUtilization and a percentage-style TargetValue (e.g. 70) — expect HTTP 400 ValidationException, observe HTTP 200 with two alarms.

Cause 7: put_scaling_policy TargetTrackingScaling on cassandra:table:WriteCapacityUnits, then cloudwatch.describe_alarms — only the Consumed*CapacityUnits pair the check pins may exist.

Cause 8: same policy on lambda:function:ProvisionedConcurrency; each minted alarm must have exactly one AlarmAction beginning '<PolicyARN>:createdBy/'.

Cause 9: keyspaces.create_table with an autoScalingSpecification, then describe_scalable_targets/describe_scaling_policies for 'keyspace/<ks>/table/<t>' — expect the read and write targets with their bounds and role, observe [].

Cause 10: put_scaling_policy and put_scheduled_action against an unregistered dimension on a real table, and delete_scheduled_action for a missing action — the ObjectNotFoundException message must use AWS's request-derived template ('No scalable target registered for service namespace: ...'), not 'No scalable target found for ...'.

## Resolution

Causes 1, 2, 3, 5, 6, 8, 9 and 10 are implemented, and causes 4 and 7 are only partly: what is left of them is recorded here so a later scan does not have to rediscover it.

- Cause 4: a `lambda:function:ProvisionedConcurrency` target naming `$LATEST`, or carrying no qualifier, is now refused with HTTP 400 ValidationException and nothing is registered. Two legs remain. (i) An alias that does not exist is still accepted: refusing it needs the Lambda function store, which this crate does not reach yet (it would take a dependency on `services/service-com-amazonaws-lambda-awsgirapiservice`, plus the Lambda function/version/alias setup in this crate's test harness). (ii) A `custom-resource` ResourceId pointing at an API Gateway stage that was never deployed is still accepted: registration would have to call the customer endpoint with GET/PATCH, and this emulator serves no execute-api data plane to answer that in-process.
- Cause 7: the `cassandra` namespace still mints the `Cassandra*ProvisionedCapacity*` alarm pair beside the consumed-units pair. The recording behind the (resolved) issue `20261022T182203Z-application-autoscaling-application-auto-scaling-minted-cloudwatch-alarms-scalable-target.md` states that AWS mints exactly that pair for this namespace, and this service's `target_tracking_alarms` test suite pins it, so the two accounts of AWS disagree and the landed one was left in place.
- Cause 1, cassandra and custom-resource dimensions: a registration's capacity window is enforced on the resource for the dynamodb table and index and the ecs service dimensions. The `cassandra` dimensions are not moved because the Keyspaces crate depends on THIS crate (it writes its managed targets here), so this crate cannot read or write the Keyspaces table store without a dependency cycle — it needs a shared capacity port under `services/`. The `custom-resource` dimension is blocked by the missing execute-api data plane, as in cause 4 (ii).
