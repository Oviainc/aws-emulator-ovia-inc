Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt
Issue-Key: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt|run#22|AssertionError|assert _depth(reads[-1]) >= 1, (
Issue-Commit: 323c298260f641866db820e53cda367b812962dd

# kms__lambda__sqs: an SQS event source mapping drains an SSE-KMS queue whose role has no kms:Decrypt instead of being refused KMS.AccessDeniedException and leaving the message queued

> _Note: this issue was identified against the code base at commit `323c298260f641866db820e53cda367b812962dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
1 net-new root cause.

Cause 1 of 1: the SSE-KMS receive path of a Lambda event-source-mapping poll is not gated on the poller's kms:Decrypt, and the undecryptable message is consumed out of the queue instead of staying in it. Failing check: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt.

AWS: a queue whose KmsMasterKeyId names a customer CMK is decrypted by SQS on behalf of the ReceiveMessage caller. When that caller — here the Lambda event source mapping polling with the function's execution role — holds no kms:Decrypt on the CMK, SQS refuses the receive rather than returning the message. Smithy (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json): ReceiveMessage errors include KmsAccessDenied, errorKind client, httpError 400, awsQueryError code "KMS.AccessDeniedException"; the sibling KMS classes (KMS.DisabledException, KMS.InvalidStateException, KMS.NotFoundException) are modelled on the same operation. The consequences of that refusal are recorded behavior: the poller obtains nothing, so the function is never invoked, and the message is not removed — the queue keeps reporting it through ApproximateNumberOfMessages (or ApproximateNumberOfMessagesNotVisible while an attempt holds it) for the whole hold window, which is longer than two visibility timeouts; granting kms:Decrypt afterwards delivers that same message. The recording's own control leg proves the mapping machinery is otherwise live: an SSE-SQS (SqsManagedSseEnabled) twin queue on the same function, through the same kind of mapping, delivers its message to the handler.

Emulator: the message sent to the SSE-KMS queue disappears from it within seconds. Both depth counters read zero on four consecutive reads one second apart — ApproximateNumberOfMessages "0" and ApproximateNumberOfMessagesNotVisible "0" — while the role attached to the mapping carries no kms:Decrypt grant for the queue's CMK. So the receive path makes no KMS authorization decision at all (no KMS.AccessDeniedException is ever produced) and the mapping drains the queue it must not be able to read.

Why: a client that deliberately scopes an execution role without kms:Decrypt relies on the SSE-KMS queue holding its message — nothing is processed, the message remains visible and eventually follows the queue's redrive policy, and the operator sees KMS.AccessDeniedException on the mapping. Against the emulator the message is silently destroyed: the data is gone, no DLQ/redrive ever sees it, and a key-policy or role-policy regression that would halt processing on AWS is indistinguishable from correct operation. The reverse check (grant kms:Decrypt, message now delivers) also becomes untestable, so encryption-scoped least-privilege designs cannot be validated.

Fix: the SQS ReceiveMessage handler's server-side-encryption authorization — it must evaluate the caller's kms:Decrypt against the CMK named by the queue's KmsMasterKeyId (key policy plus the caller's identity policy) before any message is handed out or its visibility/receipt state mutated, and refuse with the modelled KmsAccessDenied (HTTP 400, code KMS.AccessDeniedException) otherwise — and the Lambda event-source-mapping poller, which must treat that refusal as a failed poll: no invocation, no delete, message left in the queue.

Related but not owning: open issues 20261029T072249Z-kms-lambda-sqs-an-enabled-sqs-event-source.md and 20261024T102105Z-kms-lambda-sqs-lambda-event-source-mappings-never-deliver-sqs-messages.md describe the mapping never invoking at all; this run's control leg does deliver, so the surviving divergence is the missing KMS gate and the consumed message, not a dead mapping. Neither issue is marked as owning this check.

Traces: kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt.

## Evidence
Check kms__lambda__sqs.esm-on-sse-kms-queue-withholds-invocation-until-the-role-can-decrypt, verdict mismatch.

Expected (AWS): the message sent to the SSE-KMS queue stays in the queue for the whole hold (>= 2 visibility timeouts) because every ESM receive is refused HTTP 400 / KMS.AccessDeniedException (Smithy: ReceiveMessage error KmsAccessDenied, awsQueryError KMS.AccessDeniedException); the function log never carries the marker; the SSE-SQS twin's message does reach the function.

Actual (emulator): the twin leg delivered, then GetQueueAttributes on the SSE-KMS queue read empty four times in a row:
  AssertionError: the undecryptable message must stay in the queue throughout the 12s hold; queue depth read zero 4 times running: [{'ApproximateNumberOfMessages': '0', 'ApproximateNumberOfMessagesNotVisible': '0'}, {'ApproximateNumberOfMessagesNotVisible': '0', 'ApproximateNumberOfMessages': '0'}, {'ApproximateNumberOfMessagesNotVisible': '0', 'ApproximateNumberOfMessages': '0'}, {'ApproximateNumberOfMessagesNotVisible': '0', 'ApproximateNumberOfMessages': '0'}]

No KMS refusal of any kind was produced on the receive path; the message was removed from the queue while the poller role had no kms:Decrypt on the queue's CMK.

## Steps to Reproduce
1. KMS: create a customer CMK whose key policy does NOT grant kms:Decrypt to the role used below.
2. SQS: create queue A with Attributes {"KmsMasterKeyId": <that CMK>} and queue B with {"SqsManagedSseEnabled": "true"} (the control).
3. IAM: create a role trusted by lambda.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on both queues and NO kms:Decrypt.
4. Lambda: create a function with that role; CreateEventSourceMapping(EventSourceArn=A, BatchSize=1) and the same for B; wait for both mappings to report Enabled.
5. SendMessage a distinct marker body to B, then to A.
6. Confirm the control: B's marker appears in the function's log.
7. Poll GetQueueAttributes(A, [ApproximateNumberOfMessages, ApproximateNumberOfMessagesNotVisible]) once a second for at least two visibility timeouts.
Expected: A's counters keep summing to >= 1 and the function log never carries A's marker; every ESM receive on A is refused HTTP 400 KMS.AccessDeniedException.
Observed: A's counters both read "0" within seconds and stay zero — the message was consumed and deleted with no KMS authorization check.
