Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied
Issue-Key: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied|_drain#34|Timeout|resp = sqs.receive_message(
Issue-Commit: ff73d0b7325d7e3e22aa47ea31b35af4c9a9d063

# cloudwatch__events__sqs: a bus-to-bus forward denied by the destination bus policy produces nothing — no NO_PERMISSIONS dead letter and no AWS/Events TriggeredRules datapoint, so the FailedInvocations contract is unobservable

> _Note: this issue was identified against the code base at commit `ff73d0b7325d7e3e22aa47ea31b35af4c9a9d063`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets, PutEvents

## Description
One net-new root cause. The failing check was reported neither ALREADY FILED nor OWNED by an open issue, so no existing work item carries it.

=== Cause 1 of 1: an event-bus target whose destination bus policy denies the forwarding role is never recorded — no dead letter, no TriggeredRules, and FailedInvocations cannot be shown to stay 0 ===
AWS: Smithy: PutTargets.Targets -> Target{Arn required, RoleArn, DeadLetterConfig} — an event-bus target ARN is forwarded under the target's RoleArn; the model says nothing about failure accounting, so the accounting itself is recorded behavior only (AWS documents it in eb-monitoring and eb-rule-dlq). Behavior: a rule on the source bus with an event-bus target pointing at a second bus, under a role that IS otherwise entitled (events:PutEvents on the destination bus ARN), forwards matched events to the destination bus, where a rule with the same pattern delivers them to its SQS target. When the DESTINATION bus's resource policy denies that forwarding role, the forward fails and AWS records it in three places at once: (a) the rule's AWS/Events TriggeredRules datapoint for the source rule still counts the match (the rule DID trigger), (b) the event is written to the target's DeadLetterConfig queue with ERROR_CODE = NO_PERMISSIONS, and (c) AWS/Events FailedInvocations for that rule stays 0 — a permissions refusal is NOT a failed invocation, and FailedInvocations is only emitted when non-zero, so "no datapoints" and "a zero datapoint" are different observations the check distinguishes. A twin control rule on the source bus with an undeniable SQS target receives the same event at the same instant, so an absence at the destination is attributable to the Deny and not to warm-up.
Emulator: nothing observable happens. The check reports "Error: Check timed out after 60.0s": its own bounded helper re-publishes a fresh nonce until one reaches the queue to establish each delivery path before anything is broken, and that establishment never completes — PutRule/PutTargets all reported FailedEntryCount == 0, yet no forwarded event reaches the destination bus's queue, so neither the dead letter nor either AWS/Events series is ever produced and none of the three assertions above is reached. Hypothesis: the rule target-delivery path does not forward to an event-bus target ARN at all, and the failure-accounting (dead letter + AWS/Events metrics) that would record a refused forward does not exist.
Why: a client that fans events across buses and watches FailedInvocations/TriggeredRules plus a dead-letter queue to detect a mis-scoped destination bus policy is told nothing at all: the event vanishes, no metric moves, no dead letter appears. The specific distinction AWS draws — a permissions refusal dead-letters as NO_PERMISSIONS while FailedInvocations stays 0 — is the signal that tells an operator the destination POLICY is wrong rather than the destination being down, and it cannot be observed.
Fix: rule target delivery and failure accounting in the eventbridge service — forward to an event-bus target under the target's RoleArn, evaluate the destination bus's resource policy on that forward, write a refused forward to the target's DeadLetterConfig queue with ERROR_CODE NO_PERMISSIONS, and publish the AWS/Events TriggeredRules / (withheld) FailedInvocations datapoints for the rule into CloudWatch. Related open issue 20261025T002357Z-cloudwatch-events-sqs-cloudwatch-emits-no-alarm-events-to-eventbridge-no.md covers CloudWatch ALARM events reaching EventBridge and does not carry this check.
Traces: cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied

## Evidence
From the behavioral suite replay (168/319 passed; cloudwatch__events__sqs 0/5, and this is the one of the five the suite marked neither ALREADY FILED nor OWNED).

cloudwatch__events__sqs.failed-invocations-records-the-bus-forwarding-the-destination-policy-denied [mismatch]:
  Error: Check timed out after 60.0s
  Setup that completed without error: CreateEventBus (source) and CreateEventBus (destination); three SQS queues (control / destination consumer / DLQ) each with a policy allowing events.amazonaws.com SendMessage under ArnLike aws:SourceArn arn:*:events:<region>:<account>:rule/*; PutRule on the destination bus + PutTargets the destination queue (FailedEntryCount == 0); PutRule on the source bus + PutTargets the control queue (FailedEntryCount == 0); an IAM role trusting events.amazonaws.com with inline events:PutEvents on the destination bus ARN; PutRule on the source bus + PutTargets the event-bus target.
  The check then calls _emit_until_delivered(), which re-publishes a fresh nonce and drains for 8 s per attempt until one arrives, raising "<description> not established within <timeout>s: no emitted event reached <queue>" on its deadline. It never got past establishing the delivery path, so the three graded observations — TriggeredRules > 0 for the forwarding rule, a dead letter with ERROR_CODE NO_PERMISSIONS, FailedInvocations summing to 0 with no datapoints — were never produced.

## Steps to Reproduce
1. CreateEventBus <src>; CreateEventBus <dst> (note its EventBusArn).
2. Create three SQS queues (control, destination consumer, DLQ), each with a policy allowing Principal {Service: events.amazonaws.com} sqs:SendMessage with Condition ArnLike aws:SourceArn = arn:*:events:<region>:<account>:rule/*.
3. On <dst>: PutRule with pattern {"source": ["sim2real.nd.busdeny"], "detail-type": ["BusForwardProbe"]} State=ENABLED; PutTargets the destination consumer queue.
4. On <src>: PutRule with the same pattern; PutTargets the control queue (the twin control).
5. Create an IAM role trusting events.amazonaws.com with an inline policy allowing events:PutEvents on <dst>'s ARN. On <src>: PutRule with the same pattern; PutTargets {Arn: <dst> bus ARN, RoleArn: that role, DeadLetterConfig: {Arn: the DLQ ARN}}.
6. PutEvents on <src> with that source/detail-type until an event reaches the destination consumer queue (establish the forward; a fresh rule drops events until live and a drop is never retried).
7. PutPermission on <dst> denying that forwarding role events:PutEvents, then PutEvents on <src> again. Expect: the control queue receives the event; the DLQ receives it with ERROR_CODE = NO_PERMISSIONS; GetMetricStatistics on AWS/Events TriggeredRules for the forwarding rule shows a non-zero Sum; GetMetricStatistics on AWS/Events FailedInvocations for that rule sums to 0 (and emits no datapoint); the destination consumer queue receives nothing.
