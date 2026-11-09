Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.a-failing-sibling-targets-retries-do-not-redeliver-to-the-target-that-succeeded events__lambda__sqs.dotnet-eventbridge-rule-event-fields-reach-handler-via-onsuccess-destination events__lambda__sqs.java-eventbridge-rule-event-reaches-the-handler events__lambda__sqs.ruby-eventbridge-rule-matched-event-reaches-the-handler events__lambda__sqs.sqs-esm-visibility-timeout-extends-during-processing events__lambda__sqs.xsvc-lambda-1-durable-status-change-running-then-succeeded-reaches-sqs
Issue-Key: events__lambda__sqs.a-failing-sibling-targets-retries-do-not-redeliver-to-the-target-that-succeeded|_poll_until#44|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: events__lambda__sqs.dotnet-eventbridge-rule-event-fields-reach-handler-via-onsuccess-destination|_receive#17|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__lambda__sqs.java-eventbridge-rule-event-reaches-the-handler|run#19|Timeout|time.sleep(2.0)
Issue-Key: events__lambda__sqs.ruby-eventbridge-rule-matched-event-reaches-the-handler|_send_until_delivered#14|TimeoutError|raise TimeoutError("no matched event ever arrived: the run budget ended first; not graded")
Issue-Key: events__lambda__sqs.sqs-esm-visibility-timeout-extends-during-processing|run#14|AssertionError|assert stolen is None, (
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-status-change-running-then-succeeded-reaches-sqs|_await_rule_live#10|AssertionError|raise AssertionError(f"rule never delivered a warm-up {status} event for {fn_name!r} within 240s")
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# Lambda/EventBridge: 4 net-new causes — no AWS/Events invocation metrics, non-Python runtimes never receive the matched event, SQS ESM does not extend visibility, and durable-execution status changes never reach the rule

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Four independent defects grounded in six failing checks under the `events__lambda__sqs` heading.

Cause 1 of 4: EventBridge publishes no `AWS/Events` invocation metrics.
AWS: each rule match produces exactly one invocation per target, and EventBridge publishes the `AWS/Events` metric `InvocationAttempts` (dimensioned by RuleName, and by EventBusName for a custom bus) with one datapoint per attempt; a sibling target that fails is retried without redelivering to the target that already succeeded, which is observable as attempts counting per target rather than per rule.
Emulator: the metric is never published — `InvocationAttempts` for the rule stays `0` for 300s, so the per-target retry isolation cannot be observed at all.
Why: a client monitoring or alarming on EventBridge delivery (attempts, retries) sees a permanently empty metric and cannot distinguish 'no events' from 'metrics not emitted'.
Fix: the EventBridge target-invocation path's CloudWatch metric emission (`AWS/Events` InvocationAttempts and the sibling delivery metrics), with the RuleName/EventBusName dimensions.
Traces: events__lambda__sqs.a-failing-sibling-targets-retries-do-not-redeliver-to-the-target-that-succeeded.

Cause 2 of 4: an EventBridge-matched event never reaches a handler on the .NET, Java or Ruby Lambda runtimes.
AWS: a function on any supported runtime receives the matched EventBridge envelope as its event payload; the .NET variant additionally records the envelope's fields through its OnSuccess destination.
Emulator: all three runtimes deliver nothing — the .NET and Java checks exhaust their budget waiting for the handler/destination (`Check timed out after 180.0s`) and the Ruby check ends `TimeoutError: no matched event ever arrived`. The Python-runtime equivalents in the same suite pass, so the divergence is runtime-specific, not EventBridge-wide.
Why: a client cannot run any non-Python function: the invocation never completes, nothing is logged and no error is returned.
Fix: the Lambda runtime support for dotnet/java/ruby (the runtime launcher and its event delivery to the handler), and the OnSuccess destination recording for those runtimes.
Traces: events__lambda__sqs.dotnet-eventbridge-rule-event-fields-reach-handler-via-onsuccess-destination; events__lambda__sqs.java-eventbridge-rule-event-reaches-the-handler; events__lambda__sqs.ruby-eventbridge-rule-matched-event-reaches-the-handler.

Cause 3 of 4: an SQS event-source mapping does not extend a message's visibility reservation while it is held.
AWS: once an ESM receives a message it keeps the message invisible for as long as it holds it — including across the mapping's `MaximumBatchingWindowInSeconds` window — so an independent `ReceiveMessage` by another consumer does NOT see the message before the ESM is done with it, even when the queue's own `VisibilityTimeout` is shorter than the batching window.
Emulator: the reservation is not extended. An independent `receive_message` saw the message again after only the queue's 5s `VisibilityTimeout`, while the ESM's 20s batching window had not elapsed.
Why: messages are processed more than once by design-conformant consumers — a duplicate-delivery bug a client cannot work around, and the at-least-once guarantee the ESM is supposed to manage is broken.
Fix: the SQS event-source-mapping poller's visibility management (extend/renew the message's visibility for the duration the mapping holds it, including the batching window).
Traces: events__lambda__sqs.sqs-esm-visibility-timeout-extends-during-processing.

Cause 4 of 4: Lambda durable-execution status-change events never reach a subscribed rule.
AWS: a durable execution publishes status-change events (RUNNING, then SUCCEEDED) onto the default bus; a rule matching them delivers each to its target.
Emulator: nothing arrives — the rule never delivered even the warm-up `SUCCEEDED` event for the durable execution within 240s.
Why: a client cannot observe durable-execution lifecycle at all; no delivery, no dead letter, no error.
Fix: the Lambda durable-execution state machine's event emission onto the default event bus.
Traces: events__lambda__sqs.xsvc-lambda-1-durable-status-change-running-then-succeeded-reaches-sqs.

Note: open issues 20261101T210635Z-events-lambda-sqs-6-net-new-causes-eventbridge.md, 20261105T094343Z-events-lambda-sqs-the-lambda-python-runtime-provides.md and 20261107T042814Z-events-lambda-sqs-an-api-destination-dead-letter.md carry other events__lambda__sqs checks; none of them carries the six replay keys above.

## Evidence
Cause 1: `AssertionError: AWS/Events InvocationAttempts for sim2real-nd-sib-rule-f40815df8f4f reached 2 — one invocation per target on a single rule match not met within 300s. Last result: 0` (rule name is per-run).
Cause 2: `Error: Check timed out after 180.0s` (dotnet and java); `TimeoutError: no matched event ever arrived: the run budget ended first; not graded` (ruby).
Cause 3: `AssertionError: an independent receive_message call saw the message after only 5s (the queue's VisibilityTimeout), even though the ESM's 20s batching window had not elapsed -- the visibility reservation was not extended while the message waited to be batched`.
Cause 4: `AssertionError: rule never delivered a warm-up SUCCEEDED event for 'sim2real-xsvc-durev-d854dce091f4' within 240s` (execution name is per-run).

## Steps to Reproduce
1. Replay the six checks named in trace_ids.
2. Cause 1: one rule, two targets (one failing, one succeeding), PutEvents once, then read the `AWS/Events` `InvocationAttempts` metric for the rule.
3. Cause 2: create functions with the dotnet, java and ruby runtimes, wire each as an EventBridge rule target, PutEvents a matching event and wait for the handler's effect / OnSuccess destination record.
4. Cause 3: create a queue with `VisibilityTimeout=5`, an ESM with `MaximumBatchingWindowInSeconds=20`, send one message, and call `receive_message` from an independent consumer at ~6s.
5. Cause 4: start a durable execution and poll a rule subscribed to its status-change events.
