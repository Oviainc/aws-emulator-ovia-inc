Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.an-aborted-multipart-upload-emits-no-object-created-notification
Issue-Key: s3__sqs.an-aborted-multipart-upload-emits-no-object-created-notification|_drain_records#15|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: fbdf9d3baa643423c5a7eac120dec57cfd2abcbc

# s3__sqs: a completed multipart upload delivers no s3:ObjectCreated notification to the configured SQS queue

> _Note: this issue was identified against the code base at commit `fbdf9d3baa643423c5a7eac120dec57cfd2abcbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteMultipartUpload

## Description
Cause 1 of 1 — the multipart event path publishes nothing, so neither the positive nor the negative half of the abort contract can be observed.
AWS: with a bucket notification configured for `s3:ObjectCreated:*` (or `s3:ObjectCreated:CompleteMultipartUpload`) to an SQS queue, CompleteMultipartUpload publishes exactly one event record whose eventName is `ObjectCreated:CompleteMultipartUpload`, naming the bucket and key and carrying a sequencer; AbortMultipartUpload publishes nothing, because no object was created. Recorded behavior only — event delivery is not expressed in the s3 Smithy model.
Emulator: nothing is ever delivered to the queue, so the check exhausts its budget waiting for the completion event and reports `Check timed out after 60.0s`. The absence of the aborted upload's event (the thing being asserted) is therefore indistinguishable from the absence of every event, and the contract is untestable rather than satisfied.
Why: an architecture where an upload completion fans out to a queue consumer never fires against this emulator; the consumer sits idle, the client has no error to react to, and a 'no event for an abort' invariant silently passes for the wrong reason. A notification configuration the API accepted is inert.
Fix: event emission on the CompleteMultipartUpload path in the s3 notification publisher (the ObjectCreated:CompleteMultipartUpload record and its SQS delivery), with AbortMultipartUpload deliberately emitting nothing. Related open issues name the general S3→SQS notification gap for single-part writes — `20261025T032458Z-s3-sqs-s3-notification-destinations-are-never-validated.md` and `20261025T002357Z-events-s3-sqs-s3-produces-no-aws-s3-events-on-either.md` — but neither carries this check or the multipart-completion event name.
Traces: s3__sqs.an-aborted-multipart-upload-emits-no-object-created-notification.

## Evidence
Error: Check timed out after 60.0s. The check configures a bucket notification to an SQS queue, runs a multipart upload to completion and polls the queue for the ObjectCreated:CompleteMultipartUpload record (its own bounded poll raises on its deadline); no message is ever received, so the subsequent assertion that an ABORTED upload produces no event is never reached.

## Steps to Reproduce
1. Create a bucket and an SQS queue with a policy allowing s3.amazonaws.com to SendMessage.
2. PutBucketNotificationConfiguration with a QueueConfiguration for s3:ObjectCreated:*.
3. CreateMultipartUpload, UploadPart, CompleteMultipartUpload; poll the queue for an event record with eventName ObjectCreated:CompleteMultipartUpload.
4. Then CreateMultipartUpload/UploadPart/AbortMultipartUpload and confirm no further event arrives.
