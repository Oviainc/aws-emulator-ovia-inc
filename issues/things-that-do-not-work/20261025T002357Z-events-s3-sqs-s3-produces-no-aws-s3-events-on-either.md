Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.s3-notification-to-sqs-direct events__s3__sqs.object-restore-initiated-and-tags-added-carry-their-own-detail-types events__s3__sqs.obs-s3-3-object-acl-put-reaches-both-paths events__s3__sqs.obs-s3-3-object-tagging-put-and-delete-reach-both-paths events__s3__sqs.xsvc-s3-1-acl-update-and-tag-delete-route-to-their-own-rules events__s3__sqs.obs-s3-3-acl-rejected-under-bucket-owner-enforced-emits-no-event
Issue-Key: events__s3__sqs.s3-notification-to-sqs-direct|_wait_for_s3_records#9|Timeout|resp = sqs.receive_message(
Issue-Key: events__s3__sqs.object-restore-initiated-and-tags-added-carry-their-own-detail-types|_receive#20|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Key: events__s3__sqs.obs-s3-3-object-acl-put-reaches-both-paths|pump#16|Timeout|msgs = self.sqs.receive_message(QueueUrl=self.url, MaxNumberOfMessages=10,
Issue-Key: events__s3__sqs.obs-s3-3-object-tagging-put-and-delete-reach-both-paths|pump#13|Timeout|msgs = self.sqs.receive_message(QueueUrl=self.url, MaxNumberOfMessages=10,
Issue-Key: events__s3__sqs.xsvc-s3-1-acl-update-and-tag-delete-route-to-their-own-rules|_pull#12|Timeout|resp = self.sqs.receive_message(
Issue-Key: s3|GetBucketOwnershipControls|OwnershipControlsNotFoundError
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# S3 produces no aws.s3 events on either notification path, and GetBucketOwnershipControls reports a new bucket as having no ownership controls (2 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 2 — an S3 data-plane write emits nothing on either notification path.
AWS: with `PutBucketNotificationConfiguration` setting `EventBridgeConfiguration: {}`, S3 publishes events with source `aws.s3` to the account's DEFAULT event bus — detail-types "Object Created" (detail.reason naming the API used: PutObject, POST Object, CopyObject, CompleteMultipartUpload), "Object ACL Updated", "Object Tags Added", "Object Tags Deleted" and "Object Restore Initiated" — and `detail.object.etag` carries the ETag WITHOUT its surrounding quotes. With a `QueueConfiguration` instead, S3 sends the classic `s3:ObjectCreated:*` / `s3:ObjectAcl:Put` / `s3:ObjectTagging:*` records straight to the SQS queue. Both paths fire for the same write.
Emulator: neither path produces anything. Every check first establishes its own wiring (bucket, queue, queue policy admitting s3.amazonaws.com and the rule ARN, rule + target), writes the object, then polls its queue(s); the terminal read never succeeds and the replay cuts each check off at 60 s. That is true for the EventBridge path, the classic queue-configuration path, and for the detail-type routing checks that need one rule per detail-type to receive only its own type.
Why: S3 event notification is the backbone of event-driven storage pipelines. A client gets a bucket and a rule that both report healthy and a consumer that is never invoked — no object-created trigger, no ACL/tagging audit, no restore notification — and cannot distinguish "no event yet" from "events will never come".
Fix: the S3 notification publisher — the code that must emit to EventBridge and to the configured QueueConfiguration targets from the PutObject / PutObjectAcl / PutObjectTagging / DeleteObjectTagging / RestoreObject handlers in services/service-com-amazonaws-s3-amazons3/src/ops/ — and the EventBridge default-bus ingestion of `aws.s3` events.
Related open issue, not the same work item: 20261024T190039Z-events-lambda-s3-sfn-s3-event-notifications-never-fire-a-putobject-publishes.md covers the PutObject/LambdaFunctionConfiguration pair under service `events__lambda__s3__sfn`; it does not carry these checks' replay keys and does not cover the ACL/tagging/restore detail-types or the classic QueueConfiguration path.
Traces: events__s3__sqs.s3-notification-to-sqs-direct, events__s3__sqs.object-restore-initiated-and-tags-added-carry-their-own-detail-types, events__s3__sqs.obs-s3-3-object-acl-put-reaches-both-paths, events__s3__sqs.obs-s3-3-object-tagging-put-and-delete-reach-both-paths, events__s3__sqs.xsvc-s3-1-acl-update-and-tag-delete-route-to-their-own-rules.

Cause 2 of 2 — GetBucketOwnershipControls answers not-found for a bucket that has the default ownership controls.
AWS: a newly created bucket defaults to `ObjectOwnership: BucketOwnerEnforced`, and GetBucketOwnershipControls returns `OwnershipControls.Rules == [{'ObjectOwnership': 'BucketOwnerEnforced'}]` with HTTP 200. `OwnershipControlsNotFoundError` is for a bucket whose ownership controls were explicitly deleted.
Emulator: answers `OwnershipControlsNotFoundError` / "The bucket ownership controls were not found" for a freshly created bucket.
Why: a client that reads ownership controls to decide whether ACL calls are permitted sees "no controls" where AWS reports BucketOwnerEnforced, and so takes the wrong branch — and the bucket-owner-enforced refusal of PutObjectAcl (the thing this check then verifies) can never be set up.
Fix: the default OwnershipControls seeded by CreateBucket and the GetBucketOwnershipControls handler, services/service-com-amazonaws-s3-amazons3/src/ops/get_bucket_ownership_controls.rs.
Traces: events__s3__sqs.obs-s3-3-acl-rejected-under-bucket-owner-enforced-emits-no-event.

## Evidence
Cause 1: five checks report "Check timed out after 60.0s". Each had already succeeded at CreateBucket, CreateQueue, SetQueueAttributes(Policy admitting s3.amazonaws.com and the rule ARN), PutBucketNotificationConfiguration (EventBridgeConfiguration and/or QueueConfiguration), PutRule and PutTargets, and at the data-plane write itself; the subsequent ReceiveMessage polling of the queue(s) never returns a message. Expected: an `aws.s3` EventBridge envelope with the matching detail-type (and detail.object.etag unquoted, detail.reason == "PutObject" for a PutObject) on the EventBridge path, and an `s3:ObjectCreated:*` / `s3:ObjectAcl:Put` / `s3:ObjectTagging:*` record on the classic path — each inside the check's 90-180 s delivery budget.

Cause 2: "botocore.exceptions.ClientError: An error occurred (OwnershipControlsNotFoundError) when calling the GetBucketOwnershipControls operation: The bucket ownership controls were not found", raised where the check asserts "a new bucket must default to BucketOwnerEnforced". Expected: HTTP 200 with OwnershipControls.Rules == [{'ObjectOwnership': 'BucketOwnerEnforced'}].

## Steps to Reproduce
Cause 1:
1. CreateBucket(b); CreateQueue(q); SetQueueAttributes(q, Policy allowing s3.amazonaws.com SendMessage for arn:aws:s3:::b and events.amazonaws.com for the rule ARN).
2. PutBucketNotificationConfiguration(Bucket=b, NotificationConfiguration={'EventBridgeConfiguration': {}}).
3. PutRule(Name=r, EventPattern='{"source":["aws.s3"],"detail-type":["Object Created"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<q arn>}]).
4. PutObject(Bucket=b, Key=k) -> ReceiveMessage(q) never returns a message.
5. Repeat with PutObjectAcl / PutObjectTagging / DeleteObjectTagging / RestoreObject and the matching detail-types, and with a QueueConfiguration instead of EventBridgeConfiguration -> nothing delivered on either path.

Cause 2:
1. CreateBucket(b2) with no ObjectOwnership argument.
2. GetBucketOwnershipControls(Bucket=b2) -> 404/OwnershipControlsNotFoundError instead of 200 with Rules [{'ObjectOwnership': 'BucketOwnerEnforced'}].
