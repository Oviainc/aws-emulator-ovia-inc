Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.subject-with-a-line-break-or-control-character-rejected sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.subject-with-a-line-break-or-control-character-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing|run#6|AssertionError|assert (code, message) == (
Issue-Key: sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected|run#14|AssertionError|assert (code, message) == ("InvalidBatchEntryId",
Issue-Key: sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected|run#23|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2,
Issue-Commit: 40e6730291c6ab0b9cc085359e665668413412db

# SNS: four net-new behavioral divergences — Publish accepts an illegal Subject, two refusal messages diverge from the request-derived AWS text, and an accepted 128-char FIFO publish is never delivered (4 causes)

> _Note: this issue was identified against the code base at commit `40e6730291c6ab0b9cc085359e665668413412db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Four distinct root causes in the emulator's SNS implementation, each grounded in one failing behavioral check that is NOT covered by any open issue's replay keys. They are bundled because they all belong to the `sns` service; they must each be closed separately.

Cause 1 of 4: Publish does not validate the Subject's content, so a Subject AWS refuses is accepted and published.
AWS: Smithy `PublishInput$Subject` documentation states the constraint explicitly — "Subjects must be UTF-8 text with no line breaks or control characters, and less than 100 characters long." A Subject containing a line feed, a carriage return, another ASCII control character or DEL, or a Subject that is empty or only spaces, is therefore refused; Publish models `InvalidParameterException` (httpError 400, awsQueryError code `InvalidParameter`), and the recorded refusal message is `Invalid parameter: Subject` (example text, the requirement is 400 / `InvalidParameter`). A Subject containing a tab, or with leading/trailing spaces, is accepted and must be delivered byte-for-byte without trimming.
Emulator: every one of those six illegal Subjects is accepted with HTTP 200 and a MessageId (replay: `expected the call to be refused, but it succeeded: {'MessageId': 'c661eaa6-...', ... 'HTTPStatusCode': 200 ...}` on the very first case, a line feed). No Subject content validation exists at all.
Why: a client that publishes a header-injecting or control-character Subject gets a 200 here and a 400 from AWS, so the failure only appears in production; and because no refusal happens the message is fanned out with a Subject AWS would never carry into an email or JSON envelope.
Fix: the Publish handler's input validation (the Subject rule belongs beside the existing Message/MessageAttributes checks), and the same rule for `PublishBatchRequestEntry$Subject` in PublishBatch.
Traces: sns.subject-with-a-line-break-or-control-character-rejected.
Note: this is a different rule from any of the ten causes already filed in 20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md (that issue covers CreateTopic/SetTopicAttributes/MessageAttributes/Subscribe/filter-policy/size/tag/region rules, and does not carry this check); Subject content is not among them.

Cause 2 of 4: Subscribe's unknown-attribute refusal names the wrong input member and omits the offending attribute name.
AWS: Subscribe takes `Attributes` (`SubscriptionAttributesMap`) and models `InvalidParameterException` (400, awsQueryError `InvalidParameter`). The refusal message is built from the request: it names the `Attributes` member and the rejected key — recorded form `Invalid parameter: Attributes Reason: Unknown attribute <the unrecognised key>` (recorded with `RawMessgeDelivery`, a misspelling of `RawMessageDelivery`). Because the rule that produces this text is request-derived (member + offending key), the shape of the message is the requirement, not the recorded literal.
Emulator: answers `InvalidParameter` with `Invalid parameter: AttributeName` — it names `AttributeName`, which is not a member of SubscribeInput at all (it is a member of SetSubscriptionAttributes), and it never echoes the key that was rejected.
Why: the error gives the caller no way to find the typo; worse, it points at a parameter the caller did not send, so tooling that parses the message to surface "which attribute is wrong" reports a nonexistent field.
Fix: the Subscribe handler's Attributes validation — reject against the known subscription-attribute set and build the message from the `Attributes` member plus the unrecognised key.
Traces: sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing.

Cause 3 of 4: PublishBatch's InvalidBatchEntryId refusal returns the exception's generic doc string instead of the request-derived diagnostic.
AWS: Smithy `PublishBatchRequestEntry$Id` documentation — "This identifier can have up to 80 characters. The following characters are accepted: alphanumeric characters, hyphens(-), and underscores (_)." The modeled error is `InvalidBatchEntryIdException` (httpError 400, awsQueryError code `InvalidBatchEntryId`). The message is built from the request and distinguishes the two rules, naming the offending Id: recorded `The Id of a batch entry in the batch request is too long: <the 81-character id>` for an over-long Id and `The Id of a batch entry in the batch request contains an impermissible character: <the id>` for one containing a period. That derivation (which rule, which Id) is the requirement.
Emulator: returns the right code `InvalidBatchEntryId` but the message `The Id of a batch entry in a batch request doesn't abide by the specification.` — which is verbatim the Smithy doc text of `InvalidBatchEntryIdException`. It names neither the rule that was broken nor the Id that broke it, and is identical for both cases.
Why: a batch publisher cannot tell which entry of a batch is malformed or why, so it cannot repair and retry the batch; the two different violations are indistinguishable.
Fix: the PublishBatch handler's per-entry Id validation / its error construction — derive the message from the failing entry rather than defaulting to the error shape's documentation.
Traces: sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected.

Cause 4 of 4: a FIFO publish carrying 128-character MessageGroupId and MessageDeduplicationId is correctly accepted, but never reaches the subscribed FIFO SQS queue.
AWS: Smithy `PublishInput$MessageGroupId` documentation — "The length of MessageGroupId is 128 characters" (same 128 limit documented for `MessageDeduplicationId`). So 129 characters is refused (`InvalidParameterException`, 400 / `InvalidParameter`) and exactly 128 is accepted, sequenced, and delivered to the subscribed FIFO queue carrying the full untruncated 128-character group id.
Emulator: the two refusals at 129 characters are produced correctly (the check gets past them), and the boundary publish is accepted — but the subscribed FIFO queue never receives the message, so the check runs out the 60 s cap (`Error: Check timed out after 60.0s`) inside its receive loop.
Why: the FIFO boundary case cannot be verified end to end, and an ordered FIFO consumer observes nothing.
Fix: the SNS fan-out/delivery path invoked at the end of the Publish handler (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs), which records delivery into SNS's own state rather than handing the message to the subscribed queue.
Traces: sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected.
Note: this is the same missing fan-out path described as Cause 1 of 20261025T025317Z-sns-performs-no-out-of-band-delivery-a.md. That issue does NOT carry this check's replay key, so the check is filed here; a fixer closing the fan-out path there should re-replay this check too and may close this cause with it.

## Evidence
Replay output, emulator vs AWS:

Cause 1 — sns.subject-with-a-line-break-or-control-character-rejected:
  Expected (AWS): Publish(Subject="first\nsecond") -> HTTP 400, Code InvalidParameter (message e.g. "Invalid parameter: Subject"); likewise for "first\rsecond", "first\x01second", "first\x7fsecond", "" and "   ".
  Actual (emulator): HTTP 200 — AssertionError: expected the call to be refused, but it succeeded: {'MessageId': 'c661eaa6-d7d0-409b-9d25-e583c9a1117f', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', ...}}}

Cause 2 — sns.subscribe-with-an-unknown-attribute-rejected-and-creates-nothing:
  Expected (AWS): ('InvalidParameter', 'Invalid parameter: Attributes Reason: Unknown attribute RawMessgeDelivery')
  Actual (emulator): AssertionError: Subscribe with an unknown attribute name: got 'InvalidParameter' 'Invalid parameter: AttributeName'

Cause 3 — sns.publish-batch-entry-id-over-80-characters-or-with-invalid-characters-rejected:
  Expected (AWS): ('InvalidBatchEntryId', 'The Id of a batch entry in the batch request is too long: aaaa…' [81 a's])
  Actual (emulator): AssertionError: an 81-character entry Id gave 'InvalidBatchEntryId': "The Id of a batch entry in a batch request doesn't abide by the specification." (the Smithy doc string of InvalidBatchEntryIdException)

Cause 4 — sns.fifo-message-group-and-deduplication-ids-over-128-characters-rejected:
  Expected (AWS): the 129-character refusals, then Publish with 128-character ids returns a SequenceNumber and the FIFO queue receives exactly that one message with Attributes.MessageGroupId equal to the full 128-character id.
  Actual (emulator): both refusals produced, the boundary publish accepted, then the receive loop never sees the message — Error: Check timed out after 60.0s.

## Steps to Reproduce
Against the emulator's SNS endpoint (us-west-2, any account):

Cause 1:
1. CreateTopic(Name="t").
2. Publish(TopicArn, Message="x", Subject="first\nsecond") -> 200 + MessageId. AWS answers 400 InvalidParameter. Repeat with Subject="first\rsecond", "first\x01second", "first\x7fsecond", "", "   " — all 200.
3. Publish with Subject="col\tumn" and Subject="  padded  " must stay accepted and be delivered untrimmed.

Cause 2:
1. CreateTopic; CreateQueue and read its QueueArn.
2. Subscribe(TopicArn, Protocol="sqs", Endpoint=<queue arn>, ReturnSubscriptionArn=true, Attributes={"FilterPolicy": "{\"tier\":[\"gold\"]}", "RawMessgeDelivery": "true"}).
3. Observe Error.Message == "Invalid parameter: AttributeName"; AWS answers "Invalid parameter: Attributes Reason: Unknown attribute RawMessgeDelivery".

Cause 3:
1. CreateTopic.
2. PublishBatch(TopicArn, PublishBatchRequestEntries=[{"Id":"valid-sibling","Message":"a"},{"Id":"a"*81,"Message":"b"}]).
3. Observe Code InvalidBatchEntryId with the generic doc message; AWS's message names the rule and the offending Id. Repeat with Id="has.period" — AWS reports the impermissible-character rule, the emulator returns the same generic text.

Cause 4:
1. CreateTopic(Name="t.fifo", Attributes={"FifoTopic":"true"}); CreateQueue(Name="q.fifo", Attributes={"FifoQueue":"true"}); set a queue Policy allowing sns.amazonaws.com sqs:SendMessage for that topic ARN.
2. Subscribe(Protocol="sqs", Endpoint=<queue arn>, Attributes={"RawMessageDelivery":"true"}).
3. Publish with a 129-character MessageGroupId -> correctly refused InvalidParameter; same for MessageDeduplicationId.
4. Publish with both ids exactly 128 characters -> accepted with a SequenceNumber.
5. ReceiveMessage(AttributeNames=["MessageGroupId"]) in a loop: nothing ever arrives.
