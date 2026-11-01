Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs
Issue-Key: events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions|_drain#17|Timeout|return out
Issue-Key: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts|_poll_until#31|Timeout|last = probe()
Issue-Key: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll|_drain#22|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs|_collect#10|Timeout|return False, list(seen.values())
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions|_collect#10|Timeout|return False, list(seen.values())
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs|_collect#9|Timeout|return False, list(seen.values())
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# events__lambda__sqs: 6 net-new causes — EventBridge never dead-letters a failed Lambda delivery, an under-privileged ESM role is not held off the queue, and Lambda durable-execution status events are never published

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Six distinct root causes, all ending in `Error: Check timed out after 60.0s` — the observable each check waits for never arrives.

Cause 1 of 6 — deleted Lambda target dead-letters as NO_PERMISSIONS: AWS: recorded behavior only (EventBridge's model carries Target.DeadLetterConfig but no failure-classification semantics). When a rule's Lambda target no longer exists, AWS dead-letters the event to the target's DLQ with the failure attributes naming the NO_PERMISSIONS error code (plus the rule and target ARNs). Emulator: nothing is delivered to the DLQ; the check times out.
Failing check: events__lambda__sqs.events-delivery-and-failure-deleted-lambda-target-dead-letters-as-no-permissions.

Cause 2 of 6 — event-age limit dead-letters without further attempts: AWS: recorded behavior only. With RetryPolicy.MaximumEventAgeInSeconds exceeded, AWS stops retrying and dead-letters the event once, and no further invocation attempt is made after the age limit. Emulator: no DLQ message and no attempt accounting; the check times out.
Failing check: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts.

Cause 3 of 6 — ESM role without SQS permissions must not poll: AWS: recorded behavior only. An event-source mapping whose execution role lacks sqs:ReceiveMessage/DeleteMessage does not consume the queue — the messages stay visible and the function is never invoked. Emulator: the check never reaches that verdict within 60s.
Failing check: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll.

Cause 4 of 6 — durable execution TIMED_OUT status event: AWS: recorded behavior only. A durable execution that outlives DurableConfig.ExecutionTimeout emits a TIMED_OUT status-change event (never SUCCEEDED) whose endTimestamp is at least ExecutionTimeout after its startTimestamp, delivered by a default-bus rule to SQS. Emulator: no status-change event is ever published, so the queue stays empty and the check times out.
Failing check: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs.

Cause 5 of 6 — status-filtered rules must not cross-deliver: AWS: recorded behavior only. A rule whose pattern matches detail.status FAILED receives nothing from succeeding durable executions, while a SUCCEEDED-only rule on the same function and bus receives each one; a failing execution then reaches the FAILED rule and not the SUCCEEDED one. Emulator: neither rule receives anything, so the filter cannot be graded; the check times out.
Failing check: events__lambda__sqs.xsvc-lambda-1-durable-failed-status-rule-rejects-succeeded-executions.

Cause 6 of 6 — StopDurableExecution STOPPED event: AWS: recorded behavior only. StopDurableExecution on a RUNNING execution emits a STOPPED status-change event to the default bus whose endTimestamp equals the StopTimestamp the call returned. Emulator: no event arrives at the rule's queue; the check times out.
Failing check: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs.

Why: failed EventBridge deliveries vanish silently (no dead letter, no error code), so a client cannot distinguish a delivered event from a lost one or learn why delivery failed; an under-privileged ESM role's effect on polling is unobservable; and nothing can be built on Lambda durable-execution status events — terminal state, timing and per-status routing are all invisible, so a workflow that timed out looks identical to one still running.

Fix: the EventBridge target-dispatch failure path (DeadLetterConfig delivery with the NO_PERMISSIONS/retry-exhaustion attributes and RetryPolicy age enforcement), the SQS event-source-mapping poller's role authorization, and the Lambda durable-execution engine's publication of status-change events to the default bus. Related open issues 20261027T191349Z-events-lambda-sqs-lambda-durable-execution-status-change-events-are-never.md and 20261029T182228Z-events-lambda-sqs-eventbridge-sqs-targets-ignore-the.md describe neighbouring defects but carry none of these six checks.

Traces: the six check ids listed above.

## Evidence
All six failures report `Error: Check timed out after 60.0s` (replay verdicts [mismatch]).
- deleted-lambda-target: expected one DLQ message with the NO_PERMISSIONS error-code attribute; actual: empty DLQ.
- event-age-limit: expected exactly one dead letter and no attempt after MaximumEventAgeInSeconds; actual: nothing.
- lambda-role-missing-sqs-permissions-blocks-poll: expected messages to stay visible and the function never invoked; actual: no verdict in 60s.
- durable TIMED_OUT: expected a TIMED_OUT status event with endTimestamp >= startTimestamp + ExecutionTimeout at the rule's queue; actual: queue empty.
- durable FAILED-only rule: expected the SUCCEEDED-only queue to receive each success and the FAILED-only queue nothing until a failure; actual: both queues empty.
- durable STOPPED: expected a STOPPED event whose endTimestamp equals the StopDurableExecution StopTimestamp; actual: nothing delivered.

## Steps to Reproduce
1. Create a rule with a Lambda target and a DLQ; delete the target function and PutEvents a matching event — AWS dead-letters with error code NO_PERMISSIONS.
2. Configure RetryPolicy.MaximumEventAgeInSeconds on a failing target and confirm exactly one dead letter and no further attempts.
3. CreateEventSourceMapping with an execution role lacking sqs:ReceiveMessage and send a message: the queue is not drained and the function is not invoked.
4. Start a durable execution with DurableConfig.ExecutionTimeout shorter than the handler's wait and a default-bus rule to SQS; expect a TIMED_OUT status event.
5. Create SUCCEEDED-only and FAILED-only rules on the same function and run one succeeding and one failing execution.
6. StopDurableExecution on a RUNNING execution and expect a STOPPED event whose endTimestamp equals the returned StopTimestamp.
7. Against the emulator each of these waits to the 60s harness budget with nothing observed.
