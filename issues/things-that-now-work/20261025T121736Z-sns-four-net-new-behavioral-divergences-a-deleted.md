Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.deleted-endpoint-queue-dead-letters-each-subscription-in-its-own-format sns.fifo-topic-attributes-roundtrip sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn sns.subscribe-invalid-filter-policy-fails
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.deleted-endpoint-queue-dead-letters-each-subscription-in-its-own-format|_collect_keyed#19|Timeout|resp = sqs.receive_message(
Issue-Key: sns.fifo-topic-attributes-roundtrip|run#6|AssertionError|assert attrs.get("ContentBasedDeduplication") == "false", (
Issue-Key: sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn|run#14|AssertionError|assert message == ("Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn "
Issue-Key: sns.subscribe-invalid-filter-policy-fails|run#11|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Commit: a97363f812621dd47aa0891b39cb5eeebafd4acb

# SNS: four net-new behavioral divergences — a deleted-endpoint notification is never dead-lettered, a FIFO topic's ContentBasedDeduplication default/round-trip is missing, and two refusal messages diverge from AWS's text (4 causes)

> _Note: this issue was identified against the code base at commit `a97363f812621dd47aa0891b39cb5eeebafd4acb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Four distinct root causes in the emulator's SNS implementation. Each is grounded in one failing behavioral check from this replay that is NOT marked ALREADY FILED and is NOT carried by any open issue's replay keys. They are bundled because they all belong to the `sns` service; a fixer must close all four separately before submitting.

Cause 1 of 4: a notification that cannot be delivered because the subscribed queue was deleted is never dead-lettered, so the per-subscription DLQ format cannot be produced at all.
AWS: Smithy `SetSubscriptionAttributesInput$AttributeName` documents `RedrivePolicy` — "When specified, sends undeliverable messages to the specified Amazon SQS dead-letter queue. Messages that can't be delivered due to client errors (for example, when the subscribed endpoint is unreachable) ... are held in the dead-letter queue for further analysis or reprocessing." A deleted subscribed queue is exactly such a client error. Beyond that the model is silent, so the rest is recorded behavior only: the DLQ copy is written in the FAILING SUBSCRIPTION'S OWN delivery format — an envelope subscription's copy is the full `{"Type":"Notification", ...}` body with the publish's `Subject` and its `MessageAttributes` as `{Type, Value}` pairs verbatim, and an `UnsubscribeURL` whose `SubscriptionArn` query parameter is that subscription's ARN (not its raw sibling's); a `RawMessageDelivery=true` subscription's copy is the bare message body with the attributes as native SQS message attributes (a Number attribute published as `+5` normalising to `DataType=Number`/`StringValue=5`). Each subscription writes exactly one copy, and a publish that WAS delivered successfully is never copied to the DLQ.
Emulator: nothing is dead-lettered and, before that, nothing is delivered — the check does not complete and is killed at the harness cap (`Error: Check timed out after 60.0s`). Its first blocking wait is the control leg (a healthy publish reaching the still-existing envelope and raw queues), so the run never even reaches the deleted-endpoint legs. Hypothesis: the delivery path records deliveries into SNS's own state instead of handing the message to the subscribed queue, so there is no delivery attempt to fail and therefore no redrive.
Why: a redrive consumer sees nothing. A client that configures a subscription RedrivePolicy gets a 200 from SetSubscriptionAttributes and a RedrivePolicy that reads back correctly, but undeliverable notifications are silently dropped rather than parked in the DLQ — the exact failure mode redrive exists to make visible — and the two consumers (envelope parser vs raw parser) can never be exercised.
Fix: the SNS delivery path invoked by the Publish handler (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs fan-out step) and its retry/redrive stage, which must render the DLQ copy per subscription using that subscription's RawMessageDelivery setting and its own UnsubscribeURL.
Traces: sns.deleted-endpoint-queue-dead-letters-each-subscription-in-its-own-format.
Note: this is the same missing fan-out/redrive path described as Cause 1 and Cause 4 of 20261025T025317Z-sns-performs-no-out-of-band-delivery-a.md, but that issue does NOT carry this check's replay key, so the check is filed here; a fixer who restores the fan-out and redrive stages there should re-replay this check, which additionally grades the per-subscription DLQ FORMAT (envelope vs raw, and the UnsubscribeURL naming the failing subscription) that no filed check covers.

Cause 2 of 4: a FIFO topic does not report `ContentBasedDeduplication`, so neither its default nor its SetTopicAttributes round-trip can be read back.
AWS: Smithy `GetTopicAttributesResponse$Attributes` documentation lists `ContentBasedDeduplication` among the attributes that apply to FIFO topics and states "By default, ContentBasedDeduplication is set to false. If you create a FIFO topic and this attribute is false, you must specify a value for the MessageDeduplicationId parameter for the Publish action." So GetTopicAttributes on a topic created with `FifoTopic=true` returns `FifoTopic="true"` AND `ContentBasedDeduplication="false"`, and `SetTopicAttributes(AttributeName="ContentBasedDeduplication", AttributeValue="true"|"false")` flips that read-back independently of CreateTopic.
Emulator: the key is absent entirely — `AssertionError: expected ContentBasedDeduplication default false, got None` on the read immediately after CreateTopic(Attributes={"FifoTopic":"true"}).
Why: a client reading the attribute to decide whether it must supply its own MessageDeduplicationId gets a missing key instead of the documented default `false`, and cannot confirm that a SetTopicAttributes toggle took effect.
Fix: the FIFO topic default attributes in SNS state and the GetTopicAttributes attribute projection (plus the SetTopicAttributes handler's acceptance of the attribute for FIFO topics).
Traces: sns.fifo-topic-attributes-roundtrip.
Note: the same omission is Cause 4 of 20261025T025317Z-sns-fifo-deduplication-does-not-deduplicate-publishbatch-validates.md, which does NOT carry this check's replay key. This check additionally grades the true/false round-trip through SetTopicAttributes, so re-replay it after fixing that cause.

Cause 3 of 4: Publish's refusal for naming both TopicArn and TargetArn uses the wrong message text.
AWS: Smithy `PublishInput$TargetArn` — "If you don't specify a value for the TargetArn parameter, you must specify a value for the PhoneNumber or TopicArn parameters"; the targets are mutually exclusive. Publish models `InvalidParameterException` (httpError 400, awsQueryError code `InvalidParameter`). The recorded refusal is HTTP 400, Code `InvalidParameter`, Message `Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn specified. Use only one or the other.` — every word of that message is fixed text for this error class (the member name `TopicArn` after `Invalid parameter:` is the member AWS attributes the fault to); it carries no value taken from the request or from state.
Emulator: the status (400) and the code (`InvalidParameter`) are right, but the message is `Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn cannot be specified at the same time.` — the `Reason:` clause is reworded.
Why: clients and SDK-level tooling that classify or surface SNS refusals by message text (the code `InvalidParameter` is shared by dozens of unrelated SNS validation failures, so the message is the only discriminator) mis-route or fail to recognise this one.
Fix: the Publish handler's input validation where the TopicArn/TargetArn/PhoneNumber exclusivity is checked — emit AWS's fixed text for this class.
Traces: sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn.

Cause 4 of 4: Subscribe's refusal for an unparseable FilterPolicy truncates the message, dropping the parser diagnostic AWS returns.
AWS: Subscribe takes `Attributes` (`SubscriptionAttributesMap`, whose `FilterPolicy` entry is "the simple JSON object that lets your subscriber receive only a subset of messages") and models `InvalidParameterException` (httpError 400, awsQueryError code `InvalidParameter`). The recorded refusal is HTTP 400, Code `InvalidParameter`, Message `Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]`. Fixed text for this class: `Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]`. Variable parts taken from the request's FilterPolicy text: the offending token (here `this`, from the submitted value `this-is-not-json`) and the line/column at which parsing failed (here line 1, column 1).
Emulator: the status (400), the code (`InvalidParameter`) and the leading clause are right, but the message stops at `Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON.` — the request-derived parser diagnostic (offending token and line/column) is never appended.
Why: the caller is told only that its filter policy is unparseable, not where or on what token, so a malformed policy built by templating or string concatenation cannot be located; and a client asserting on AWS's message sees a mismatch.
Fix: the Subscribe handler's Attributes/FilterPolicy validation — carry the JSON parser's token and line/column into the InvalidParameter message (the same text applies to SetSubscriptionAttributes(FilterPolicy)).
Traces: sns.subscribe-invalid-filter-policy-fails.
Note: this is not covered by 20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md — here the call IS refused with the correct code and status; only the message text diverges.

## Evidence
Replay output of this suite run (emulator vs AWS), one block per cause:

Cause 1 — sns.deleted-endpoint-queue-dead-letters-each-subscription-in-its-own-format (verdict: mismatch):
  Expected (AWS): the healthy publish reaches both the envelope and the raw queue and is NOT copied to the DLQ; after both queues are deleted, a publish with Subject="dead-letter" and MessageAttributes={"n":{"DataType":"Number","StringValue":"+5"}} puts exactly one copy per subscription into the shared DLQ — the envelope copy with Type="Notification", the topic's ARN, Subject="dead-letter", MessageAttributes={"n":{"Type":"Number","Value":"+5"}} and an UnsubscribeURL whose SubscriptionArn is the ENVELOPE subscription's ARN; the raw copy as the bare body with SQS message attribute n = (Number, "5").
  Actual (emulator): Error: Check timed out after 60.0s — the run never gets past its first wait, the control leg "the enveloped endpoint receives while it exists" (a _receive_keyed with timeout_s=60.0). No notification and no DLQ copy is ever observed.

Cause 2 — sns.fifo-topic-attributes-roundtrip (verdict: mismatch):
  Expected (AWS): GetTopicAttributes on a topic created with Name="...fifo", Attributes={"FifoTopic":"true"} returns FifoTopic="true" and ContentBasedDeduplication="false".
  Actual (emulator): AssertionError: expected ContentBasedDeduplication default false, got None (check_sns_fifo_topic_attributes_roundtrip.py line 34) — FifoTopic is correct, ContentBasedDeduplication is absent from the Attributes map.

Cause 3 — sns.publish-by-target-arn-naming-a-topic-delivers-like-topic-arn (verdict: mismatch):
  Expected (AWS): Publish(TopicArn=<topic>, TargetArn=<same topic>, Message=...) -> HTTP 400, Code 'InvalidParameter', Message 'Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn specified. Use only one or the other.'
  Actual (emulator): Code 'InvalidParameter' (correct) with Message 'Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn cannot be specified at the same time.' — AssertionError: unexpected message for a publish naming both targets.

Cause 4 — sns.subscribe-invalid-filter-policy-fails (verdict: mismatch):
  Expected (AWS): Subscribe(..., Attributes={"FilterPolicy": "this-is-not-json"}) -> HTTP 400, Code 'InvalidParameter', Message "Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]"
  Actual (emulator): HTTP 400 and Code 'InvalidParameter' (both correct), Message 'Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON.' — AssertionError at check_sns_subscribe_invalid_filter_policy_fails.py line 109. The positive control (a well-formed FilterPolicy at Subscribe) and the effect-absent leg (ListSubscriptionsByTopic shows only the control subscription) both passed, so the divergence is isolated to the message text.

## Steps to Reproduce
Against the emulator's SNS endpoint (any region/account; an SQS endpoint is needed for cause 1):

Cause 1:
1. CreateTopic(Name="t").
2. Create three SQS queues (dlq, env, raw) and set each one's Policy to allow sns.amazonaws.com sqs:SendMessage with Condition ArnEquals aws:SourceArn = the topic ARN.
3. Subscribe(Protocol="sqs", Endpoint=<env arn>, ReturnSubscriptionArn=true) and Subscribe(Protocol="sqs", Endpoint=<raw arn>, Attributes={"RawMessageDelivery":"true"}, ReturnSubscriptionArn=true); poll GetSubscriptionAttributes until PendingConfirmation="false" on both.
4. SetSubscriptionAttributes(AttributeName="RedrivePolicy", AttributeValue='{"deadLetterTargetArn":"<dlq arn>"}') on both; confirm it reads back.
5. Publish(TopicArn, Message="healthy") and ReceiveMessage on env and raw in a loop: nothing ever arrives (this is where the check dies at 60 s).
6. DeleteQueue(env), DeleteQueue(raw); Publish(TopicArn, Message="dead", Subject="dead-letter", MessageAttributes={"n":{"DataType":"Number","StringValue":"+5"}}); ReceiveMessage on the DLQ with MessageAttributeNames=["All"] in a loop: no copy ever arrives. AWS puts one copy per subscription, each in that subscription's own format.

Cause 2:
1. CreateTopic(Name="anything.fifo", Attributes={"FifoTopic":"true"}).
2. GetTopicAttributes(TopicArn) -> FifoTopic="true" but no ContentBasedDeduplication key (AWS returns "false").
3. SetTopicAttributes(AttributeName="ContentBasedDeduplication", AttributeValue="true"), then GetTopicAttributes -> must read "true"; set it back to "false" and it must read "false".

Cause 3:
1. CreateTopic(Name="t").
2. Publish(TargetArn=<topic arn>, Message="x") -> must succeed and return a MessageId.
3. Publish(TopicArn=<topic arn>, TargetArn=<same topic arn>, Message="x") -> observe Code InvalidParameter with Message 'Invalid parameter: TopicArn Reason: Both TopicArn and TargetArn cannot be specified at the same time.'; AWS answers '... Reason: Both TopicArn and TargetArn specified. Use only one or the other.'

Cause 4:
1. CreateTopic(Name="t"); CreateQueue and read its QueueArn.
2. Subscribe(TopicArn, Protocol="sqs", Endpoint=<queue arn>, Attributes={"FilterPolicy": "{\"event\":[\"created\"]}"}, ReturnSubscriptionArn=true) -> succeeds (control).
3. Subscribe(TopicArn, Protocol="sqs", Endpoint=<queue arn>, Attributes={"FilterPolicy": "this-is-not-json"}, ReturnSubscriptionArn=true) -> HTTP 400 InvalidParameter with the truncated message; AWS appends "Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]".
