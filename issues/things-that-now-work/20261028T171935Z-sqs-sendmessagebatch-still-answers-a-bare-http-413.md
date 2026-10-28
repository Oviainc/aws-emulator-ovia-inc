Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.batch-total-size-exceeds-limit-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.batch-total-size-exceeds-limit-rejected|run#14|AssertionError|assert code == ERR_BATCH_TOO_LONG, (
Issue-Commit: 62bd53a2191a4112059965729a42c5aa0aaceb39

# sqs: SendMessageBatch still answers a bare HTTP 413 for an over-aggregate-size batch instead of AWS.SimpleQueueService.BatchRequestTooLong

> _Note: this issue was identified against the code base at commit `62bd53a2191a4112059965729a42c5aa0aaceb39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessageBatch

## Description
REOPENS 20261027T034110Z-sqs-10-net-new-behavioral-root-causes-batchrequesttoolong.md: its fix left the call refused.

That issue is marked fixed and carried sqs.batch-total-size-exceeds-limit-rejected as Cause 1 ("BatchRequestTooLong served as a bare 413"), but the check still fails on exactly the same refusal shape: the call comes back as a non-modeled HTTP 413 with no SQS error document.

Cause 1 of 1: a SendMessageBatch whose entries are each individually legal but whose aggregate payload exceeds the queue's MaximumMessageSize is refused at the transport layer with a bare HTTP 413 instead of the modeled BatchRequestTooLong error.

AWS: Smithy (services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json) binds `BatchRequestTooLong` to SendMessageBatch as a client error with httpError 400 and aws.protocols#awsQueryError code `AWS.SimpleQueueService.BatchRequestTooLong` ("The length of all the messages put together is more than the limit."). The bound AWS enforces is the queue's own `MaximumMessageSize` attribute, and the refusal message is the fixed template `Batch requests cannot be longer than {MaximumMessageSize} bytes. You have sent {total request bytes} bytes.` — the two numbers are values taken from the queue's attribute and from the request, everything else is fixed text. The refusal is all-or-nothing: no entry of the batch enqueues, and the same entries split so that each request is under the bound are accepted.

Emulator: the response carries HTTP status 413 with an empty body, so botocore surfaces Error.Code `'413'` and Error.Message `''` — no modeled error shape, no awsQuery error code, no message, and nothing naming the limit or the measured size. Hypothesis: the aggregate request-body size is now rejected by a transport-level/body-size guard ahead of the operation handler (the inverse divergence is already filed for the single-message path in open issue 20261028T073617Z-sqs-sendmessage-an-oversized-2-mb-messagebody-is.md, where AWS's transport 413 is instead answered with a modeled error); neither that open issue nor any other open issue carries this check's replay key, so none covers it.

Why: a client that catches `BatchRequestTooLong` — or any modeled SQS error — never matches. A bare 413 with an empty body is not a parsable SQS error document, so SDK error classification treats it as an unknown transport failure (often retryable) rather than a sender fault, and the caller never learns which bound it exceeded or by how much, so the standard recovery of splitting the batch is never triggered.

Fix: the SendMessageBatch handler's aggregate-size validation plus the awsQuery/awsJson1.0 SQS error serializer — the per-request aggregate must be measured against the queue's MaximumMessageSize inside the operation (before any entry is enqueued) and rendered as the modeled BatchRequestTooLong error with AWS's message template, and the generic transport body-size guard must not pre-empt this operation.

Traces: sqs.batch-total-size-exceeds-limit-rejected.

## Evidence
From the behavioral replay report (suite simparity-behavioral-ga-prod-v1; sqs 243/260 passed, 17 failed). The check is marked neither ALREADY FILED nor OWNED.

[mismatch] sqs.batch-total-size-exceeds-limit-rejected
  expected: Error.Code `AWS.SimpleQueueService.BatchRequestTooLong`, HTTP 400, Error.Message `Batch requests cannot be longer than {MaximumMessageSize} bytes. You have sent {total bytes} bytes.` (fixed text; the two numbers come from the queue attribute and the request), nothing enqueued.
  actual:   AssertionError: expected AWS.SimpleQueueService.BatchRequestTooLong for a 1398104-byte batch of 4 individually-legal 349526-byte entries, got '413' ('')

The check reads MaximumMessageSize off the live queue (asserting it is at least the documented 262144 floor) and builds 4 entries of limit//3 + 1 bytes, so the per-entry sizes and the total are derived from the emulator's own reported limit, not hardcoded.

## Steps to Reproduce
1. CreateQueue with a fresh name.
2. GetQueueAttributes(AttributeNames=["MaximumMessageSize"]) and read the limit L.
3. SendMessageBatch with 4 entries whose MessageBody is each (L//3 + 1) bytes — each entry individually legal, the sum (~4L/3) over the bound.
4. Observe the response is HTTP 413 with an empty body (botocore Error.Code '413', Error.Message ''), instead of HTTP 400 with code AWS.SimpleQueueService.BatchRequestTooLong and message "Batch requests cannot be longer than L bytes. You have sent <total> bytes.".
5. Confirm the all-or-nothing contract still holds (ApproximateNumberOfMessages stays 0, nothing is delivered) and that the same entries sent as two batches of two succeed.
