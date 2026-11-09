Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs__stepfunctions.events-putevents-resources-carry-run-arns-and-rejected-entry-fails-task events__sqs__stepfunctions.xsvc-sfn-2-dlq-missing-permission
Issue-Key: events__sqs__stepfunctions.events-putevents-resources-carry-run-arns-and-rejected-entry-fails-task|run#19|AssertionError|assert set(event["resources"]) == {good_exec_arn, sm_arn}, (
Issue-Key: events__sqs__stepfunctions.xsvc-sfn-2-dlq-missing-permission|_poll_until#39|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# EventBridge: 2 net-new causes — PutEvents from a Step Functions task delivers an empty 'resources' array, and no AWS/Events InvocationsSentToDlq datapoint is recorded

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two independent defects grounded in two failing checks under the `events__sqs__stepfunctions` heading.

Cause 1 of 2: an event put by a Step Functions task carries no `resources`.
AWS: when a Step Functions `Task` state calls EventBridge `PutEvents`, the delivered event's `resources` array is populated by the service with the calling execution's ARN and its state machine's ARN (exactly those two entries), so a consumer can attribute the event to the execution that produced it. A rejected entry in the same PutEvents batch fails the task.
Emulator: the delivered event's `resources` is the empty array `[]` — the execution and state machine ARNs are never injected.
Why: a consumer cannot attribute an event to its producing execution; correlation, filtering on `resources` and auditing all break, and a rule whose pattern constrains `resources` never matches.
Fix: the PutEvents handler's envelope construction when the caller is a Step Functions task integration (populate `resources` with the execution ARN and the state machine ARN), or the Step Functions aws-sdk/optimized EventBridge integration that must supply them.
Traces: events__sqs__stepfunctions.events-putevents-resources-carry-run-arns-and-rejected-entry-fails-task.

Cause 2 of 2: no `AWS/Events` `InvocationsSentToDlq` datapoint is published when a target invocation is dead-lettered for a missing permission.
AWS: when a rule's target invocation fails because the target role lacks the required permission and the target has a DeadLetterConfig, EventBridge dead-letters the event AND publishes an `AWS/Events` `InvocationsSentToDlq` datapoint dimensioned by RuleName (and EventBusName for a custom bus).
Emulator: the metric is never published — the check polls for 150s and `InvocationsSentToDlq` stays `0`.
Why: a client that alarms on dead-lettered invocations gets no signal at all, so a permission misconfiguration silently drains events with nothing to detect it.
Fix: CloudWatch metric emission in the EventBridge dead-letter path (`AWS/Events` InvocationsSentToDlq, with RuleName/EventBusName dimensions).
Traces: events__sqs__stepfunctions.xsvc-sfn-2-dlq-missing-permission.

## Evidence
Cause 1: `AssertionError: the delivered event's resources must be exactly the execution ARN and the state machine ARN; got []`.
Cause 2: `AssertionError: AWS/Events InvocationsSentToDlq records a datapoint for the rule not met within 150.0s. Last result: 0`.

## Steps to Reproduce
1. Cause 1: create a state machine whose Task calls EventBridge PutEvents (with one valid and one rejected entry), subscribe a rule + SQS target to the valid entry, StartExecution, and read the delivered event's `resources`.
2. Cause 2: create a rule with a target whose role lacks the required permission and a DeadLetterConfig; PutEvents a matching event; read the `AWS/Events` `InvocationsSentToDlq` metric for the rule.
3. Replay both checks in trace_ids.
