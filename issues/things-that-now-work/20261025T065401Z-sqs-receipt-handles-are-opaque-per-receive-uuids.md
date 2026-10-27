Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.delete-message-repeated-with-the-same-receipt-handle-succeeds sqs.receipt-handle-from-another-queue-rejected sqs.delete-and-visibility-batches-receipt-handle-from-another-queue-fails-alone sqs.receipt-handle-from-a-deleted-queue-incarnation-rejected sqs.delete-message-invalid-receipt-handle-rejected sqs.expired-receipt-handle-delete-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|DeleteMessage|ReceiptHandleIsInvalid
Issue-Key: sqs.receipt-handle-from-another-queue-rejected|run#14|AssertionError|assert message == _handle_reason(handle_a), (
Issue-Key: sqs.delete-and-visibility-batches-receipt-handle-from-another-queue-fails-alone|run#17|AssertionError|assert failed["foreign"].get("Message") == _handle_reason(foreign_handle), (
Issue-Key: sqs.receipt-handle-from-a-deleted-queue-incarnation-rejected|run#16|AssertionError|assert (err.get("Code"), err.get("Message")) == (_NO_SUCH_QUEUE, _NO_SUCH_QUEUE_MSG), (
Issue-Key: sqs.delete-message-invalid-receipt-handle-rejected|run#11|AssertionError|assert message == "The input receipt handle is invalid.", (
Issue-Key: sqs.expired-receipt-handle-delete-rejected|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS receipt handles are opaque per-receive UUIDs with no message or queue binding: a repeat delete is refused, and a foreign-queue handle is indistinguishable from a malformed one (5 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMessage

## Description
The emulator hands out bare UUIDs as receipt handles and validates them only by presence in a single live table, so three distinct AWS outcomes collapse into one refusal.

Cause 1 of 5: a repeated DeleteMessage with the same handle must succeed. AWS: DeleteMessage is idempotent for a handle whose message has already been deleted within the same receive — the second call returns 200 with no body. Smithy: DeleteMessage models ReceiptHandleIsInvalid (404, awsQueryError ReceiptHandleIsInvalid) only for a handle that is not valid for the queue. Emulator: the second DeleteMessage is refused with ReceiptHandleIsInvalid, and the check aborts. Why: a consumer that retries a delete after a network timeout — the standard at-least-once pattern — gets a hard error where AWS returns success. Fix: the handle lookup in the DeleteMessage handler must treat an already-consumed handle of this queue's current incarnation as a success. Traces: sqs.delete-message-repeated-with-the-same-receipt-handle-succeeds.

Cause 2 of 5: a handle issued by another queue must be refused AS SUCH. AWS: DeleteMessage with a handle belonging to a different queue is refused with a message attributing the fault to the handle belonging to another queue, distinct from the malformed/expired case; in DeleteMessageBatch only that entry fails and the other entries succeed. Emulator: the generic 'The input receipt handle "<uuid>" is not a valid receipt handle for this queue.' is used for every rejection, so the foreign-queue case cannot be told from a malformed one. Why: a client cannot distinguish 'you addressed the wrong queue' from 'your handle expired' and retries the wrong recovery. Fix: bind the queue identity into the handle and classify the refusal in the DeleteMessage / DeleteMessageBatch / ChangeMessageVisibility handlers. Traces: sqs.receipt-handle-from-another-queue-rejected, sqs.delete-and-visibility-batches-receipt-handle-from-another-queue-fails-alone.

Cause 3 of 5: a handle from a deleted queue incarnation. AWS: after a queue is deleted and recreated under the same name, a handle from the old incarnation is refused with the incarnation-specific refusal, not the generic invalid-handle one. Emulator: same generic message/code. Fix: include the queue incarnation in the handle and classify it. Traces: sqs.receipt-handle-from-a-deleted-queue-incarnation-rejected.

Cause 4 of 5: the refusal message for a malformed handle. AWS: a handle that is not a handle at all is refused with 'The input receipt handle is invalid.' — a fixed string that does NOT echo the supplied value. Emulator: it echoes the caller's string: 'The input receipt handle "totally-bogus-receipt-handle" is not a valid receipt handle for this queue.' Why: the message is AWS's documented fixed text for this fault, so a client (or a log scraper) matching it sees a different fault. Fix: the refusal constructor used by the DeleteMessage handler. Traces: sqs.delete-message-invalid-receipt-handle-rejected.

Cause 5 of 5: the refusal message for an expired handle. AWS: 'The receipt handle "<handle>" is not valid for this queue.' Emulator: 'The input receipt handle "<handle>" is not a valid receipt handle for this queue.' — a different template for a distinct AWS fault, built from the request and therefore part of the contract. Fix: same refusal constructor, expired-handle branch. Traces: sqs.expired-receipt-handle-delete-rejected.

## Evidence
Expected (AWS): a repeat DeleteMessage with the same handle returns 200; a foreign-queue handle, a stale-incarnation handle, a malformed handle and an expired handle are four distinguishable refusals with AWS's own message templates ('The input receipt handle is invalid.', 'The receipt handle "<h>" is not valid for this queue.').
Actual (emulator replay): 'botocore.errorfactory.ReceiptHandleIsInvalid: An error occurred (ReceiptHandleIsInvalid) when calling the DeleteMessage operation: The input receipt handle "8adb0d7b-..." is not a valid receipt handle for this queue.' on the second delete; 'the refusal must be attributed to the handle belonging to another QUEUE, not merely to a malformed or expired handle: got The input receipt handle "7e03d100-..." is not a valid receipt handle for this queue.'; 'expected message The input receipt handle is invalid., got The input receipt handle "totally-bogus-receipt-handle" is not a valid receipt handle for this queue.'; 'expected message The receipt handle "346652cd-..." is not valid for this queue., got The input receipt handle "346652cd-..." is not a valid receipt handle for this queue.'; receipt handles observed in ReceiveMessage responses are bare UUIDs ('ReceiptHandle': '2fb0c4a5-b190-409a-a9fc-6ebfa341dda0').

## Steps to Reproduce
1. CreateQueue, SendMessage, ReceiveMessage, DeleteMessage with the returned handle, then DeleteMessage again with the SAME handle -> ReceiptHandleIsInvalid instead of 200.
2. Create two queues, receive from queue A, DeleteMessage against queue B with A's handle -> the generic invalid-handle message, not the foreign-queue one.
3. DeleteMessage with the literal string 'totally-bogus-receipt-handle' -> the emulator echoes it in the message instead of AWS's fixed 'The input receipt handle is invalid.'
4. Receive a message, let the visibility timeout lapse, DeleteMessage with the stale handle -> the wrong message template.
5. Re-replay the check ids listed in trace_ids.
