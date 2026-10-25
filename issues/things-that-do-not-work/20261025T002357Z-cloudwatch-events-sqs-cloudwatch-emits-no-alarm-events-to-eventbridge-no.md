Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.xsvc-cloudwatch-1-state-and-resource-pattern-blocked-ok-and-other-alarm cloudwatch__events__sqs.xsvc-cloudwatch-1-input-transformer-reshapes-alarm-state-change cloudwatch__events__sqs.xsvc-cloudwatch-2-state-changes-tagging-and-noop-puts-emit-no-configuration-change-event cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-1-state-and-resource-pattern-blocked-ok-and-other-alarm|_drain_into#23|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-1-input-transformer-reshapes-alarm-state-change|_drain_into#18|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-2-state-changes-tagging-and-noop-puts-emit-no-configuration-change-event|_drain#11|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied|_drain#30|Timeout|resp = sqs.receive_message(
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# CloudWatch emits no alarm events to EventBridge: no 'CloudWatch Alarm State Change' or configuration-change event is ever routed, and FailedInvocations is not recorded (2 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 2 — CloudWatch publishes no alarm events to the default event bus.
AWS: an alarm state transition emits a `aws.cloudwatch` event with detail-type "CloudWatch Alarm State Change" on the account's default bus, carrying the alarm name, the previous and new state and the reason; `detail.state.value`/`detail.previousState.value` are what a state-and-resource event pattern filters on, so a rule matching one state and one alarm resource receives that transition and nothing else. Separately, a `PutMetricAlarm` that actually changes an alarm's configuration emits "CloudWatch Alarm Configuration Change", while a tagging call or a no-op put (identical configuration) emits none.
Emulator: nothing is delivered on any of these paths. Each check completes CreateQueue, SetQueueAttributes, PutMetricAlarm, PutRule and PutTargets (FailedEntryCount 0) and the alarm transition (SetAlarmState / metric data), then polls its queue; no message ever arrives and the replay cuts each check off at 60 s. Because the positive control never fires, the negative halves — a near-miss state/resource pattern receiving nothing, a tagging call and a no-op put emitting no configuration-change event, and an InputTransformer reshaping the alarm event — cannot be graded either.
Why: alarm-driven automation (EventBridge rule on alarm state change -> remediation target) never runs; a client sees an alarm transitioning in CloudWatch and a rule reporting healthy, with no event in between and nothing to diagnose.
Fix: the CloudWatch alarm event publisher — the code that must emit to EventBridge from the SetAlarmState / PutMetricAlarm / metric-evaluation paths in services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801 — and the EventBridge default-bus ingestion of `aws.cloudwatch` events.
Traces: cloudwatch__events__sqs.xsvc-cloudwatch-1-state-and-resource-pattern-blocked-ok-and-other-alarm, cloudwatch__events__sqs.xsvc-cloudwatch-1-input-transformer-reshapes-alarm-state-change, cloudwatch__events__sqs.xsvc-cloudwatch-2-state-changes-tagging-and-noop-puts-emit-no-configuration-change-event.

Cause 2 of 2 — the FailedInvocations metric is not recorded for a target EventBridge could not deliver to.
AWS: when a rule's target rejects delivery (here, a destination bus/queue whose policy denies the forwarding bus), EventBridge increments the `AWS/Events` `FailedInvocations` metric dimensioned on the RuleName, so the failure is visible in CloudWatch even when the event is gone.
Emulator: the metric is never observable — the check polls GetMetricStatistics/GetMetricData for FailedInvocations after provoking a denied delivery and is cut off at 60 s with nothing recorded.
Why: the only signal AWS gives for silently dropped deliveries is missing, so a client monitoring FailedInvocations sees a permanently healthy rule while every event is being discarded.
Fix: the EventBridge target dispatcher's failure accounting — it must publish AWS/Events FailedInvocations to CloudWatch on a delivery refusal — and the CloudWatch metric store that must serve it.
Traces: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied.

## Evidence
All four checks report "Check timed out after 60.0s". In each, the setup calls returned 2xx first: CreateQueue, SetQueueAttributes (policy admitting the rule ARN), PutMetricAlarm, PutRule, PutTargets (FailedEntryCount 0), and the state transition itself (SetAlarmState / PutMetricData). The terminal reads — ReceiveMessage on the rule's queue for the alarm event, and GetMetricStatistics for AWS/Events FailedInvocations — never return anything. Expected: an `aws.cloudwatch` envelope with detail-type "CloudWatch Alarm State Change" (or "CloudWatch Alarm Configuration Change") on the matching rule's queue and none on the near-miss rule's queue, and a non-zero FailedInvocations datapoint for the rule whose delivery was denied.

## Steps to Reproduce
Cause 1:
1. CreateQueue(q); SetQueueAttributes(q, Policy admitting events.amazonaws.com for the rule ARN).
2. PutMetricAlarm(AlarmName=a, ...).
3. PutRule(Name=r, EventPattern='{"source":["aws.cloudwatch"],"detail-type":["CloudWatch Alarm State Change"],"resources":[<alarm arn>],"detail":{"state":{"value":["ALARM"]}}}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<q arn>}]).
4. SetAlarmState(AlarmName=a, StateValue='ALARM', StateReason='t') -> ReceiveMessage(q) never returns a message.
5. TagResource on the alarm, and a PutMetricAlarm with an identical configuration -> no configuration-change event either way (unverifiable, since the positive control never fired).

Cause 2:
1. Wire a rule whose target is an event bus/queue whose policy denies the forwarding principal; PutEvents a matching event.
2. GetMetricStatistics(Namespace='AWS/Events', MetricName='FailedInvocations', Dimensions=[{Name:'RuleName', Value:r}]) -> no datapoints.
