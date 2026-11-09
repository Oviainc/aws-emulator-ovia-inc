Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__sqs.throttled-lambda-target-is-one-successful-attempt-with-no-retry-and-no-dlq
Issue-Key: events__sqs.throttled-lambda-target-is-one-successful-attempt-with-no-retry-and-no-dlq|_poll_until#42|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# EventBridge: no AWS/Events InvocationAttempts datapoint for a throttled Lambda target's single accepted asynchronous invoke

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: the `AWS/Events` invocation metrics are not published.
AWS: when a rule's Lambda target is throttled (reserved concurrency exhausted), EventBridge's asynchronous invoke is still ACCEPTED — it counts as exactly one successful invocation attempt with no rule-level retry and nothing on the dead-letter queue, because throttling is handled inside Lambda's async path. EventBridge records that single attempt as an `AWS/Events` `InvocationAttempts` datapoint dimensioned by RuleName (and EventBusName for a custom bus), with no `InvocationsSentToDlq`/`FailedInvocations` datapoint.
Emulator: both counters stay at zero for 300s (`Last result: (0, 0)`) — no `AWS/Events` metric datapoint is published at all, so the 'one attempt, no retry, no DLQ' contract cannot be observed and cannot be distinguished from 'the event was never delivered'.
Why: a client that alarms on EventBridge delivery metrics sees a permanently empty namespace, and cannot tell an accepted-but-throttled delivery from a lost one.
Fix: CloudWatch metric emission in the EventBridge target-invocation path (`AWS/Events` InvocationAttempts, InvocationsSentToDlq, FailedInvocations, with RuleName/EventBusName dimensions).
Traces: events__sqs.throttled-lambda-target-is-one-successful-attempt-with-no-retry-and-no-dlq.

## Evidence
`AssertionError: AWS/Events InvocationAttempts for rule sim2real-nd-retry-subj-09d79b919ff3 on bus sim2real-nd-retry-bus-f294f13a3d48 recorded the single accepted async invoke not met within 300s. Last result: (0, 0)` (rule and bus names are per-run).

## Steps to Reproduce
1. Create a custom bus, a rule, and a Lambda target whose reserved concurrency is exhausted so the async invoke is throttled; configure a DLQ on the target.
2. PutEvents one matching event.
3. Read the `AWS/Events` `InvocationAttempts` and `InvocationsSentToDlq` metrics for the rule/bus — both remain 0.
4. Replay events__sqs.throttled-lambda-target-is-one-successful-attempt-with-no-retry-and-no-dlq.
