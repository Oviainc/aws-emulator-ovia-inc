Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.fifo-source-preserves-message-order pipes.pipe-preserves-fifo-message-ordering
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|SendMessage|NotImplemented
Issue-Commit: cfd0674e2ddfe499cbab1f11b70f1bc18d331a71

# SQS SendMessage to a FIFO queue answers the non-modeled NotImplemented, so no FIFO producer works and FIFO ordering through a pipe cannot be exercised

> _Note: this issue was identified against the code base at commit `cfd0674e2ddfe499cbab1f11b70f1bc18d331a71`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
This bundle has 1 cause.

Cause 1 of 1: SendMessage refuses any request whose queue is a FIFO (.fifo) queue with a non-modeled NotImplemented error, even though CreateQueue with FifoQueue=true succeeded.
AWS: Smithy (com.amazonaws.sqs#SendMessage): input members QueueUrl (required), MessageBody (required), DelaySeconds, MessageAttributes, MessageSystemAttributes, MessageDeduplicationId and MessageGroupId; MessageGroupId/MessageDeduplicationId exist precisely for FIFO queues (MessageGroupId is required for a FIFO queue, and MessageDeduplicationId is required unless the queue has ContentBasedDeduplication enabled). SendMessage's modeled errors are InvalidMessageContents, UnsupportedOperation, RequestThrottled, QueueDoesNotExist, InvalidSecurity, InvalidAddress and KmsInvalidKeyUsage-class failures — NotImplemented is not among them. A successful call is HTTP 200 with MessageId, MD5OfMessageBody and (for FIFO) SequenceNumber; subsequent ReceiveMessage returns messages of a group in strict send order.
Emulator: every SendMessage whose target is a FIFO queue fails with botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendMessage operation: operation SendMessage to a FIFO queue is scaffolded but not implemented in this emulator. SendMessage to a standard queue is served normally, so this is a per-queue-type refusal inside an otherwise implemented operation. The two failing checks were not marked ALREADY FILED or OWNED by any open issue; the stub issue the suite filed for sqs SendMessageBatch (services/service-com-amazonaws-sqs-amazonsqs/src/ops/send_message_batch.rs) is a different operation and does not cover this.
Why: no client can produce into a FIFO queue at all, so ordering, deduplication and message-group semantics — the only reasons to choose FIFO — are unreachable, and the error code is one no SDK models, so retry/classification logic sees an unclassifiable fault instead of a clean unsupported-feature signal. It also blocks every cross-service topology that uses a FIFO queue as a source, including EventBridge Pipes FIFO ordering.
Fix: the SQS SendMessage handler and the queue state behind it (services/service-com-amazonaws-sqs-amazonsqs) — accept FIFO queues, require MessageGroupId, require MessageDeduplicationId unless ContentBasedDeduplication is set, return SequenceNumber, and preserve per-group FIFO order on receive.
Traces: pipes.fifo-source-preserves-message-order, pipes.pipe-preserves-fifo-message-ordering. Both check ids carry the leading token `pipes` because they live in the pipes suite directory, but the failing call and the defect are SQS's (SendMessage); sqs is bootstrapped.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 12/107 passed (11.2%). The emulator was serving (no bootstrap signal; CreateQueue with FifoQueue=true and SendMessage to standard queues both succeeded in these runs).

Both checks fail on their first FIFO producer call, before any ordering assertion:
  pipes.fifo-source-preserves-message-order — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendMessage operation: operation SendMessage to a FIFO queue is scaffolded but not implemented in this emulator
  pipes.pipe-preserves-fifo-message-ordering — identical error.
  Expected: HTTP 200 with MessageId, MD5OfMessageBody and SequenceNumber for SendMessage(QueueUrl=<...>.fifo, MessageBody=..., MessageGroupId=...), after which per-group order is preserved on ReceiveMessage.
  Actual: error code NotImplemented, which SendMessage does not model.

Queue names, URLs, message ids and sequence numbers are generated per run and are examples only; the requirement is that SendMessage to a FIFO queue succeeds with the FIFO members honored.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call CreateQueue with QueueName='<name>.fifo' and Attributes {FifoQueue: 'true', ContentBasedDeduplication: 'true'} — this succeeds.
3. Call SendMessage(QueueUrl=<that url>, MessageBody='{"n":1}', MessageGroupId='g1'). Observe the request fail with error code NotImplemented ('operation SendMessage to a FIFO queue is scaffolded but not implemented in this emulator') instead of HTTP 200 with MessageId/MD5OfMessageBody/SequenceNumber.
4. Re-replay pipes.fifo-source-preserves-message-order and pipes.pipe-preserves-fifo-message-ordering to confirm; each fails on its first FIFO SendMessage. Note those checks also depend on the pipes runtime filed separately in this run.
