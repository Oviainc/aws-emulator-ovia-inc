Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.sqs-subscription-delivers-the-published-message-unconditionally sns.publish-delivers-json-envelope-to-sqs sns.publish-batch-success-delivery sns.publish-unicode-body-preserved sns.publish-binary-message-attribute-byte-preserved sns.message-attributes-survive-delivery-byte-for-byte sns.subscribe-raw-message-delivery-end-to-end sns.raw-message-delivery-strips-the-envelope-for-only-that-subscription sns.raw-delivery-toggle-on-a-live-subscription-switches-the-delivered-format sns.raw-delivery-normalises-number-attributes-the-envelope-carries-verbatim sns.raw-message-delivery-drops-message-attributes-for-an-https-subscriber-but-keeps-them-for-sqs sns.e2e-topic-fanout-filter-teardown-flow sns.publish-still-delivers-to-siblings-when-a-subscribed-queue-is-deleted sns.recreated-queue-with-the-same-name-resumes-delivery-on-the-old-subscription sns.standard-topic-message-group-id-reaches-the-queue-as-its-message-group-id sns.string-array-message-attribute-matches-a-member-and-binary-is-ignored sns.custom-attribute-type-labels-and-string-array-survive-both-delivery-modes sns.message-structure-json-selects-the-protocol-body-and-falls-back-to-default sns.publish-message-structure-json-semantics sns.notification-signature-verifies-against-the-signing-certificate sns.signature-version-two-switches-the-delivered-digest-to-sha256 sns.fifo-delivery-preserves-order-per-group-and-carries-the-group-id sns.fifo-topic-ordering-delivery sns.fifo-topic-preserves-publish-order-and-deduplicates-by-id sns.fifo-topic-accepts-both-queue-types sns.fifo-sequence-numbers-monotonic-within-group sns.fifo-message-group-id-validation sns.fifo-publish-without-a-message-group-id-rejected-a-grouped-publish-delivers sns.fifo-throughput-scope-message-group-narrows-the-deduplication-scope sns.publish-batch-aggregate-size-at-the-exact-262144-byte-boundary sns.publish-batch-content-based-dedup-collapses-duplicate-bodies-in-one-call sns.publish-batch-duplicate-ids-rejected sns.publish-batch-fifo-orders-entries-and-dedups-across-batches sns.publish-batch-validation-is-request-level sns.publish-batch-to-a-deleted-topic-not-found sns.filter-policy-exists-false-and-anything-but-decide-delivery sns.filter-policy-message-body-scope-matches-array-members-and-withholds-non-json sns.filter-policy-scope-switch-on-a-live-subscription-reroutes-delivery sns.clearing-a-filter-policy-with-an-empty-value-restores-unfiltered-delivery sns.lambda-subscription-invocation-carries-the-sns-records-envelope sns.lambda-subscription-the-function-refuses-to-sns-dead-letters-into-the-subscription-dlq sns.cross-region-lambda-subscription-invokes-the-function-in-its-own-region sns.https-endpoint-failing-with-500-is-retried-then-dead-lettered sns.https-raw-message-delivery-posts-the-bare-body-with-the-rawdelivery-header sns.https-subscription-receives-nothing-until-confirmed-then-gets-the-notification sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once sns.firehose-subscription-role-without-put-record-dead-letters-until-granted sns.firehose-subscription-writes-each-notification-envelope-into-the-stream-bucket sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs sns.sqs-failure-feedback-role-logs-the-denied-delivery-while-the-message-never-reaches-the-queue sns.success-feedback-sample-rate-zero-writes-no-log-while-full-rate-logs-every-delivery sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record sns.unsubscribe-stops-delivery-and-attribute-lookup-is-rejected sns.authenticate-on-unsubscribe-refuses-the-anonymous-unsubscribe-url sns.confirm-subscription-token-confirms-only-its-own-subscription-whatever-the-topic-arn sns.confirm-subscription-token-is-reusable-until-unsubscribe-then-not-found sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state sns.deploy-cfn-topic-publish-subscribe-flow sns.deploy-cfn-stack-update-changes-a-filter-policy-in-place
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.sqs-subscription-delivers-the-published-message-unconditionally|_await_body#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.publish-delivers-json-envelope-to-sqs|_poll_queue_for_messages#9|Timeout|time.sleep(interval_s)
Issue-Key: sns.publish-batch-success-delivery|_poll_until#13|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: sns.publish-unicode-body-preserved|_poll_for_body_containing#16|AssertionError|raise AssertionError(f"message containing {needle!r} never arrived")
Issue-Key: sns.publish-binary-message-attribute-byte-preserved|_poll_for_body#16|AssertionError|raise AssertionError(f"message {body_token!r} never arrived")
Issue-Key: sns.message-attributes-survive-delivery-byte-for-byte|_await_body#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.subscribe-raw-message-delivery-end-to-end|_poll_queue_for_messages#10|Timeout|time.sleep(interval_s)
Issue-Key: sns.raw-message-delivery-strips-the-envelope-for-only-that-subscription|_await_body#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.raw-delivery-toggle-on-a-live-subscription-switches-the-delivered-format|_receive_keyed#9|Timeout|resp = sqs.receive_message(
Issue-Key: sns.raw-delivery-normalises-number-attributes-the-envelope-carries-verbatim|_receive_keyed#10|Timeout|resp = sqs.receive_message(
Issue-Key: sns.raw-message-delivery-drops-message-attributes-for-an-https-subscriber-but-keeps-them-for-sqs|_captured#7|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.e2e-topic-fanout-filter-teardown-flow|_drain_once#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.publish-still-delivers-to-siblings-when-a-subscribed-queue-is-deleted|_await_bodies#18|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: sns.recreated-queue-with-the-same-name-resumes-delivery-on-the-old-subscription|_collect_keyed#11|Timeout|resp = sqs.receive_message(
Issue-Key: sns.standard-topic-message-group-id-reaches-the-queue-as-its-message-group-id|_drain_until#14|Timeout|for msg in sqs.receive_message(
Issue-Key: sns.string-array-message-attribute-matches-a-member-and-binary-is-ignored|_await_envelopes#16|Timeout|resp = sqs.receive_message(
Issue-Key: sns.custom-attribute-type-labels-and-string-array-survive-both-delivery-modes|_receive_keyed#14|Timeout|resp = sqs.receive_message(
Issue-Key: sns.message-structure-json-selects-the-protocol-body-and-falls-back-to-default|_await_envelopes#16|Timeout|resp = sqs.receive_message(
Issue-Key: sns.publish-message-structure-json-semantics|_poll_for_body#7|Timeout|resp = sqs.receive_message(
Issue-Key: sns.notification-signature-verifies-against-the-signing-certificate|_await_body#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.signature-version-two-switches-the-delivered-digest-to-sha256|_await_envelope#7|Timeout|resp = sqs.receive_message(
Issue-Key: sns.fifo-delivery-preserves-order-per-group-and-carries-the-group-id|run#11|Timeout|resp = sqs.receive_message(
Issue-Key: sns.fifo-topic-ordering-delivery|_poll_queue_for_messages#10|Timeout|time.sleep(interval_s)
Issue-Key: sns.fifo-topic-preserves-publish-order-and-deduplicates-by-id|_drain_messages#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.fifo-topic-accepts-both-queue-types|_poll_queue_for_messages#15|Timeout|time.sleep(interval_s)
Issue-Key: sns.fifo-sequence-numbers-monotonic-within-group|_poll_queue_for_messages#10|Timeout|time.sleep(interval_s)
Issue-Key: sns.fifo-message-group-id-validation|_drain_until_body#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.fifo-publish-without-a-message-group-id-rejected-a-grouped-publish-delivers|_drain_until_body#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.fifo-throughput-scope-message-group-narrows-the-deduplication-scope|_await_bodies#17|Timeout|resp = sqs.receive_message(
Issue-Key: sns.publish-batch-aggregate-size-at-the-exact-262144-byte-boundary|_await_bodies#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.publish-batch-content-based-dedup-collapses-duplicate-bodies-in-one-call|_drain_until_body#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.publish-batch-duplicate-ids-rejected|_drain_until_all#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.publish-batch-fifo-orders-entries-and-dedups-across-batches|run#21|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: sns.publish-batch-validation-is-request-level|_drain_until_body#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.publish-batch-to-a-deleted-topic-not-found|_drain_until#11|Timeout|for msg in sqs.receive_message(
Issue-Key: sns.filter-policy-exists-false-and-anything-but-decide-delivery|_await_bodies#17|Timeout|resp = sqs.receive_message(
Issue-Key: sns.filter-policy-message-body-scope-matches-array-members-and-withholds-non-json|_await_bodies#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: sns.filter-policy-scope-switch-on-a-live-subscription-reroutes-delivery|_receive_keyed#10|Timeout|resp = sqs.receive_message(
Issue-Key: sns.clearing-a-filter-policy-with-an-empty-value-restores-unfiltered-delivery|_drain_until#12|Timeout|for msg in sqs.receive_message(
Issue-Key: sns.lambda-subscription-invocation-carries-the-sns-records-envelope|_captured#13|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.lambda-subscription-the-function-refuses-to-sns-dead-letters-into-the-subscription-dlq|_drain_until#12|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2).get("Messages", []):
Issue-Key: sns.cross-region-lambda-subscription-invokes-the-function-in-its-own-region|_captured#14|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.https-endpoint-failing-with-500-is-retried-then-dead-lettered|_captured#7|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.https-raw-message-delivery-posts-the-bare-body-with-the-rawdelivery-header|_captured#5|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.https-subscription-receives-nothing-until-confirmed-then-gets-the-notification|_captured#5|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once|_captured#3|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.firehose-subscription-role-without-put-record-dead-letters-until-granted|_drain_until#15|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2).get("Messages", []):
Issue-Key: sns.firehose-subscription-writes-each-notification-envelope-into-the-stream-bucket|_records#9|Timeout|time.sleep(5)
Issue-Key: sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs|run#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.sqs-failure-feedback-role-logs-the-denied-delivery-while-the-message-never-reaches-the-queue|_wait_delivered#24|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.success-feedback-sample-rate-zero-writes-no-log-while-full-rate-logs-every-delivery|_wait_delivered#20|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record|_wait_delivered#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.unsubscribe-stops-delivery-and-attribute-lookup-is-rejected|_drain_until_all#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.authenticate-on-unsubscribe-refuses-the-anonymous-unsubscribe-url|_captured#6|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.confirm-subscription-token-confirms-only-its-own-subscription-whatever-the-topic-arn|_captured#6|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.confirm-subscription-token-is-reusable-until-unsubscribe-then-not-found|_captured#5|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state|_refusal_once_settled#12|Timeout|time.sleep(15)
Issue-Key: sns.deploy-cfn-topic-publish-subscribe-flow|_poll_for_envelope#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns.deploy-cfn-stack-update-changes-a-filter-policy-in-place|_drain_until#9|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS performs no out-of-band delivery: a published message never reaches any subscribed endpoint (4 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
SNS accepts Publish/PublishBatch and returns a MessageId, but nothing is ever handed to the subscribed endpoint, so every behavioral property that is only observable downstream of a publish fails. 60 of the suite's sns checks are grounded here.

Cause 1 of 4: no fan-out to subscribed endpoints.
AWS: Smithy/Publish doc — "If you send a message to a topic, Amazon SNS delivers the message to each endpoint that is subscribed to the topic. ... When a messageId is returned, the message is saved and Amazon SNS immediately delivers it to subscribers." For protocol `sqs` the queue receives a JSON envelope with Type=Notification, the verbatim Message, the MessageId Publish returned, TopicArn, Timestamp, SignatureVersion, Signature and SigningCertURL (raw-delivery subscriptions instead receive the bare body plus native SQS message attributes); for `lambda` the function is invoked with the Records/Sns envelope; for `https` the endpoint is POSTed; for `firehose` the record is put to the stream.
Emulator: Publish validates, stores a PublishedMessage and calls `store.record_delivery(...)` for each confirmed subscription — a write into SNS's OWN state only. No request is ever made to the subscribed SQS queue, Lambda function, HTTPS endpoint or Firehose stream. Checks with bounded waits report the queue empty ("condition not met within 30s. Last result: []", "message ... never arrived"); the rest hang to the 60 s cap.
Why: every SNS-based pipeline is silently inert. A client publishes, gets a 200 and a MessageId, and the consumer never sees the message — fan-out, raw delivery, FIFO ordering/sequencing as seen by the consumer, filter-policy evaluation, message-attribute preservation and envelope/signature shape are all unobservable and untestable against the emulator.
Fix: the Publish handler's fan-out step and the delivery path it should call (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs, the recipients loop at the end of `handle`), plus the same step in PublishBatch; delivery must cross into the subscribed service rather than terminate in SNS state.
Traces: sns.sqs-subscription-delivers-the-published-message-unconditionally, sns.publish-delivers-json-envelope-to-sqs, sns.publish-batch-success-delivery, sns.publish-unicode-body-preserved, sns.publish-binary-message-attribute-byte-preserved, sns.message-attributes-survive-delivery-byte-for-byte, the raw-delivery group, the FIFO delivery group, the filter-policy delivery group, the lambda/https/firehose subscription groups.

Cause 2 of 4: delivery-status logging never happens.
AWS: with <Protocol>SuccessFeedbackRoleArn / FailureFeedbackRoleArn and a sample rate set on the topic, SNS writes a delivery record to CloudWatch Logs for each (sampled) delivery attempt, and writes none at 0% sample rate or when the role lacks logs permission.
Emulator: no delivery attempt is made at all, so no log group/record is ever produced and the checks wait forever.
Why: a client cannot observe or alarm on delivery success/failure.
Fix: the delivery path's status-logging hook (same SNS delivery component as cause 1).
Traces: sns.sqs-success-feedback-role-writes-the-delivery-record-to-cloudwatch-logs, sns.sqs-failure-feedback-role-logs-the-denied-delivery-while-the-message-never-reaches-the-queue, sns.success-feedback-sample-rate-zero-writes-no-log-while-full-rate-logs-every-delivery, sns.feedback-role-without-logs-permission-still-delivers-and-writes-no-log-record.

Cause 3 of 4: no confirmation request is ever sent to an https subscriber, so the ConfirmSubscription lifecycle cannot run.
AWS: Subscribe on an https endpoint POSTs a SubscriptionConfirmation message carrying a Token; ConfirmSubscription(Token) (modeled errors InvalidParameterException 400 / NotFoundException 404) moves the subscription out of PendingConfirmation, the token stays usable until Unsubscribe, and AuthenticateOnUnsubscribe=true makes the unsubscribe URL refuse an anonymous GET.
Emulator: the endpoint is never contacted, no token exists, and the checks hang to the cap.
Why: https/email subscriptions can never be confirmed, so no non-SQS subscriber can be driven end to end.
Fix: the Subscribe handler's confirmation-dispatch step and the ConfirmSubscription handler.
Traces: sns.https-subscription-receives-nothing-until-confirmed-then-gets-the-notification, sns.confirm-subscription-token-confirms-only-its-own-subscription-whatever-the-topic-arn, sns.confirm-subscription-token-is-reusable-until-unsubscribe-then-not-found, sns.authenticate-on-unsubscribe-refuses-the-anonymous-unsubscribe-url.

Cause 4 of 4: an undeliverable notification is never dead-lettered and never retried.
AWS: a subscription RedrivePolicy sends a notification that exhausted its delivery policy retries to the dead-letter queue; a zero-retry delivery policy attempts exactly once.
Emulator: there is no attempt, no retry and no dead-letter message.
Why: redrive/retry configuration appears accepted but has no effect, hiding delivery failures.
Fix: the retry/redrive stage of the SNS delivery path.
Traces: sns.https-endpoint-failing-with-500-is-retried-then-dead-lettered, sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once, sns.lambda-subscription-the-function-refuses-to-sns-dead-letters-into-the-subscription-dlq, sns.firehose-subscription-role-without-put-record-dead-letters-until-granted.

Related open issues (narrower symptoms of the same missing path, not duplicates of this one): 20261024T102105Z-kms-sns-sqs-sns-never-delivers-to-sqs-subscribers-when-kms.md, 20261025T000319Z-iam-sns-sns-performs-no-iam-authorization-publish.md, 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md.

## Evidence
Expected (AWS): a confirmed sqs subscription receives {"Type":"Notification","MessageId":<the id Publish returned>,"TopicArn":...,"Message":<verbatim>,"Timestamp":...,"SignatureVersion":...,"Signature":...,"SigningCertURL":...} within seconds of Publish.
Actual (emulator):
- sns.publish-batch-success-delivery: AssertionError: condition not met within 30s. Last result: []
- sns.publish-unicode-body-preserved: AssertionError: message containing 'sim2real-unicode-81325f6a1e6c' never arrived
- sns.publish-binary-message-attribute-byte-preserved: AssertionError: message 'sim2real-binattr-body-12111e7b447f' never arrived
- 50+ further delivery checks: Error: Check timed out after 60.0s
Publish itself answers 200 with a MessageId in every case; src/ops/publish.rs only calls store.record_delivery(&recipient, published.clone()) into SNS's own store.

## Steps to Reproduce
1. CreateTopic; CreateQueue and set a queue Policy allowing sns.amazonaws.com:sqs:SendMessage for that topic ARN.
2. Subscribe(Protocol="sqs", Endpoint=<queue arn>, ReturnSubscriptionArn=true); poll GetSubscriptionAttributes until PendingConfirmation=false (it is).
3. Publish(TopicArn, Message="hello") — note the returned MessageId.
4. ReceiveMessage on the queue with WaitTimeSeconds=5 in a loop for 60 s: no message ever arrives. Same with Protocol="lambda", "https" and "firehose".
