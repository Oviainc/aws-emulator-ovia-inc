Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.fifo-deduplication-collapses-duplicate-publish sns.fifo-concurrent-duplicate-publish-dedup sns.publish-batch-entry-group-or-dedup-id-over-128-characters-rejects-the-whole-batch sns.publish-batch-one-invalid-fifo-entry-rejected-and-no-sibling-delivered sns.fifo-archive-replay-redelivers-archived-messages-to-a-new-subscription sns.replay-policy-on-a-topic-without-an-archive-policy-rejected sns.op-topic-attribute-defaults-and-effective-delivery-policy sns.confirmed-sqs-subscription-attributes-and-listing-agree-field-for-field sns.filter-policy-scope-set-without-a-policy-is-hidden-until-a-policy-is-set
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.fifo-deduplication-collapses-duplicate-publish|run#9|AssertionError|assert second.get("MessageId") == first.get("MessageId"), (
Issue-Key: sns.fifo-concurrent-duplicate-publish-dedup|run#16|AssertionError|assert len(seqs) == 1, (
Issue-Key: sns.publish-batch-entry-group-or-dedup-id-over-128-characters-rejects-the-whole-batch|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.publish-batch-one-invalid-fifo-entry-rejected-and-no-sibling-delivered|_error#11|AssertionError|raise AssertionError(
Issue-Key: sns.fifo-archive-replay-redelivers-archived-messages-to-a-new-subscription|run#8|AssertionError|assert beginning, f"an archiving topic must report BeginningArchiveTime, got {attrs!r}"
Issue-Key: sns.replay-policy-on-a-topic-without-an-archive-policy-rejected|run#6|AssertionError|assert beginning, "the archiving topic must report BeginningArchiveTime"
Issue-Key: sns.op-topic-attribute-defaults-and-effective-delivery-policy|run#31|KeyError|assert fifo_attrs["ContentBasedDeduplication"] == "false", fifo_attrs
Issue-Key: sns.confirmed-sqs-subscription-attributes-and-listing-agree-field-for-field|run#10|AssertionError|assert got == base, (
Issue-Key: sns.filter-policy-scope-set-without-a-policy-is-hidden-until-a-policy-is-set|run#11|AssertionError|assert filter_keys(early) == {}, (
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS FIFO deduplication does not deduplicate, PublishBatch validates per entry instead of per request, and four attribute read-backs are wrong (6 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishBatch

## Description
Cause 1 of 6: FIFO deduplication never collapses a duplicate publish.
AWS: Smithy Publish$MessageDeduplicationId — "If a message with a particular MessageDeduplicationId is sent successfully" within the 5-minute interval, subsequent copies are accepted but not delivered, and Publish returns the ORIGINAL MessageId. With ContentBasedDeduplication, identical bodies dedupe the same way, including for concurrent publishers.
Emulator: each duplicate publish gets a fresh MessageId and its own SequenceNumber (five distinct sequence numbers for five concurrent duplicates).
Why: a client relying on at-most-once FIFO semantics sees every retry as a new message.
Fix: the deduplication state/lookup in the SNS store used by the Publish and PublishBatch handlers.
Traces: sns.fifo-deduplication-collapses-duplicate-publish, sns.fifo-concurrent-duplicate-publish-dedup.

Cause 2 of 6: PublishBatch applies per-message validation per entry instead of to the whole request.
AWS: an entry whose MessageGroupId/MessageDeduplicationId is invalid (e.g. 129 characters) or missing on a FIFO topic makes the WHOLE PublishBatch request fail with a top-level 400 InvalidParameterException, and no sibling entry is published.
Emulator: the call returns 200 with the bad entry in `Failed` (Code=InvalidParameter, SenderFault=true) and the siblings in `Successful` with MessageIds and SequenceNumbers.
Why: siblings AWS would never have accepted are published, so a batch is partially applied where AWS is all-or-nothing.
Fix: hoist the entry validation in the PublishBatch handler to a request-level check before any entry is recorded.
Traces: sns.publish-batch-entry-group-or-dedup-id-over-128-characters-rejects-the-whole-batch, sns.publish-batch-one-invalid-fifo-entry-rejected-and-no-sibling-delivered.

Cause 3 of 6: a topic with an ArchivePolicy does not report BeginningArchiveTime.
AWS: once an archive policy is in force, GetTopicAttributes includes BeginningArchiveTime (and a ReplayPolicy on a topic without an archive policy is refused with InvalidParameter).
Emulator: GetTopicAttributes returns ArchivePolicy but no BeginningArchiveTime key.
Why: a client cannot discover the replayable window, and replay configuration cannot be validated.
Fix: the GetTopicAttributes serializer/state for archive-enabled topics.
Traces: sns.fifo-archive-replay-redelivers-archived-messages-to-a-new-subscription, sns.replay-policy-on-a-topic-without-an-archive-policy-rejected.

Cause 4 of 6: GetTopicAttributes omits ContentBasedDeduplication on a FIFO topic.
AWS: a FIFO topic always reports ContentBasedDeduplication (default "false") among its attributes.
Emulator: the key is absent (the check raises KeyError: 'ContentBasedDeduplication').
Why: a client reading the attribute to decide whether to send its own dedup id gets a missing key instead of a default.
Fix: the topic default attributes in SNS state / the GetTopicAttributes handler.
Traces: sns.op-topic-attribute-defaults-and-effective-delivery-policy.

Cause 5 of 6: GetSubscriptionAttributes omits SubscriptionPrincipal.
AWS: a confirmed plain sqs subscription reports exactly ConfirmationWasAuthenticated, Endpoint, Owner, PendingConfirmation, Protocol, RawMessageDelivery, SubscriptionArn, SubscriptionPrincipal, TopicArn.
Emulator: every key but SubscriptionPrincipal is present.
Why: a client auditing who created a subscription has no value to read, and attribute round-trips do not agree with the listing.
Fix: the GetSubscriptionAttributes handler/state (record the subscribing principal at Subscribe time).
Traces: sns.confirmed-sqs-subscription-attributes-and-listing-agree-field-for-field.

Cause 6 of 6: FilterPolicyScope is reported for a subscription that has no filter policy.
AWS: a FilterPolicyScope supplied at Subscribe without a FilterPolicy is not reported until a policy is set.
Emulator: GetSubscriptionAttributes reports {'FilterPolicyScope': 'MessageBody'} with no policy present.
Why: a client reads a scope that is not in effect and believes body-scoped filtering is active.
Fix: the subscription attribute projection in the GetSubscriptionAttributes handler.
Traces: sns.filter-policy-scope-set-without-a-policy-is-hidden-until-a-policy-is-set.

## Evidence
- sns.fifo-deduplication-collapses-duplicate-publish: AssertionError: expected duplicate publish to return the original MessageId, got 6b812857-... then 4023b2e4-...
- sns.fifo-concurrent-duplicate-publish-dedup: expected dedup to collapse concurrent duplicates to one SequenceNumber, got {'00000000000000000001'...'00000000000000000005'}
- sns.publish-batch-one-invalid-fifo-entry-rejected-and-no-sibling-delivered: the call was expected to be rejected but SUCCEEDED, returning {'Successful': [e0, e1, e3 with MessageIds], 'Failed': [{'Id': 'e2', 'Code': 'InvalidParameter', 'Message': 'Invalid parameter: The MessageGroupId parameter is required for FIFO topics', 'SenderFault': True}], 'HTTPStatusCode': 200}
- sns.fifo-archive-replay-...: an archiving topic must report BeginningArchiveTime, got {... 'ArchivePolicy': '{"MessageRetentionPeriod": "1"}', 'FifoTopic': 'true'} (no BeginningArchiveTime)
- sns.op-topic-attribute-defaults-and-effective-delivery-policy: KeyError: 'ContentBasedDeduplication'
- sns.confirmed-sqs-subscription-attributes-and-listing-agree-field-for-field: ... got keys [...], mismatched ['SubscriptionPrincipal']
- sns.filter-policy-scope-set-without-a-policy-...: a scope given at Subscribe without a policy must not be reported, got {'FilterPolicyScope': 'MessageBody'}

## Steps to Reproduce
1. CreateTopic(Name="t.fifo", Attributes={"FifoTopic":"true"}); Publish twice with the same MessageDeduplicationId -> two different MessageIds and two SequenceNumbers.
2. PublishBatch on that topic with entries [valid, valid, {no MessageGroupId}] -> 200 with two Successful entries (AWS: whole request 400 InvalidParameter).
3. GetTopicAttributes on the FIFO topic -> no ContentBasedDeduplication key.
4. SetTopicAttributes(ArchivePolicy={"MessageRetentionPeriod":"1"}); GetTopicAttributes -> no BeginningArchiveTime.
5. Subscribe(Protocol="sqs"), then GetSubscriptionAttributes -> no SubscriptionPrincipal.
6. Subscribe with Attributes={"FilterPolicyScope":"MessageBody"} and no FilterPolicy, then GetSubscriptionAttributes -> FilterPolicyScope reported.
