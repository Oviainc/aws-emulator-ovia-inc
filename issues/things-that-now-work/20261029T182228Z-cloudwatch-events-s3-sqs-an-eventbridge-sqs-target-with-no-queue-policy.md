Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed
Issue-Key: cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed|_drain#30|Timeout|resp = sqs.receive_message(
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# An EventBridge SQS target with no queue policy neither delivers nor dead-letters, and FailedInvocations is never counted

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: the failure path of an unauthorized SQS target is entirely absent — no dead-letter message and no FailedInvocations datapoint, and nothing changes once the queue policy is fixed.
AWS: when a rule's SQS target has no resource policy statement allowing sqs:SendMessage for the service principal events.amazonaws.com, the invocation fails: the envelope lands in the target's configured DeadLetterConfig queue with the message attributes RULE_ARN, TARGET_ARN, ERROR_CODE and ERROR_MESSAGE, and EventBridge increments FailedInvocations in the AWS/Events namespace dimensioned by RuleName. Once the queue policy grants the principal, the next matching event is delivered to the target queue and FailedInvocations stops increasing.
Emulator: neither leg is observable — the dead-letter queue stays empty, the FailedInvocations metric never reports, and the check ends "Check timed out after 60.0s".
Why: a client's only two signals that a target is misconfigured (the DLQ and the FailedInvocations alarm) are both silent, so a broken rule looks identical to an idle one; and the repair leg cannot be confirmed either.
Fix: the EventBridge target invoker must authorize against the destination's resource policy, route the failed invocation to the target's DeadLetterConfig with the four documented message attributes, and emit the AWS/Events FailedInvocations metric.
Traces: cloudwatch__events__s3__sqs.xsvc-s3-2-eventbridge-target-without-queue-policy-dead-letters-and-counts-failures-until-fixed

## Evidence
"Error: Check timed out after 60.0s" — after a matching PutEvents against a rule whose SQS target has no events.amazonaws.com SendMessage grant, the target's DeadLetterConfig queue is empty and AWS/Events FailedInvocations for that RuleName reports no datapoint. Expected: one DLQ message carrying RULE_ARN/TARGET_ARN/ERROR_CODE/ERROR_MESSAGE, plus a non-zero FailedInvocations Sum.

## Steps to Reproduce
Create an SQS target queue with no queue policy, a DLQ with a policy that allows events.amazonaws.com, PutRule + PutTargets with DeadLetterConfig pointing at the DLQ, PutEvents a matching entry, and poll the DLQ and AWS/Events FailedInvocations. Both stay empty. Then add the SendMessage grant to the target queue and PutEvents again — the target queue stays empty as well.
