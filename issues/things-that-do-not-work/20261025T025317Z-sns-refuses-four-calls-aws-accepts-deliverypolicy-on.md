Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.subscription-delivery-policy-reads-back-normalized-on-an-sqs-subscription sns.raw-message-delivery-flag-is-case-insensitive-and-reads-back-lowercase sns.firehose-raw-subscription-writes-the-bare-message-bodies sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|SetSubscriptionAttributes|InvalidParameter
Issue-Key: sns|Subscribe|InvalidParameter
Issue-Key: sns|Publish|InvalidParameter
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS refuses four calls AWS accepts: DeliveryPolicy on an sqs subscription, RawMessageDelivery case, firehose raw delivery, and MessageStructure=json object branches (4 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetSubscriptionAttributes

## Description
Four over-strict validations turn a legitimate request into a 400 InvalidParameterException (code InvalidParameter), blocking the checks outright.

Cause 1 of 4: SetSubscriptionAttributes(DeliveryPolicy) on an sqs subscription is refused.
AWS: SetSubscriptionAttributes accepts a DeliveryPolicy on an SQS subscription and reads it back normalized (the delivery policy's healthy-retry fields filled in). Only the modeled errors apply; there is no "protocol does not support a delivery policy" rule for sqs.
Emulator: 400 InvalidParameter "Invalid parameter: Attributes Reason: Delivery protocol [sqs] does not support a delivery policy".
Why: a client cannot set or inspect retry behaviour for queue subscriptions; the check cannot even reach its assertions.
Fix: the per-protocol attribute table in the SetSubscriptionAttributes/Subscribe validation.
Traces: sns.subscription-delivery-policy-reads-back-normalized-on-an-sqs-subscription.

Cause 2 of 4: RawMessageDelivery is matched case-sensitively.
AWS: the flag is case-insensitive on input ("True"/"TRUE" accepted) and reads back lowercase "true".
Emulator: 400 InvalidParameter "Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false".
Why: a client (or an IaC tool) sending "True" is refused where AWS succeeds.
Fix: boolean attribute parsing in the Subscribe/SetSubscriptionAttributes validation.
Traces: sns.raw-message-delivery-flag-is-case-insensitive-and-reads-back-lowercase.

Cause 3 of 4: raw message delivery is rejected for the firehose protocol.
AWS: a firehose subscription supports RawMessageDelivery=true, writing the bare message bodies to the stream.
Emulator: Subscribe answers 400 InvalidParameter "Invalid parameter: Attributes Reason: Delivery protocol [firehose] does not support raw message delivery".
Why: the documented raw-delivery firehose topology cannot be created at all.
Fix: the same per-protocol attribute table (allow RawMessageDelivery for firehose).
Traces: sns.firehose-raw-subscription-writes-the-bare-message-bodies.

Cause 4 of 4: Publish with MessageStructure=json refuses non-string top-level values.
AWS: Smithy Publish$MessageStructure — the Message must be a valid JSON object containing at least a top-level "default" string key; other top-level keys select per-protocol bodies. Recorded behavior: scalar branches are stringified and object-valued branches are ignored, not refused.
Emulator: 400 InvalidParameter "Invalid parameter: Message - every top-level value of the message must be a string".
Why: a publish AWS accepts is refused, so protocol-specific payload shaping cannot be used.
Fix: the MessageStructure=json validation in the Publish handler's message rules.
Traces: sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches.

## Evidence
Expected (AWS): HTTP 200 for all four calls.
Actual (emulator):
- sns.subscription-delivery-policy-reads-back-normalized-on-an-sqs-subscription: botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameter) when calling the SetSubscriptionAttributes operation: Invalid parameter: Attributes Reason: Delivery protocol [sqs] does not support a delivery policy
- sns.raw-message-delivery-flag-is-case-insensitive-and-reads-back-lowercase: (InvalidParameter) when calling the Subscribe operation: Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false
- sns.firehose-raw-subscription-writes-the-bare-message-bodies: (InvalidParameter) when calling the Subscribe operation: Invalid parameter: Attributes Reason: Delivery protocol [firehose] does not support raw message delivery
- sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches: (InvalidParameter) when calling the Publish operation: Invalid parameter: Message - every top-level value of the message must be a string

## Steps to Reproduce
1. Subscribe(Protocol="sqs"); SetSubscriptionAttributes(AttributeName="DeliveryPolicy", AttributeValue='{"healthyRetryPolicy":{"numRetries":3}}') -> 400 InvalidParameter.
2. Subscribe(Protocol="sqs", Attributes={"RawMessageDelivery":"True"}) -> 400 InvalidParameter.
3. Subscribe(Protocol="firehose", SubscriptionRoleArn=..., Attributes={"RawMessageDelivery":"true"}) -> 400 InvalidParameter.
4. Publish(MessageStructure="json", Message='{"default":"d","n":1,"o":{"k":"v"}}') -> 400 InvalidParameter.
