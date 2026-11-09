Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs cloudwatch__events__sqs.xsvc-cloudwatch-1-input-transformer-reshapes-alarm-state-change cloudwatch__events__sqs.xsvc-cloudwatch-1-state-and-resource-pattern-blocked-ok-and-other-alarm cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs|_rule_live_barrier#17|AssertionError|raise AssertionError(f"{what}: no canary event delivered after {flips} transitions")
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-1-input-transformer-reshapes-alarm-state-change|_rule_live_barrier#18|AssertionError|raise AssertionError(f"{what}: no canary event delivered after {flips} transitions")
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-1-state-and-resource-pattern-blocked-ok-and-other-alarm|_rule_live_barrier#23|AssertionError|raise AssertionError(f"{what}: no canary event delivered after {flips} transitions")
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations|_poll_until#23|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CloudWatch: alarm state transitions emit no 'CloudWatch Alarm State Change' event to EventBridge, so matched rules, input transformers and the DLQ/FailedInvocations path never see anything

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
Cause 1 of 1: no CloudWatch alarm state-change events are published to EventBridge.
AWS: when a metric alarm changes state (including via `SetAlarmState`), CloudWatch publishes a `CloudWatch Alarm State Change` event with `source` `aws.cloudwatch`, the alarm ARN in `resources`, and a `detail` carrying `alarmName`, `state` (value/reason/timestamp) and `previousState`, onto the default event bus. A rule matching that detail-type delivers the event to its targets; a rule with an InputTransformer delivers the reshaped payload; a rule whose pattern also constrains `detail.state.value` and `resources` delivers only the matching alarms; and a target whose queue policy denies EventBridge dead-letters the event and increments `AWS/Events` `FailedInvocations`.
Emulator: nothing is ever delivered. Across four checks the canary/alarm transition is driven 60, 120 and 60 times respectively and no event reaches the rule's queue; the dead-letter variant records `(0, 0)` — neither the healthy rule's queue nor the failing rule's DLQ receives anything within 150s. Because no event is emitted at all, the input-transformer rendering, the pattern precision and the dead-letter/metric behavior are all unreachable rather than wrong.
Why: a client building alarm-driven automation (notify, remediate, fan out) gets silence: no delivery, no dead letter, no FailedInvocations datapoint, and no error anywhere to explain it.
Fix: the CloudWatch alarm state machine's event emission — the alarm-state transition path (including SetAlarmState and evaluation-driven transitions) must publish a `CloudWatch Alarm State Change` event onto the default bus so EventBridge's rule matching, input transformation and dead-letter paths run.
Traces: cloudwatch__events__sqs.xsvc-cloudwatch-1-alarm-state-change-chain-delivered-to-sqs; cloudwatch__events__sqs.xsvc-cloudwatch-1-input-transformer-reshapes-alarm-state-change; cloudwatch__events__sqs.xsvc-cloudwatch-1-state-and-resource-pattern-blocked-ok-and-other-alarm; cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-change-event-denied-by-queue-policy-dead-letters-and-counts-failed-invocations.

Note: open issues 20261025T002357Z-cloudwatch-events-sqs-cloudwatch-emits-no-alarm-events-to-eventbridge-no.md and 20261028T010828Z-cloudwatch-events-sqs-cloudwatch-publishes-no-cloudwatch-alarm-configuration-change-event.md carry other cloudwatch__events__sqs checks (configuration-change events); none of them carries the four replay keys above, which are all about the STATE-change event.

## Evidence
`AssertionError: the default-bus rule becoming live: no canary event delivered after 60 transitions`; `AssertionError: the rule becoming live: no canary event delivered after 120 transitions`; `AssertionError: the wide rule becoming live: no canary event delivered after 60 transitions`; `AssertionError: one alarm change event reached the healthy rule's queue AND the failing rule's DLQ (good, dlq counts) not met within 150s. Last result: (0, 0)`.

## Steps to Reproduce
1. `cloudwatch.put_metric_alarm(...)` for a metric alarm.
2. PutRule on the default bus with `{"source":["aws.cloudwatch"],"detail-type":["CloudWatch Alarm State Change"]}` and PutTargets an SQS queue (and a variant with an InputTransformer, and a variant whose queue policy denies events.amazonaws.com with a DLQ configured).
3. Drive the alarm with `cloudwatch.set_alarm_state(AlarmName=..., StateValue='ALARM'|'OK', StateReason=...)` repeatedly.
4. Poll the target queue, the DLQ, and the `AWS/Events` `FailedInvocations` metric — all stay empty/zero.
5. Replay the four checks in trace_ids.
