Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq dynamodb__lambda__sqs.sqs-fifo-esm-partial-batch-response-deletes-a-later-same-group-success-despite-an-earlier-failure
Issue-Key: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery|_assert_absent_for#30|AssertionError|assert not seen, f"{description} must NOT have happened, but observed: {seen!r}"
Issue-Key: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq|_poll_for_denial#43|Timeout|time.sleep(1)
Issue-Key: dynamodb__lambda__sqs.sqs-fifo-esm-partial-batch-response-deletes-a-later-same-group-success-despite-an-earlier-failure|run#47|Timeout|time.sleep(3)
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# dynamodb__lambda__sqs: a queue-policy Deny does not stop event-source-mapping delivery, and the revoked-permission/FIFO partial-batch scenarios never settle (3 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
Three net-new root causes in this service. The open issue 20261029T180026Z-dynamodb-lambda-sqs-sqs-and-dynamodb-stream-event-source-mappings-never.md covers two other checks of this suite (stream filter criteria and KEYS_ONLY views); it does not carry these replay keys and does not describe queue-policy evaluation or FIFO partial-batch deletion.

Cause 1 of 3: the queue's resource policy is ignored by the event-source-mapping poller.
AWS: an SQS event source mapping polls the queue with the FUNCTION's execution role. The queue's `Policy` attribute is a resource-based policy and an explicit Deny in it overrides any identity Allow, so a Deny on sqs:ReceiveMessage naming that role means the poller receives nothing: the message stays on the queue, the function is never invoked, and nothing downstream is written. Smithy models the attribute (SetQueueAttributes, QueueAttributeName `Policy`) but not its evaluation; the override rule is AWS's documented policy-evaluation order.
Emulator: the Deny is accepted and then ignored — the message is delivered, the function runs, and the item it writes shows up downstream. The check observes the forbidden side effect {'pk': {'S': '<marker>'}, 'source': {'S': '<source queue ARN>'}}.
Why: a resource policy is the control used to fence a queue off from a consumer; if it is stored but not evaluated, an emulator reports delivery that real AWS suppresses, so an access regression that would break in production passes locally.
Fix: policy evaluation in the SQS queue state/attribute layer and in the ESM poller's authorization path — the poller's ReceiveMessage must be refused when the queue policy explicitly denies the function role.
Traces: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery.

Cause 2 of 3: the revoked-downstream-permission scenario never reaches a verdict.
AWS: when a mapping's function loses the downstream grant, the handler's call is refused with the IAM implicit-deny message ("User: <assumed-role session ARN> is not authorized to perform: <action> on resource: <resource ARN> because no identity-based policy allows the <action> action"), the invocation fails, and after the queue's maxReceiveCount the message is moved to the configured dead-letter queue. All of that is observable within the recording's budget.
Emulator: the check reaches no assertion at all — it is killed by the harness: "Error: Check timed out after 60.0s". Nothing about the denial or the DLQ hand-off is produced in 60 s. Recorded behavior only for the budget itself; the AWS-side sequence above is the documented behavior the check asserts.
Why: a client cannot tell the emulator apart from a hung dependency; retry/DLQ wiring and the exact authorization message are both unverifiable.
Fix: the SQS event-source-mapping runtime (poller, retry and redrive-to-DLQ accounting) together with the authorization path named in cause 1.
Traces: dynamodb__lambda__sqs.revoked-downstream-permission-is-denied-with-the-exact-authz-message-and-the-message-lands-in-the-dlq.

Cause 3 of 3: the FIFO partial-batch-response scenario never reaches a verdict.
AWS: for a FIFO SQS mapping with FunctionResponseTypes=["ReportBatchItemFailures"], Lambda deletes every message the handler did not report in batchItemFailures, including a later message in the SAME MessageGroupId that succeeded after an earlier failure in that group, and redelivers only the reported ones.
Emulator: "Error: Check timed out after 60.0s" — no batch is processed to a verdict inside the budget, so neither the deletion set nor the redelivery set is observable.
Why: partial-batch reporting is the mechanism that keeps a FIFO consumer from reprocessing a whole batch; if the scenario never settles, the emulator cannot be used to validate it.
Fix: the SQS event-source-mapping runtime's partial-batch-response handling for FIFO queues (message-group ordering plus the delete/redeliver decision).
Traces: dynamodb__lambda__sqs.sqs-fifo-esm-partial-batch-response-deletes-a-later-same-group-success-despite-an-earlier-failure.

## Evidence
Cause 1 — expected: with an explicit Deny on sqs:ReceiveMessage for the function role in the queue's Policy, the ESM delivers nothing and the downstream item is never written. Actual: the item appears — "AssertionError: delivery of the message while the queue policy denies ReceiveMessage must NOT have happened, but observed: {'pk': {'S': 'sim2real-xsvc-msg-suppressed-...'}, 'source': {'S': 'arn:aws:sqs:us-west-2:000000000000:sim2real-xsvc-q-esm-...'}}" (the ids are per-run values; the requirement is that nothing is observed).

Cause 2 — expected: the denied downstream call is refused with the IAM implicit-deny message and the message lands on the DLQ after maxReceiveCount. Actual: "Error: Check timed out after 60.0s" — no verdict.

Cause 3 — expected: a later same-group success is deleted despite an earlier failure in that group, and only the reported failure is redelivered. Actual: "Error: Check timed out after 60.0s" — no verdict.

## Steps to Reproduce
1. Create an SQS queue, a Lambda function and an enabled SQS event source mapping between them; have the handler write a marker item downstream.
2. Cause 1: SetQueueAttributes with a Policy containing an explicit Deny on sqs:ReceiveMessage for the function's execution role, then send a message. The emulator still invokes the function and the marker item appears.
3. Cause 2: revoke the handler's downstream grant, configure a redrive policy to a DLQ, send a message and wait. The check is killed at 60 s with no verdict.
4. Cause 3: use a FIFO source queue with FunctionResponseTypes=["ReportBatchItemFailures"] and a batch in which an earlier message of a group fails and a later one succeeds. The check is killed at 60 s with no verdict.
5. Replay the three check ids in trace_ids.
