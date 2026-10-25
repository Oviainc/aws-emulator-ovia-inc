Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns.snstrig-raw-message-delivery-rejected-on-a-lambda-subscription
Issue-Key: lambda__sns.snstrig-raw-message-delivery-rejected-on-a-lambda-subscription|run#8|AssertionError|assert err == want, f"RawMessageDelivery=true on a lambda subscription: {err!r}, expected {want!r}"
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# lambda__sns: the lambda raw-message-delivery refusal message does not match AWS's text (1 cause)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetSubscriptionAttributes

## Description
Cause 1 of 1: the InvalidParameter message for RawMessageDelivery on a lambda subscription differs from AWS's fixed text.
AWS: setting RawMessageDelivery=true on a `lambda` subscription is refused with HTTP 400 InvalidParameterException (awsQueryError code InvalidParameter) and the fixed message 'Invalid parameter: Delivery protocol [lambda] does not support raw message delivery.' — the text is a requirement here because AWS emits exactly this sentence for this condition (it is what the check recorded), not a free-form diagnostic.
Emulator: the status (400) and code (InvalidParameter) are correct, but the message is 'Invalid parameter: Attributes Reason: Delivery protocol [lambda] does not support raw message delivery' — wrapped in the emulator's generic 'Attributes Reason:' frame and missing the trailing period.
Why: a client or test that matches AWS's documented message for this refusal (a common way to distinguish it from other InvalidParameter causes) does not match.
Fix: the message construction in the SNS subscription-attribute validation for the per-protocol raw-delivery rule.
Traces: lambda__sns.snstrig-raw-message-delivery-rejected-on-a-lambda-subscription.
(The other five failures in this group all abort on SNS ListSubscriptions answering NotImplemented, which the replay reports as already filed.)

## Evidence
Expected (AWS): ('InvalidParameter', 'Invalid parameter: Delivery protocol [lambda] does not support raw message delivery.')
Actual (emulator): ('InvalidParameter', 'Invalid parameter: Attributes Reason: Delivery protocol [lambda] does not support raw message delivery')

## Steps to Reproduce
1. CreateFunction; CreateTopic; Subscribe(Protocol="lambda", Endpoint=<function arn>).
2. SetSubscriptionAttributes(SubscriptionArn, AttributeName="RawMessageDelivery", AttributeValue="true").
3. Compare Error.Message with AWS's text above; status 400 and code InvalidParameter already agree.
