Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-configuration-change-events-route-create-update-delete-only-to-the-matching-rule
Issue-Key: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed|_drain#38|Timeout|resp = sqs.receive_message(
Issue-Key: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-configuration-change-events-route-create-update-delete-only-to-the-matching-rule|_drain#14|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Related: 20261029T064332Z-cloudwatch-events-sqs-a-destination-event-bus-s.md
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# EventBridge bus-to-bus forwarding ignores the destination bus policy, and neither InvocationsSentToDLQ nor alarm configuration-change events are observable (3 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Three root causes. 20261029T064332Z-cloudwatch-events-sqs-a-destination-event-bus-s.md, 20261028T010828Z-... and 20261025T002357Z-cloudwatch-events-sqs-cloudwatch-emits-no-alarm-events-to-eventbridge-no.md own other checks in this service; the checks below were not bound to any of them.

Cause 1 of 3: a cross-bus (event-bus target) forward succeeds even though the destination bus's resource policy denies events:PutEvents for the forwarding role.
AWS: when a rule's target is another event bus, EventBridge assumes the target's RoleArn and calls PutEvents on the destination bus. If the destination bus policy does not allow events:PutEvents for that role, the forward is refused: nothing arrives on the destination bus, its consumers see nothing, and the source rule's FailedInvocations is incremented in AWS/Events dimensioned by RuleName.
Emulator: the destination bus's consumer receives the forwarded envelope. "AssertionError: the destination bus's consumer received 1 message(s) during a 10s hold (after the control delivered) even though its bus policy denies events:PutEvents for arn:aws:iam::<account>:role/<forward role>". The fixed text of the requirement is that a denying bus policy blocks the forward; the role ARN is a value from the request.
Why: the emulator fabricates a cross-account/cross-bus delivery AWS would refuse, so bus-policy regressions and least-privilege rollouts pass locally and fail in AWS; and FailedInvocations, the only signal, is never raised.
Fix: resource-policy authorization in the EventBridge event-bus target invoker (evaluate the destination bus policy against the assumed target role before forwarding), plus the FailedInvocations emission on refusal.
Traces: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied

Cause 2 of 3: the AWS/Events InvocationsSentToDLQ metric is never published.
AWS: when a rule's target invocation fails and the target has a DeadLetterConfig, EventBridge increments InvocationsSentToDLQ in AWS/Events dimensioned by RuleName, only for the rule whose target failed; a sibling rule whose target succeeded keeps a zero sum over the same period.
Emulator: the metric never reports; the check ends "Check timed out after 60.0s".
Why: dead-letter alarming is impossible; a client cannot distinguish "no failures" from "metric not emitted".
Fix: the EventBridge dead-letter path must emit InvocationsSentToDLQ with the RuleName dimension.
Traces: cloudwatch__events__sqs.invocations-sent-to-dlq-counts-only-the-rule-whose-target-failed

Cause 3 of 3: CloudWatch publishes no "CloudWatch Alarm Configuration Change" events for PutMetricAlarm create/update and DeleteAlarms.
AWS: creating, updating or deleting an alarm emits on the default bus source "aws.cloudwatch", detail-type "CloudWatch Alarm Configuration Change", with detail.operation one of create/update/delete and detail.alarmName, and the alarm ARN in the envelope's resources. recorded behavior only for the detail body.
Emulator: a rule scoped to that detail-type receives nothing for any of the three operations; "Check timed out after 60.0s".
Why: alarm-governance automation (detect an alarm being weakened or deleted) never fires.
Fix: the CloudWatch PutMetricAlarm / DeleteAlarms handlers must publish the configuration-change event to the EventBridge bus.
Traces: cloudwatch__events__sqs.xsvc-cloudwatch-2-alarm-configuration-change-events-route-create-update-delete-only-to-the-matching-rule

## Evidence
Cause 1: "AssertionError: the destination bus's consumer received 1 message(s) during a 10s hold (after the control delivered) even though its bus policy denies events:PutEvents for arn:aws:iam::000000000000:role/<role>" — expected zero messages on the destination bus and a FailedInvocations increment on the source rule.
Cause 2: "Error: Check timed out after 60.0s" waiting for a non-zero AWS/Events InvocationsSentToDLQ Sum for the failing rule's RuleName.
Cause 3: "Error: Check timed out after 60.0s" — the SQS target behind a rule on {"source":["aws.cloudwatch"],"detail-type":["CloudWatch Alarm Configuration Change"]} is empty after PutMetricAlarm (create), PutMetricAlarm (update) and DeleteAlarms.

## Steps to Reproduce
(1) Create two buses; put a bus policy on the destination that denies events:PutEvents for the forwarding role; PutTargets an event-bus target with that RoleArn on a rule of the source bus; PutEvents a matching entry; read the destination bus's SQS consumer — the envelope arrives. (2) Configure a rule whose target always fails with a DeadLetterConfig, PutEvents, then read AWS/Events InvocationsSentToDLQ — no datapoint. (3) PutRule on the CloudWatch Alarm Configuration Change detail-type with an SQS target, then PutMetricAlarm twice and DeleteAlarms, and poll — empty.
