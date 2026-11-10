Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.s3-multi-event-type-routing-sqs
Issue-Key: events__s3__sqs.s3-multi-event-type-routing-sqs|_drain#18|Timeout|resp = sqs.receive_message(
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# events__s3__sqs: a two-destination S3 notification configuration delivers neither ObjectCreated nor ObjectRemoved to its queue

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration (multi-event-type routing to SQS)

## Description
One net-new root cause in the events__s3__sqs suite. It is NOT covered by the open issues for this heading (20261107T164117Z-events-s3-sqs-s3-eventbridge-envelopes-are-not.md, which is about the EventBridge envelope, and 20261101T182649Z-events-s3-sqs-puttargets-accepts-an-sqs-fifo.md, which is about PutTargets); this check carries neither's replay key.

Cause 1 of 1: an S3 notification configuration with one QueueConfiguration per event-type family delivers nothing, so the per-type routing contract is never exercised.
AWS: recorded behavior only for the delivery. With two QueueConfigurations on one bucket — s3:ObjectCreated:* to queue A and s3:ObjectRemoved:* to queue B — a PutObject delivers exactly one ObjectCreated record to queue A and nothing to queue B, and a DeleteObject delivers exactly one ObjectRemoved record to queue B and nothing to queue A. No record leaks across in either direction.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached. The first wait — for the ObjectCreated record on queue A — never completes, so neither the delivered record nor the required absence on the sibling queue was ever compared.
Why: a client fanning S3 events to per-concern consumers gets no events at all, and because both queues stay empty the no-leak half of the contract passes vacuously: a dispatcher that broadcast every event to every destination would look identical to one that routed correctly.
Fix: the S3 notification dispatcher — it must evaluate each event against every configured QueueConfiguration's event-type list and deliver the record to the matching destinations only.
Traces: events__s3__sqs.s3-multi-event-type-routing-sqs.

## Evidence
Check events__s3__sqs.s3-multi-event-type-routing-sqs, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "An S3 notification configuration delivers ObjectCreated only to its queue and ObjectRemoved only to its own, with nothing leaking to the other queue in either direction." The check creates the bucket, two queues, sets each queue's policy, and then waits on the first delivery.

Expected: one s3:ObjectCreated:* record in the created-queue after PutObject and one s3:ObjectRemoved:* record in the removed-queue after DeleteObject, with each sibling queue empty. Actual: no message arrives on either queue inside the run budget. Bucket names, queue URLs and keys are per-run values.

## Steps to Reproduce
1. Create a bucket and two SQS queues, each with a policy allowing s3.amazonaws.com to SendMessage.
2. PutBucketNotificationConfiguration with QueueConfigurations [{queue A, Events: ["s3:ObjectCreated:*"]}, {queue B, Events: ["s3:ObjectRemoved:*"]}].
3. PutObject a key and ReceiveMessage on both queues: AWS puts one ObjectCreated record on A and nothing on B.
4. DeleteObject that key and ReceiveMessage on both queues: AWS puts one ObjectRemoved record on B and nothing on A.
5. The emulator delivers nothing to either queue. Re-replay events__s3__sqs.s3-multi-event-type-routing-sqs to confirm the fix.
