Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative
Issue-Key: lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative|run#18|Timeout|time.sleep(5)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# lambda__s3__sqs: an SQS event source mapping never delivers, so the disabled-then-re-enabled mapping contract cannot be measured

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping / UpdateEventSourceMapping (SQS poller)

## Description
One net-new root cause in the lambda__s3__sqs suite. It is NOT covered by the open issues for this heading (20261107T081525Z-lambda-s3-sqs-2-net-new-causes-a.md, 20261109T185037Z-lambda-s3-sqs-2-net-new-causes-s3-presigned-urls-are.md, 20261109T023059Z-lambda-s3-sqs-createeventsourcemapping-refuses-a-foreign-account.md, 20261105T094842Z-lambda-s3-sqs-the-python-lambda-runtime-provides.md, 20261107T145219Z-lambda-s3-sqs-a-bucket-notification-configuration-that.md); this check carries none of their replay keys.

Cause 1 of 1: an enabled SQS event source mapping never invokes its function, so the check cannot even measure the mapping's delivery latency — the baseline it needs before it can assert that a DISABLED mapping withholds a message and that re-enabling delivers the one queued while it was off.
AWS: recorded behavior only for the poller timing; the surface is modelled. CreateEventSourceMapping with an SQS EventSourceArn and Enabled=true reaches State "Enabled" and the poller invokes the function for each queued message (BatchSize=1 here), with the message removed from the queue on success. UpdateEventSourceMapping with Enabled=false moves it to "Disabled" and stops delivery: a message sent while disabled stays in the queue and the function is not invoked for a window sized from the mapping's own measured delivery latency. Re-enabling it delivers exactly that queued message.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — the first leg, measuring one delivery through the enabled mapping, never completes.
Why: a client's SQS-triggered Lambda never runs, so every queue-driven workload is dead. And because nothing is delivered in the enabled state either, the negative half of the contract is vacuous: a mapping that ignores Enabled=false entirely cannot be distinguished from one that honours it.
Fix: the Lambda event-source-mapping poller for SQS — an Enabled mapping must receive from the queue and invoke the function (honouring BatchSize), deleting the message on success; Enabled=false must suspend receiving without losing queued messages; re-enabling must resume and deliver them.
Traces: lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative.

## Evidence
Check lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "A disabled event source mapping does not deliver a queued message for a window sized from the mapping's own measured delivery latency, and re-enabling it delivers the message that was queued while it was disabled." The check creates a bucket, a role, a queue and a function, then creates the mapping with batch_size=1, enabled=True, and measures one delivery before disabling.

Expected: the enabled mapping invokes the function for the first queued message (observable as the function's report object in the bucket) within the measured latency; the disabled mapping invokes nothing for the absence window; re-enabling delivers the withheld message. Actual: the first delivery never happens, so the check is killed with nothing compared. Queue URLs, function names, mapping UUIDs and bucket names are per-run values.

## Steps to Reproduce
1. Create a bucket, an execution role, an SQS queue and a Lambda function that writes a report object per received record.
2. CreateEventSourceMapping(FunctionName=..., EventSourceArn=<queue arn>, BatchSize=1, Enabled=True) and wait for State "Enabled"; SendMessage and time how long the invocation takes to appear.
3. UpdateEventSourceMapping(UUID=..., Enabled=False), wait for State "Disabled", SendMessage again and confirm no invocation for a window sized from step 2's latency, with the message still in the queue.
4. UpdateEventSourceMapping(Enabled=True) and confirm the withheld message is delivered.
5. The emulator never completes step 2. Re-replay lambda__s3__sqs.sqs-esm-disabled-mapping-no-invocation-negative to confirm the fix.
