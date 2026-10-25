Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.delete-message-batch-partial-failure sqs.max-message-size
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.delete-message-batch-partial-failure|run#25|AssertionError|assert failed["d2"].get("Message") == expected_message, (
Issue-Key: sqs.max-message-size|run#14|AssertionError|assert code == "413", f"expected a 413 (request entity too large) for a 2MB body, got {code}"
Issue-Commit: a97363f812621dd47aa0891b39cb5eeebafd4acb

# sqs: DeleteMessageBatch's ReceiptHandleIsInvalid entry message is re-worded, and a request body far over the 262144-byte limit is answered ValidationException instead of a transport-level HTTP 413 (2 causes)

> _Note: this issue was identified against the code base at commit `a97363f812621dd47aa0891b39cb5eeebafd4acb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMessageBatch

## Description
Two net-new root causes in the sqs suite. Neither failing check is marked OWNED or ALREADY FILED in the replay report. Both are refusal-shape divergences on calls the emulator otherwise handles correctly.

Cause 1 of 2: the per-entry failure message of DeleteMessageBatch for a receipt handle that belongs to another queue does not use AWS's wording.
AWS: Smithy (com-amazonaws-sqs-amazonsqs) models DeleteMessageBatch with required {QueueUrl, Entries} and output {Successful, Failed} where Failed is a BatchResultErrorEntryList — a handle that is not valid for the named queue is reported as a per-ENTRY failure (the call itself is 200), not as a request-level error; the request-level errors modelled on the operation are BatchEntryIdsNotDistinct, EmptyBatchRequest, InvalidAddress, InvalidBatchEntryId, InvalidSecurity, QueueDoesNotExist, RequestThrottled, TooManyEntriesInBatchRequest and UnsupportedOperation. Recorded behavior (this check's oracle) for the entry: Code `ReceiptHandleIsInvalid` with Message template `The receipt handle "<receipt-handle>" is not valid for this queue.` — fixed text `The receipt handle "`, `" is not valid for this queue.`, with <receipt-handle> being the handle from the request, verbatim and double-quoted.
Emulator: the entry Id, the `Code` (ReceiptHandleIsInvalid) and the partial-success semantics are all correct — the valid sibling entry is reported Successful and really deleted, and the foreign-queue message really stays in its own queue — but the Message is re-worded to `The input receipt handle "<receipt-handle>" is not a valid receipt handle for this queue.` (an inserted "input" and "is not a valid receipt handle" in place of "is not valid").
Why: because this failure is per-entry rather than an exception, the Message is the only description a client gets for the entry; batch consumers that log, surface or match on it see text that corresponds to no AWS message, and a client comparing per-entry messages across the batch cannot rely on the documented template.
Fix: the message template used when DeleteMessageBatch (and the sibling ChangeMessageVisibilityBatch, which AWS words the same way) builds a BatchResultErrorEntry for an invalid/foreign receipt handle, in the sqs crate's batch entry handling. The classification is already right and must not change.
Traces: sqs.delete-message-batch-partial-failure.

Cause 2 of 2: a SendMessage request whose body is far above the native 262144-byte limit is refused by SQS field validation instead of being rejected at the HTTP transport layer.
AWS: Smithy models SendMessage.MessageBody as a plain required String with no @length bound, and the operation's modelled client errors are InvalidAddress, InvalidMessageContents, InvalidSecurity, the KMS.* family, QueueDoesNotExist, RequestThrottled and UnsupportedOperation — there is no "body too large" error shape, so this is recorded behavior only. Recorded behavior (this check's oracle): a body at exactly the native limit (262144 bytes / 256 KiB) is ACCEPTED and round-trips byte-for-byte, while a body far above it (2 MiB) is rejected by the HTTP transport itself with a raw 413 (Request Entity Too Large) — no SQS error document, so botocore surfaces Error.Code as the bare status string `413` — and the message is never enqueued. A moderate excess over a queue's configured MaximumMessageSize is a different, field-validation refusal and is covered by other checks.
Emulator: the 262144-byte positive control passes (accepted and returned unchanged), but the 2 MiB request is answered with an SQS error document carrying Code `ValidationException`, i.e. the oversized request is read and routed into field validation rather than being refused by the transport before SQS's own validation runs.
Why: a client cannot distinguish "the request never fit on the wire" from "SQS looked at my field and disliked it": the former is non-retryable at the transport layer and surfaces in SDKs/proxies/load balancers as a 413 before any service error handling, while a ValidationException is an application-level refusal with a different retry and error-mapping path. Emulating it as a service error also means an emulator silently accepts and buffers multi-megabyte bodies that AWS refuses up front. (The ValidationException code itself is the same taxonomy problem reported in open issue 20261025T065401Z-sqs-refusal-taxonomy-input-validation-refusals-answer-validationexception.md, which owns many other sqs refusal checks but not this one and does not mention the transport-level 413.)
Fix: the request-size guard in front of the sqs request pipeline — reject a request body above the transport limit with HTTP 413 and no SQS error document, before the SendMessage handler's field validation runs; keep the exactly-262144-byte body accepted, and ensure the refused message is not enqueued.
Traces: sqs.max-message-size.

## Evidence
Cause 1 — sqs.delete-message-batch-partial-failure [mismatch]:
AssertionError: expected message 'The receipt handle "<handle>" is not valid for this queue.', got 'The input receipt handle "<handle>" is not a valid receipt handle for this queue.'
The assertions before it PASSED: Successful == {'d1'}, 'd2' in Failed, Failed['d2']['Code'] == 'ReceiptHandleIsInvalid'. The checks after it (independent reads at both queues) confirm the emulator really deleted the d1 message and really left the foreign-queue message in place — so only the entry Message text diverges. The handle in the message is this run's value filling the template; the requirement is the fixed text with the request's ReceiptHandle substituted, double-quoted.

Cause 2 — sqs.max-message-size [mismatch]:
AssertionError: expected a 413 (request entity too large) for a 2MB body, got ValidationException
The positive control before it PASSED: a 262144-byte body was accepted and received back unchanged. Expected (AWS): the 2 MiB send_message raises a ClientError whose Error.Code is the bare transport status `413`, with no SQS error document, and the queue stays empty. Actual (emulator): Error.Code `ValidationException`.

## Steps to Reproduce
Cause 1:
1. sqs:create_queue A and B, both with VisibilityTimeout=2.
2. send_message twice to A, receive both (note handles h1, h2); send_message once to B, receive it (note handle hB).
3. delete_message_batch(QueueUrl=<A>, Entries=[{'Id':'d1','ReceiptHandle':h1},{'Id':'d2','ReceiptHandle':hB}]).
   Expected: HTTP 200 with Successful=[{Id:'d1'}] and Failed=[{Id:'d2', Code:'ReceiptHandleIsInvalid', Message:'The receipt handle "<hB>" is not valid for this queue.'}].
   Actual: same Id/Code, Message = 'The input receipt handle "<hB>" is not a valid receipt handle for this queue.'
4. (Already correct, keep it:) after the 2 s visibility timeout only the untouched A message redelivers, and B's message is still in B.

Cause 2:
1. sqs:create_queue with default attributes.
2. send_message(MessageBody='x'*262144) -> must be accepted; receive_message must return the body byte-for-byte unchanged.
3. send_message(MessageBody='x'*2097152).
   Expected: refused at the HTTP layer with status 413 and no SQS error document (botocore reports Error.Code == '413'); nothing enqueued.
   Actual: refused with an SQS error document carrying Error.Code == 'ValidationException'.
4. receive_message must return no messages in either case.

Replay the two check ids in trace_ids to confirm.
