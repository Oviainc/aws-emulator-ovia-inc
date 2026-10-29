Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.ddb-stream-esm-record-envelope-and-images-follow-item-lifecycle dynamodb__lambda.ddb-stream-esm-partial-batch-response-redelivers-only-the-suffix dynamodb__lambda.ddb-stream-esm-bisect-on-function-error-isolates-the-poison-record dynamodb__lambda.ddb-stream-latest-mapping-never-sees-records-written-before-it dynamodb__lambda.ddb-stream-new-image-view-carries-only-after-images dynamodb__lambda.ddb-stream-old-image-view-carries-only-before-images dynamodb__lambda.ddb-stream-writes-that-change-nothing-emit-no-record dynamodb__lambda.concurrent-lambda-ddb-writes-no-lost-updates
Issue-Key: dynamodb__lambda.ddb-stream-esm-record-envelope-and-images-follow-item-lifecycle|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda.ddb-stream-esm-partial-batch-response-redelivers-only-the-suffix|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda.ddb-stream-esm-bisect-on-function-error-isolates-the-poison-record|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda.ddb-stream-latest-mapping-never-sees-records-written-before-it|run#15|Timeout|time.sleep(5)
Issue-Key: dynamodb__lambda.ddb-stream-new-image-view-carries-only-after-images|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda.ddb-stream-old-image-view-carries-only-before-images|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda.ddb-stream-writes-that-change-nothing-emit-no-record|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__lambda.concurrent-lambda-ddb-writes-no-lost-updates|run#6|AssertionError|assert _concurrent_successes(results) == 5, (
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# DynamoDB-stream ESM record envelope/images never observable, and concurrent Invoke is refused 501 (2 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes in the dynamodb__lambda group. Each needs its own fix.

Cause 1 of 2: DynamoDB-stream event source mappings never deliver, so the record envelope and the stream views cannot be observed.
AWS: an enabled ESM invokes the function with records whose envelope (eventName, eventSourceARN, dynamodb.Keys/NewImage/OldImage, SequenceNumber) follows the item lifecycle; a NEW_IMAGE view carries only after-images and an OLD_IMAGE view only before-images; a LATEST mapping never sees records written before it; a write that changes nothing emits no record; a partial-batch response redelivers only the reported suffix and BisectBatchOnFunctionError isolates the poison record.
Emulator: no delivery; each check polls for the effect and ends with `Check timed out after 60.0s`.
Why: every stream-driven workload is silently inert, and none of the stream-view or failure-handling semantics a consumer depends on can be relied upon.
Fix: the lambda event-source-mapping poller for DynamoDB streams (delivery, record envelope, StreamViewType projection, starting position, partial-batch/bisect handling).
Traces: dynamodb__lambda.ddb-stream-esm-record-envelope-and-images-follow-item-lifecycle, .ddb-stream-esm-partial-batch-response-redelivers-only-the-suffix, .ddb-stream-esm-bisect-on-function-error-isolates-the-poison-record, .ddb-stream-latest-mapping-never-sees-records-written-before-it, .ddb-stream-new-image-view-carries-only-after-images, .ddb-stream-old-image-view-carries-only-before-images, .ddb-stream-writes-that-change-nothing-emit-no-record

Cause 2 of 2: concurrent Invoke of a real function is refused with a 501 that no SDK models.
AWS: `Invoke` runs the function and returns its payload (and the concurrency semantics the check measures).
Emulator: every one of the concurrent invocations fails with HTTP 501 `NotImplemented` and the message `Invoke resolved <function ARN> at version $LATEST, but running its <runtime> deployment package is not implemented in this emulator: ...`.
Why: no lost-update or concurrency property of a Lambda-driven writer can be exercised; the caller gets a status/code the Lambda model does not define. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes the same refusal but does not carry this check's replay key.
Fix: the lambda Invoke handler (execute the deployment package / return the function's payload).
Traces: dynamodb__lambda.concurrent-lambda-ddb-writes-no-lost-updates

## Evidence
- Cause 1 (seven checks): Error: Check timed out after 60.0s.
- Cause 2: AssertionError: Not all invocations succeeded: [ClientError('An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:<fn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator ...'), ... x many].

## Steps to Reproduce
Boot the emulator and replay the eight checks listed in trace_ids. Cause 1: CreateEventSourceMapping over a table stream, write/modify/delete items, and poll for the records. Cause 2: Invoke a created function concurrently.
