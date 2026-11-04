Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.stream-records-reach-the-handler-with-typed-nested-images-intact dynamodb__lambda__logs.bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn dynamodb__lambda__logs.bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image dynamodb__lambda__logs.deleting-a-request-delivers-a-remove-with-only-the-old-image dynamodb__lambda__logs.new-image-only-stream-view-makes-the-bat-transition-skip-too dynamodb__lambda__logs.patched-handler-processes-direct-in-progress-inserts dynamodb__lambda__logs.re-saving-an-in-progress-request-is-skipped-as-no-transition
Issue-Key: dynamodb__lambda__logs.stream-records-reach-the-handler-with-typed-nested-images-intact|_await_invocation#7|Timeout|time.sleep(4)
Issue-Key: dynamodb__lambda__logs.bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn|_await_invocation#6|Timeout|time.sleep(4)
Issue-Key: dynamodb__lambda__logs.bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image|_await_invocation#7|Timeout|time.sleep(4)
Issue-Key: dynamodb__lambda__logs.deleting-a-request-delivers-a-remove-with-only-the-old-image|_await_invocation#7|Timeout|time.sleep(4)
Issue-Key: dynamodb__lambda__logs.new-image-only-stream-view-makes-the-bat-transition-skip-too|_await_invocation#10|Timeout|time.sleep(4)
Issue-Key: dynamodb__lambda__logs.patched-handler-processes-direct-in-progress-inserts|_await_invocation#8|Timeout|time.sleep(4)
Issue-Key: dynamodb__lambda__logs.re-saving-an-in-progress-request-is-skipped-as-no-transition|_await_invocation#7|Timeout|time.sleep(4)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# DynamoDB Streams records never reach the mapped Lambda, so the whole request-transition scenario never settles

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: an enabled DynamoDB Streams event source mapping must deliver INSERT/MODIFY/REMOVE records — with the StreamViewType's images, typed and nested as written, and the table name parseable out of the eventSourceARN — to the mapped function, and must emit no record for a write that changes nothing.
AWS: recorded behavior only — each of these seven recordings writes items to a streamed table and then reads the handler's own log lines out of /aws/lambda/<name> to see which records arrived, with which images, and under which table name; on AWS every one of them lands well inside the suite's 60s budget.
Emulator: all seven checks are cut off with "Error: Check timed out after 60.0s". CreateEventSourceMapping is accepted and reports the mapping Enabled, but no batch is ever handed to the function, so no handler log line is ever written and each poll runs out the budget.
Why: every DynamoDB-stream consumer silently processes nothing while the mapping reports itself healthy; none of the image-view semantics (KEYS_ONLY / NEW_IMAGE / OLD_IMAGE / NEW_AND_OLD_IMAGES), the no-op-write suppression, or the eventSourceARN-derived table name are observable at all.
Fix: the Lambda event-source-mapping poller for DynamoDB Streams — shard iteration, batch assembly, the record envelope (eventName/eventSourceARN/dynamodb images) and the invocation of the target function.
Traces: dynamodb__lambda__logs.stream-records-reach-the-handler-with-typed-nested-images-intact, .bat-transition-is-processed-under-the-table-name-parsed-from-the-stream-arn, .bulk-insert-in-progress-is-never-processed-for-want-of-an-old-image, .deleting-a-request-delivers-a-remove-with-only-the-old-image, .new-image-only-stream-view-makes-the-bat-transition-skip-too, .patched-handler-processes-direct-in-progress-inserts, .re-saving-an-in-progress-request-is-skipped-as-no-transition.

## Evidence
Expected (AWS): the handler logs one record per qualifying write, with the images the StreamViewType prescribes and the table name taken from the eventSourceARN.
Actual (emulator): all seven checks end with 'Error: Check timed out after 60.0s' — the mapping is Enabled but no record is ever delivered.

## Steps to Reproduce
1. Create a DynamoDB table with StreamSpecification enabled (try each StreamViewType).
2. CreateEventSourceMapping from the table's LatestStreamArn to a Lambda; confirm it reports State Enabled.
3. PutItem / UpdateItem / DeleteItem on the table and poll the function's log group for its per-record lines. Observe no record is ever delivered within 60s.
