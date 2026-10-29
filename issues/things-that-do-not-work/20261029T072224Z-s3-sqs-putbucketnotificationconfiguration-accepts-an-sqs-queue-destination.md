Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key
Issue-Key: s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key|_error#61|AssertionError|raise AssertionError(
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# s3__sqs: PutBucketNotificationConfiguration accepts an SQS queue destination whose resource policy does not grant S3 sqs:SendMessage (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
ONE root cause. 1 CAUSE must be closed before this issue is submitted.

NOTE ON PRIOR ART: the open issue 20261028T220212Z-s3-sqs-putbucketnotificationconfiguration-accepts-an-sqs-destination-it.md names this check id in its Issue-Trace-Ids, but its stored replay key for it (`_error#59|AssertionError|raise AssertionError(`) no longer matches — the refusal helper now raises from `_error` at line 79 — so the replay suite did not mark this failure as OWNED and the fix is un-gated for this check. This issue re-grounds the same cause on the current replay key; it does not cover that issue's second cause (the rolled-back bucket surviving ROLLBACK_COMPLETE), which remains that issue's to close. The open issue 20261025T025317Z-s3-sns-sqs-a-bucket-notification-to-a.md reports the SNS-topic form of the same gap and does not cover the SQS-queue destination.

Cause 1 of 1: PutBucketNotificationConfiguration performs no delivery-permission validation of an SQS queue destination — a configuration naming an existing queue whose resource policy does NOT grant sqs:SendMessage to s3.amazonaws.com is accepted and installed instead of refused. Failing check: s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key.

AWS: the Smithy doc for PutBucketNotificationConfiguration states that after S3 receives the request it "first verifies that any Amazon Simple Notification Service (Amazon SNS) or Amazon Simple Queue Service (Amazon SQS) destination exists, and that the bucket owner has permission to publish to it by sending a test notification", and that "If the message fails, the entire PUT action will fail, and Amazon S3 will not add the configuration to your bucket". The model also binds PutBucketNotificationConfigurationRequest.SkipDestinationValidation to the header x-amz-skip-destination-validation, documented as skipping validation of SQS/SNS/Lambda destinations — the opt-out exists because validation is on by default. The refusal class is HTTP 400 with Error.Code "InvalidArgument" and the error message "Unable to validate the following destination configurations"; that sentence is the fixed text of this class and the variable part is the destination(s) named in the detail, taken from the request's QueueConfigurations. S3 reuses InvalidArgument for an overlapping-prefix configuration ("Configuration is ambiguously defined. Cannot have overlapping suffixes ..."), so the message, not the code alone, is what identifies this class. The refused PUT must leave the previously stored configuration exactly as it was, and the refused destination must receive nothing.

Emulator: the PUT of a two-destination configuration whose second QueueConfiguration names an existing queue with no S3 send grant SUCCEEDS (HTTP 200) and installs the configuration. The replay fails on exactly that: "the call was expected to be rejected by the service but it SUCCEEDED". Every ordering, sequencer, event-name/versionId and eventTime assertion before the negative arm passed, so the notification pipeline itself is live; what is missing is the resource-policy / test-notification half of destination validation. (The emulator does produce this refusal for a destination queue that does not exist at all, per the sibling deploy check, so it is the permission half specifically.)

Why: a client that relies on configuration-time validation to learn that its pipeline is wired gets a 200 for a notification configuration that can never deliver; the failure then surfaces only as permanent silence at the destination, with no error anywhere and no way to tell it from an idle bucket. Conversely, code that catches InvalidArgument in order to repair a queue policy and retry never runs.

Fix: the destination-validation step of the PutBucketNotificationConfiguration handler in the s3 crate — for each TopicConfiguration/QueueConfiguration, resolve the destination and require that its resource policy admit the S3 service principal for this bucket (the documented "send a test notification" step), refusing the WHOLE request with HTTP 400 InvalidArgument / "Unable to validate the following destination configurations" and leaving the stored configuration untouched; honour SkipDestinationValidation (x-amz-skip-destination-validation) as the documented opt-out.

Traces: s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key (frames run() line 367 -> `_error` line 79).

## Evidence
Expected (AWS): put_bucket_notification_configuration(Bucket, NotificationConfiguration={"QueueConfigurations": [<granted queue, prefix 'ord/'>, <ungranted queue, prefix 'deny/'>]}) is refused HTTP 400 with Error.Code "InvalidArgument" and Error.Message "Unable to validate the following destination configurations"; GetBucketNotificationConfiguration still lists only the granted queue; a subsequent PutObject under 'deny/' delivers nothing to the ungranted queue; and the same two-destination shape IS accepted when the second queue's policy does grant sqs:SendMessage to s3.amazonaws.com.

Actual (emulator):
  [mismatch] s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key
  File "check_s3__sqs_s3_sequencer_is_the_only_total_order_over_one_key.py", line 367, in run -> refusal = _error(lambda: s3.put_bucket_notification_configuration(Bucket=bucket, NotificationConfiguration=bad_notify))
  File "...", line 79, in _error -> raise AssertionError(...)
  AssertionError: the call was expected to be rejected by the service but it SUCCEEDED -- a target that accepts this input is more broken than one that returns the wrong error code
All prior arms of the check passed (the granted queue received all 24 events, sequencers were a strict total order over the key, delete markers reported ObjectRemoved:DeleteMarkerCreated, eventTimes were ISO-8601 UTC and no sequencer was shared by different (eventName, versionId) pairs), which establishes the pipeline was live and the only divergence is the missing refusal.

Smithy: PutBucketNotificationConfiguration doc — S3 "verifies that any ... Amazon SQS destination exists, and that the bucket owner has permission to publish to it by sending a test notification"; "If the message fails, the entire PUT action will fail, and Amazon S3 will not add the configuration to your bucket". PutBucketNotificationConfigurationRequest.SkipDestinationValidation -> header x-amz-skip-destination-validation.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. CreateBucket (versioning enabled). Create two SQS queues: one with a resource policy allowing sqs:SendMessage to Principal Service s3.amazonaws.com for the bucket ARN (with aws:SourceArn/aws:SourceAccount conditions), and one with no such grant.
3. PutBucketNotificationConfiguration with QueueConfigurations = [granted queue with Filter prefix 'ord/', ungranted queue with Filter prefix 'deny/'], both on s3:ObjectCreated:* (disjoint prefixes, so the overlapping-prefix refusal cannot be what fires).
4. Observe the call SUCCEED (HTTP 200) instead of being refused HTTP 400 Error.Code 'InvalidArgument' with message 'Unable to validate the following destination configurations'; GetBucketNotificationConfiguration now lists the ungranted queue.
5. Repeat with the second queue's policy granting s3.amazonaws.com sqs:SendMessage — that control arm must be accepted.
6. Replay: s3__sqs.s3-sequencer-is-the-only-total-order-over-one-key.
