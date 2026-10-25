Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.publish-empty-message-fails sns.publish-message-size-limit-enforced sns.publish-of-exactly-256-kib-delivered-intact-and-one-byte-more-rejected sns.publish-without-target-fails sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn sns.subject-over-100-characters-rejected-and-the-limit-counts-characters sns.subject-with-a-line-break-or-control-character-rejected sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected sns.standard-topic-rejects-fifo-dedup-id sns.publish-batch-too-many-entries-rejected sns.publish-batch-too-long-in-aggregate-rejected-and-nothing-delivered sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected sns.set-topic-attribute-invalid-policy-rejected sns.set-topic-attributes-unknown-name-fails sns.topic-feedback-sample-rate-out-of-range-rejected sns.topic-maximum-message-size-round-trip-and-bounds-rejected sns.set-topic-attributes-invalid-signature-version-rejected sns.set-topic-attributes-invalid-tracing-config-rejected sns.topic-set-attribute-immutable-rejected sns.subscribe-invalid-filter-policy-fails sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing sns.filter-policy-scope-constrains-policy-shape sns.subscription-redrive-policy-non-sqs-target-rejected sns.subscription-redrive-policy-round-trip-and-replace sns.list-tags-for-nonexistent-topic-fails sns.tag-resource-nonexistent-topic-fails sns.tag-resource-lifecycle-and-limit sns.add-permission-malformed-account-id-rejected sns.create-topic-idempotent-and-conflict
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.publish-empty-message-fails|run#7|AssertionError|assert error.get('Message') == 'Invalid parameter: Empty message', (
Issue-Key: sns.publish-message-size-limit-enforced|run#6|AssertionError|assert message == 'Invalid parameter: Message too long', (
Issue-Key: sns.publish-of-exactly-256-kib-delivered-intact-and-one-byte-more-rejected|run#9|AssertionError|assert (code, message) == ("InvalidParameter", "Invalid parameter: Message too long"), (
Issue-Key: sns.publish-without-target-fails|run#5|AssertionError|assert message == 'Invalid parameter: TopicArn or TargetArn Reason: no value for required parameter', (
Issue-Key: sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn|run#14|AssertionError|assert message == ("Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn "
Issue-Key: sns.subject-over-100-characters-rejected-and-the-limit-counts-characters|run#5|AssertionError|assert (code, message) == ("InvalidParameter", "Invalid parameter: Subject"), (
Issue-Key: sns.subject-with-a-line-break-or-control-character-rejected|run#6|AssertionError|assert (code, message) == ("InvalidParameter", "Invalid parameter: Subject"), (
Issue-Key: sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected|run#15|AssertionError|assert (code, message) == ("InvalidParameter", "Invalid parameter: The MessageGroupId parameter can only include "
Issue-Key: sns.standard-topic-rejects-fifo-dedup-id|run#10|AssertionError|assert message == "Invalid parameter: MessageDeduplicationId Reason: The request includes MessageDeduplicationId parameter that is not valid for this topic type
Issue-Key: sns.publish-batch-too-many-entries-rejected|run#11|AssertionError|assert message == expected_message, (
Issue-Key: sns.publish-batch-too-long-in-aggregate-rejected-and-nothing-delivered|run#10|AssertionError|assert message == "The length of all the messages put together is more than the limit.", f"unexpected refusal message {message!r}"
Issue-Key: sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected|run#14|AssertionError|assert (code, message) == ("InvalidBatchEntryId",
Issue-Key: sns.set-topic-attribute-invalid-policy-rejected|run#7|AssertionError|assert message == 'Invalid parameter: Policy Error: null', (
Issue-Key: sns.set-topic-attributes-unknown-name-fails|run#6|AssertionError|assert message == 'Invalid parameter: AttributeName', (
Issue-Key: sns.topic-feedback-sample-rate-out-of-range-rejected|run#4|AssertionError|assert message == expected_message, (
Issue-Key: sns.topic-maximum-message-size-round-trip-and-bounds-rejected|run#13|AssertionError|assert message == expected_message, (
Issue-Key: sns.set-topic-attributes-invalid-signature-version-rejected|run#7|AssertionError|assert message == "Invalid parameter: SignatureVersion: Invalid value '[3]'. Must be 1 or 2.", f"unexpected message for SignatureVersion=3: {message!r}"
Issue-Key: sns.set-topic-attributes-invalid-tracing-config-rejected|run#7|AssertionError|assert message == "Invalid parameter: Invalid tracing config value: Sampled", f"unexpected message for TracingConfig=Sampled: {message!r}"
Issue-Key: sns.topic-set-attribute-immutable-rejected|run#7|AssertionError|assert message == "Invalid parameter: AttributeName", (
Issue-Key: sns.subscribe-invalid-filter-policy-fails|run#11|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing|run#6|AssertionError|assert (code, message) == (
Issue-Key: sns.filter-policy-scope-constrains-policy-shape|run#11|AssertionError|assert msg_scope == expected, f"expected message {expected!r}, got {msg_scope!r}"
Issue-Key: sns.subscription-redrive-policy-non-sqs-target-rejected|run#9|AssertionError|assert message == "Invalid parameter: RedrivePolicy: deadLetterTargetArn must be a SQS queue", f"unexpected message for a topic ARN as dead-letter target: {mess
Issue-Key: sns.subscription-redrive-policy-round-trip-and-replace|run#17|AssertionError|assert message == "Invalid parameter: RedrivePolicy: missing required value", (
Issue-Key: sns.list-tags-for-nonexistent-topic-fails|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sns.tag-resource-nonexistent-topic-fails|run#11|AssertionError|assert message == "Resource does not exist", f"unexpected ResourceNotFound message for TagResource: {message!r}"
Issue-Key: sns.tag-resource-lifecycle-and-limit|run#14|AssertionError|assert message == "Could not complete request: tag quota of per resource exceeded", f"unexpected TagLimitExceeded message: {message!r}"
Issue-Key: sns.add-permission-malformed-account-id-rejected|run#9|AssertionError|assert message == "Invalid parameter: A provided account ID is not valid", f"unexpected message for a malformed account id: {message!r}"
Issue-Key: sns.create-topic-idempotent-and-conflict|run#11|AssertionError|assert message == "Invalid parameter: Attributes Reason: Topic already exists with different attributes", (
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS refusal messages do not match the fixed text AWS returns for the same condition, across ten operation families (10 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
In all of the causes below the HTTP status and the error code already match AWS (400 / InvalidParameter, ResourceNotFound, TagLimitExceeded, InvalidBatchEntryId, BatchRequestTooLong, TooManyEntriesInBatchRequest). What diverges is the message, and in each case the message is a REQUIREMENT because AWS emits a fixed text for that condition — the text each check recorded from real AWS — rather than a free-form diagnostic. The emulator instead emits its own "<Member> Reason: ..." diagnostic. A client that string-matches a documented message (or a test suite asserting it) breaks.
Fix for every cause below: the message produced by that operation's validation (the message-construction helpers in the SNS ops/state validation layer), leaving status and code as they are.

Cause 1 of 10: Publish destination/body rules. AWS: 'Invalid parameter: Empty message'; 'Invalid parameter: Message too long'; 'Invalid parameter: TopicArn or TargetArn Reason: no value for required parameter'. Emulator: 'Invalid parameter: Message - must not be empty'; "Invalid parameter: Message - too long: 263168 bytes exceeds the topic's limit of 262144 bytes"; 'Invalid parameter: exactly one of TopicArn, TargetArn or PhoneNumber must be specified' / '... are mutually exclusive'. Traces: sns.publish-empty-message-fails, sns.publish-message-size-limit-enforced, sns.publish-of-exactly-256-kib-delivered-intact-and-one-byte-more-rejected, sns.publish-without-target-fails, sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn.

Cause 2 of 10: Subject rules. AWS: 'Invalid parameter: Subject' for both a 101-character Subject and a Subject containing a line break. Emulator: 'Invalid parameter: Subject - must be text with no control characters and fewer than 100 characters long'. Traces: sns.subject-over-100-characters-rejected-and-the-limit-counts-characters, sns.subject-with-a-line-break-or-control-character-rejected.

Cause 3 of 10: FIFO id rules on Publish. AWS refuses a 129-character MessageGroupId / MessageDeduplicationId and a MessageDeduplicationId on a standard topic with its own fixed texts; the emulator answers 'Invalid parameter: MessageGroupId - must be 1 to 128 alphanumeric or punctuation characters' and 'Invalid parameter: MessageDeduplicationId - applies only to FIFO topics'. Traces: sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected, sns.standard-topic-rejects-fifo-dedup-id.

Cause 4 of 10: PublishBatch request-level messages. AWS: 'The batch request contains more entries than permissible.'; the recorded BatchRequestTooLong and InvalidBatchEntryId texts. Emulator appends computed detail: 'The batch request contains more entries than permissible: 11 entries were sent, and the maximum is 10.', "The length of all the batch messages put together is more than the limit: 307200 bytes exceeds the topic's limit of 262144 bytes.", "...an Id has 1 to 80 alphanumeric, hyphen or underscore characters, and this one has 81.". Traces: sns.publish-batch-too-many-entries-rejected, sns.publish-batch-too-long-in-aggregate-rejected-and-nothing-delivered, sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected.

Cause 5 of 10: SetTopicAttributes value messages. AWS: 'Invalid parameter: Policy Error: null'; 'Invalid parameter: AttributeName'; 'Invalid parameter: SQSSuccessFeedbackSampleRate: 101 value provided is not an integer between 0-100'; 'Invalid parameter: MaximumMessageSize: 1023 is not an integer between 1024 and 1048576 bytes'; fixed texts for SignatureVersion and TracingConfig. Emulator: the uniform 'Invalid parameter: AttributeValue Reason: <Name> - the value must be ...' / 'Invalid parameter: AttributeName Reason: <name> is not a topic attribute this operation sets'. Traces: sns.set-topic-attribute-invalid-policy-rejected, sns.set-topic-attributes-unknown-name-fails, sns.topic-feedback-sample-rate-out-of-range-rejected, sns.topic-maximum-message-size-round-trip-and-bounds-rejected, sns.set-topic-attributes-invalid-signature-version-rejected, sns.set-topic-attributes-invalid-tracing-config-rejected, sns.topic-set-attribute-immutable-rejected.

Cause 6 of 10: Subscribe attribute messages. AWS returns the Jackson-derived parse text for a malformed FilterPolicy and its own fixed text for an unknown attribute name; the emulator returns 'Invalid parameter: Attributes Reason: FilterPolicy - the value must be a filter policy document ...' and '... unknown attribute name: RawMessgeDelivery'. Traces: sns.subscribe-invalid-filter-policy-fails, sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing.

Cause 7 of 10: FilterPolicyScope message. AWS: 'Invalid parameter: FilterPolicyScope: Invalid value [MessageHeaders]. Please use either MessageBody or MessageAttributes'. Emulator: 'Invalid parameter: AttributeValue Reason: FilterPolicyScope - the value must be one of ["MessageAttributes", "MessageBody"]'. Traces: sns.filter-policy-scope-constrains-policy-shape.

Cause 8 of 10: RedrivePolicy messages on SetSubscriptionAttributes. AWS has fixed texts for a non-SQS dead-letter target and for an empty RedrivePolicy; the emulator returns its 'AttributeValue Reason: RedrivePolicy - ...' diagnostics. Traces: sns.subscription-redrive-policy-non-sqs-target-rejected, sns.subscription-redrive-policy-round-trip-and-replace.

Cause 9 of 10: resource-not-found and tag-limit texts. AWS: 'Resource does not exist' for ListTagsForResource/TagResource on a missing topic, and its fixed TagLimitExceeded text. Emulator: 'Topic does not exist' and "Can't add more than 50 tags to a topic". Traces: sns.list-tags-for-nonexistent-topic-fails, sns.tag-resource-nonexistent-topic-fails, sns.tag-resource-lifecycle-and-limit.

Cause 10 of 10: AddPermission/CreateTopic conflict texts. AWS: the fixed 'Invalid parameter: AWSAccountId' text for a malformed account id, and the fixed text for a name reused with different attributes. Emulator: 'Invalid parameter: AWSAccountId Reason: an Amazon Web Services account id is 12 digits: not-an-account' and 'Invalid parameter: Attributes Reason: a topic with this name already exists with a different DisplayName'. Traces: sns.add-permission-malformed-account-id-rejected, sns.create-topic-idempotent-and-conflict.

## Evidence
Status and code agree with AWS in every case; only the message differs. Examples from the replay:
- sns.publish-empty-message-fails: expected the exact message 'Invalid parameter: Empty message', got 'Invalid parameter: Message - must not be empty'
- sns.publish-message-size-limit-enforced: expected the exact message 'Invalid parameter: Message too long', got "Invalid parameter: Message - too long: 263168 bytes exceeds the topic's limit of 262144 bytes"
- sns.publish-batch-too-many-entries-rejected: expected the exact message 'The batch request contains more entries than permissible.', got 'The batch request contains more entries than permissible: 11 entries were sent, and the maximum is 10.'
- sns.set-topic-attribute-invalid-policy-rejected: expected the exact message 'Invalid parameter: Policy Error: null', got 'Invalid parameter: AttributeValue Reason: Policy - the value must be an access policy document with a Statement list'
- sns.list-tags-for-nonexistent-topic-fails: expected message 'Resource does not exist', got 'Topic does not exist'
- sns.filter-policy-scope-constrains-policy-shape: expected message 'Invalid parameter: FilterPolicyScope: Invalid value [MessageHeaders]. Please use either MessageBody or MessageAttributes', got 'Invalid parameter: AttributeValue Reason: FilterPolicyScope - the value must be one of ["MessageAttributes", "MessageBody"]'

## Steps to Reproduce
1. Publish(TopicArn, Message="") -> 400 InvalidParameter with 'Invalid parameter: Message - must not be empty' (AWS: 'Invalid parameter: Empty message').
2. Publish with a 262,145-byte body -> message naming byte counts (AWS: 'Invalid parameter: Message too long').
3. PublishBatch with 11 entries -> message with the counts appended (AWS: 'The batch request contains more entries than permissible.').
4. SetTopicAttributes(AttributeName="Policy", AttributeValue="null") -> 'AttributeValue Reason: ...' (AWS: 'Invalid parameter: Policy Error: null').
5. ListTagsForResource on a nonexistent topic -> 'Topic does not exist' (AWS: 'Resource does not exist').
Compare Error.Message in each response with the text each named check asserts.
