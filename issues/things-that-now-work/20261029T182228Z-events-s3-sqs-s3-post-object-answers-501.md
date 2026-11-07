Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule events__s3__sqs.eventbridge-full-envelope-fields-sqs events__s3__sqs.eventbridge-target-without-queue-policy-silently-fails events__s3__sqs.s3-notification-without-queue-policy-fails events__s3__sqs.sqs-policy-wrong-source-arn-blocks-delivery events__s3__sqs.sqs-queue-policy-sourcearn-scoping events__s3__sqs.eventbridge-dlq-on-target-failure events__s3__sqs.eventbridge-fifo-queue-target-arn events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers events__s3__sqs.s3-multi-event-type-routing-sqs
Issue-Key: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule|run#28|AssertionError|assert status == 201, (
Issue-Key: events__s3__sqs.eventbridge-full-envelope-fields-sqs|run#26|ValueError|event_dt = datetime.strptime(envelope["time"], "%Y-%m-%dT%H:%M:%SZ").replace(
Issue-Key: events__s3__sqs.eventbridge-target-without-queue-policy-silently-fails|run#21|AssertionError|assert not messages, (
Issue-Key: events__s3__sqs.s3-notification-without-queue-policy-fails|_client_error#9|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {resp!r}")
Issue-Key: events__s3__sqs.sqs-policy-wrong-source-arn-blocks-delivery|_client_error#8|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {resp!r}")
Issue-Key: events__s3__sqs.sqs-queue-policy-sourcearn-scoping|_client_error#10|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {resp!r}")
Issue-Key: events__s3__sqs.eventbridge-dlq-on-target-failure|_reemit_until_delivered#15|AssertionError|raise AssertionError(
Issue-Key: events__s3__sqs.eventbridge-fifo-queue-target-arn|_reemit_until_delivered#18|AssertionError|raise AssertionError(
Issue-Key: events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers|poll#30|Timeout|resp = self._sqs.receive_message(QueueUrl=self._url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__s3__sqs.s3-multi-event-type-routing-sqs|_drain#18|Timeout|resp = sqs.receive_message(
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/s3/sqs: S3 POST Object answers 501, the envelope `time` carries sub-second precision, EventBridge ignores the target queue policy, and S3 notification destinations are unvalidated (5 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostObject

## Description
Five root causes.

Cause 1 of 5: S3 browser-based POST Object is not implemented.
AWS: a signed HTML-form POST to the bucket root (POST /<bucket>/) with the policy, signature and file fields uploads the object and answers 204, or the status named by success_action_status (201 here) with a Location/Bucket/Key body. The resulting event is reason "POST Object", so a rule scoped to reason "PutObject" must not receive it.
Emulator: the request answers HTTP 501 with body <Error><Code>NotImplemented</Code><Message>operation POST /<bucket>/ is not implemented in this emulator</Message>...</Error>. The fixed parts of the requirement are the 201/204 status and the success body members; the bucket name and key are values from the request.
Why: every browser/mobile direct-upload flow fails outright. It also destroys the check's logic: an empty PutObject-scoped queue is explained by the failed upload rather than by the reason filter, so the filter cannot be validated at all.
Fix: the S3 POST Object route and its form-policy/signature handling in the S3 crate's request router.
Traces: events__s3__sqs.a-post-object-upload-is-withheld-from-a-putobject-scoped-rule

Cause 2 of 5: the EventBridge envelope's `time` field is serialised with fractional seconds.
AWS: `time` is an ISO-8601 instant with whole-second precision and a trailing Z — %Y-%m-%dT%H:%M:%SZ.
Emulator: the delivered envelope carries "2026-10-29T17:42:35.064099Z"; a consumer parsing the documented format raises "ValueError: time data '2026-10-29T17:42:35.064099Z' does not match format '%Y-%m-%dT%H:%M:%SZ'". The format is the requirement; the instant is a per-run value.
Why: any consumer that parses `time` strictly fails on the first event.
Fix: the EventBridge envelope serializer's timestamp formatting.
Traces: events__s3__sqs.eventbridge-full-envelope-fields-sqs

Cause 3 of 5: EventBridge delivers to an SQS target whose queue policy does not grant events.amazonaws.com.
AWS: with no Allow for the service principal, sqs:SendMessage is refused and nothing is enqueued.
Emulator: the queue receives the message. "AssertionError: queue received a message despite missing resource policy for events.amazonaws.com -- emulator must not fabricate delivery that AWS would reject".
Why: the emulator fabricates a delivery AWS refuses, so missing-policy misconfigurations pass locally and fail in AWS.
Fix: resource-policy authorization in the EventBridge SQS-target invoker.
Traces: events__s3__sqs.eventbridge-target-without-queue-policy-silently-fails

Cause 4 of 5: PutBucketNotificationConfiguration accepts an SQS destination the bucket cannot send to.
AWS: S3 validates the destination at configure time and refuses with HTTP 400 InvalidArgument when the queue policy does not allow sqs:SendMessage for s3.amazonaws.com with the bucket as aws:SourceArn — including when the policy's aws:SourceArn names a different bucket. The message is of the fixed form "Unable to validate the following destination configurations" with the destination ARN from the request.
Emulator: the call succeeds (HTTP 200, empty body). "AssertionError: the call was expected to be refused but succeeded: None" and "...succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Why: the misconfiguration is accepted and then silently never delivers; the caller loses the only signal AWS gives.
Fix: destination validation (queue policy and aws:SourceArn scoping) in the S3 PutBucketNotificationConfiguration handler.
Traces: events__s3__sqs.s3-notification-without-queue-policy-fails, events__s3__sqs.sqs-policy-wrong-source-arn-blocks-delivery, events__s3__sqs.sqs-queue-policy-sourcearn-scoping

Cause 5 of 5: in other configurations the S3→EventBridge→SQS and S3→SQS native paths deliver nothing.
AWS: with EventBridge notifications enabled, PutObject produces one aws.s3 "Object Created" envelope on the bus and a matching rule delivers it to its SQS target (including a FIFO queue target, which requires MessageGroupId); with a native QueueConfiguration, PutObject delivers the S3 event record directly to the queue, one per configured event type; removing the EventBridge configuration stops the envelope while the native queue configuration keeps delivering; a failing target with a DeadLetterConfig dead-letters the envelope.
Emulator: nothing arrives — "AssertionError: event never delivered to <queue url> after 8 attempts of 6.0s each" and "Error: Check timed out after 60.0s" — so the FIFO target, the multi-event-type routing, the configuration-removal differential and the dead-letter path are all ungraded.
Why: both S3 notification mechanisms are unreliable, and this coexists with cause 3 (over-delivery), so no client can reason about S3 eventing at all.
Fix: the S3 bucket-notification dispatcher (EventBridge and SQS destinations, per-event-type routing, configuration removal) and the EventBridge SQS/FIFO target invoker and dead-letter path.
Traces: events__s3__sqs.eventbridge-dlq-on-target-failure, events__s3__sqs.eventbridge-fifo-queue-target-arn, events__s3__sqs.removing-eventbridge-configuration-withholds-envelope-but-native-queue-still-delivers, events__s3__sqs.s3-multi-event-type-routing-sqs

## Evidence
Cause 1: "the signed POST Object upload of '<key>' answered HTTP 501 ... expected 201 (success_action_status=201). Body: b'<?xml version=\"1.0\"?><Error><Code>NotImplemented</Code><Message>operation POST /<bucket>/ is not implemented in this emulator</Message>...'".
Cause 2: "ValueError: time data '2026-10-29T17:42:35.064099Z' does not match format '%Y-%m-%dT%H:%M:%SZ'".
Cause 3: "AssertionError: queue received a message despite missing resource policy for events.amazonaws.com -- emulator must not fabricate delivery that AWS would reject".
Cause 4: "AssertionError: the call was expected to be refused but succeeded: None" and "...succeeded: {'ResponseMetadata': {'RequestId': ..., 'HTTPStatusCode': 200, ...}}" — expected HTTP 400 InvalidArgument naming the unvalidatable destination.
Cause 5: "AssertionError: event never delivered to https://127.0.0.1:<port>/<account>/<queue> after 8 attempts of 6.0s each"; "Error: Check timed out after 60.0s".

## Steps to Reproduce
(1) POST a signed browser-form upload to https://<endpoint>/<bucket>/ with success_action_status=201 — 501 NotImplemented. (2) Enable EventBridge notifications, PutObject, read the SQS target and parse the envelope's `time` with %Y-%m-%dT%H:%M:%SZ — it fails. (3) PutRule + PutTargets with a queue that has no events.amazonaws.com grant, PutEvents — the message arrives. (4) PutBucketNotificationConfiguration with an SQS destination whose queue policy has no grant, or whose aws:SourceArn names another bucket — accepted. (5) Configure a FIFO SQS target / multiple native event types / then remove the EventBridge configuration and PutObject — nothing is delivered.
