Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.notification-destination-validation-rejects-unresolvable-queue s3__sqs.deploy-s3-3-notification-nonexistent-queue-rollback-rejected s3__sqs.s3-object-created-put-subscription-withholds-the-multipart-completion-event s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key
Issue-Key: s3__sqs.notification-destination-validation-rejects-unresolvable-queue|run#16|AssertionError|assert test_events, f"no s3:TestEvent arrived after a valid PutBucketNotificationConfiguration; saw {messages!r}"
Issue-Key: s3__sqs.deploy-s3-3-notification-nonexistent-queue-rollback-rejected|run#13|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: s3__sqs.s3-object-created-put-subscription-withholds-the-multipart-completion-event|_collect#11|Timeout|resp = sqs.receive_message(
Issue-Key: s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key|_drain_test_event#19|Timeout|resp = sqs.receive_message(
Issue-Commit: 271fde8cc1a42675cd9f2105c7c7bd008dd933ab

# s3__sqs: S3 notification destinations are never validated and no object event is ever delivered to SQS (3 causes)

> _Note: this issue was identified against the code base at commit `271fde8cc1a42675cd9f2105c7c7bd008dd933ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Three distinct root causes on the S3-to-SQS notification path. Each is numbered with its own failing check ids; all three must be closed before this issue is submitted.

Cause 1 of 3: PutBucketNotificationConfiguration accepts a destination queue that cannot receive, and emits no s3:TestEvent.
AWS: PutBucketNotificationConfiguration resolves every destination and refuses an unresolvable or unauthorized queue with InvalidArgument/400 ('Unable to validate the following destination configurations'); a configuration that does validate causes S3 to publish an s3:TestEvent to the destination immediately.
Emulator: the configuration naming an unresolvable queue is accepted, and after a valid put no test event arrives — `no s3:TestEvent arrived after a valid PutBucketNotificationConfiguration; saw []`.
Why: the configuration-time check is the only place a mis-wired destination is caught; without it and without the test event, a broken pipeline looks identical to a working one.
Fix: destination resolution/validation in the PutBucketNotificationConfiguration handler, and the s3:TestEvent emitter.
Traces: s3__sqs.notification-destination-validation-rejects-unresolvable-queue

Cause 2 of 3: CloudFormation reports CREATE_COMPLETE for a stack whose bucket notifies a nonexistent queue.
AWS: because S3 validates the destination at configuration time, the AWS::S3::Bucket resource fails and the stack rolls back to ROLLBACK_COMPLETE.
Emulator: `a stack whose BucketB notifies a nonexistent queue ended 'CREATE_COMPLETE' (reason None); expected ROLLBACK_COMPLETE`.
Why: an invalid template deploys 'successfully', so the deploy-time guardrail is gone and the failure only surfaces as missing events later.
Fix: destination validation on the notification-configuration path, surfaced through the CloudFormation AWS::S3::Bucket provider so the resource fails.
Traces: s3__sqs.deploy-s3-3-notification-nonexistent-queue-rollback-rejected

Cause 3 of 3: no S3 object event is ever delivered to SQS, so event selection and ordering are unobservable.
AWS: with an s3:ObjectCreated:Put subscription, a simple PUT produces one event and a CompleteMultipartUpload does not; every event for one key carries a monotonically increasing sequencer that is the only total order over that key.
Emulator: both checks hang to the 60 s cap waiting for messages that never arrive.
Why: event-driven consumers receive nothing, and the two properties clients depend on (event-type selection and per-key sequencer ordering) cannot be relied on.
Fix: the S3 event emitter on the object write paths and its SQS delivery, including event-name selection and the sequencer field.
Traces: s3__sqs.s3-object-created-put-subscription-withholds-the-multipart-completion-event, s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key

## Evidence
Cause 1: `AssertionError: no s3:TestEvent arrived after a valid PutBucketNotificationConfiguration; saw []`. Expected: InvalidArgument/400 for the unresolvable queue, and an s3:TestEvent on the valid configuration.
Cause 2: `AssertionError: a stack whose BucketB notifies a nonexistent queue ended 'CREATE_COMPLETE' (reason None); expected ROLLBACK_COMPLETE`.
Cause 3: `Error: Check timed out after 60.0s` for s3__sqs.s3-object-created-put-subscription-withholds-the-multipart-completion-event and for s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key.

## Steps to Reproduce
Boot the emulator and replay the suite, re-running the three check ids in trace_ids. Manual: (1) CreateQueue, then PutBucketNotificationConfiguration with a QueueConfiguration naming a queue ARN that does not exist (expect InvalidArgument/400), then one naming the real queue and poll for s3:TestEvent; (2) deploy a CloudFormation stack whose AWS::S3::Bucket NotificationConfiguration names a nonexistent queue and read the terminal stack status; (3) with an s3:ObjectCreated:Put subscription, PutObject once and CompleteMultipartUpload once, and receive from the queue.
