Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.platform-application-event-endpoint-created-notifies-configured-topic sns__sqs.queue-maximum-message-size-does-not-bound-sns-delivery sns__sqs.sns-to-sqs-envelope-carries-the-subject-and-typed-message-attributes
Issue-Key: sns__sqs.platform-application-event-endpoint-created-notifies-configured-topic|run#25|AssertionError|raise AssertionError(
Issue-Key: sns__sqs.queue-maximum-message-size-does-not-bound-sns-delivery|run#28|AssertionError|raise AssertionError(
Issue-Key: sns__sqs.sns-to-sqs-envelope-carries-the-subject-and-typed-message-attributes|run#20|AssertionError|assert got == want, f"envelope fields {got!r}, expected {want!r}"
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 3 net-new causes: no platform-application event notifications, SNS delivery bounded by the queue's MaximumMessageSize, and the notification envelope omits SignatureVersion

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Three distinct root causes in the sns__sqs suite. Each is listed with its own failing check ids; all three must be closed.

Cause 1 of 3 — a platform application's EventEndpointCreated notification is never published. AWS: a platform application with EventEndpointCreated set to a topic publishes a notification naming the new endpoint ARN (arn:aws:sns:<region>:<account>:endpoint/<platform>/<app name>/<endpoint id>) whenever CreatePlatformEndpoint succeeds. Emulator: 'no EventEndpointCreated notification naming <endpoint arn> arrived within 90s'. Why: a client that provisions endpoints and reacts to the lifecycle event (registration bookkeeping, token refresh) gets nothing, and cannot tell a missing subscription from a service that never publishes. Fix: platform-application event notifications in the SNS endpoint lifecycle. Traces: sns__sqs.platform-application-event-endpoint-created-notifies-configured-topic.

Cause 2 of 3 — SNS->SQS delivery is bounded by the destination queue's MaximumMessageSize. AWS: a queue's MaximumMessageSize does NOT bound what SNS delivers into it; a 4096-byte body published to the topic arrives at the subscribed queue even when the queue's MaximumMessageSize is set below the envelope size. Emulator: 'the SNS-delivered message carrying the 4096-byte body never arrived within 90s' — the delivery is dropped silently. Why: messages vanish between SNS and SQS with no error anywhere, which is the hardest class of bug to diagnose in production. Fix: the SNS->SQS delivery path (do not apply the queue's MaximumMessageSize to SNS deliveries). Traces: sns__sqs.queue-maximum-message-size-does-not-bound-sns-delivery.

Cause 3 of 3 — the delivered notification envelope omits SignatureVersion. AWS: the JSON notification SNS delivers to an SQS subscriber carries Type, MessageId, TopicArn, Subject (when set), Message, Timestamp, SignatureVersion ('1'), Signature, SigningCertURL and UnsubscribeURL. Emulator: SignatureVersion is absent (None) while every other field round-trips, including the unicode Subject and Message. Why: consumers that verify or branch on the envelope's signature fields cannot do so; a verifying client treats every notification as malformed. Fix: the SNS notification envelope serializer. Related open issue filed for sibling checks: 20261029T183635Z-sns-sqs-the-delivered-notification-envelope-omits-signatureversion.md. Traces: sns__sqs.sns-to-sqs-envelope-carries-the-subject-and-typed-message-attributes.

## Evidence
Cause 1: AssertionError: no EventEndpointCreated notification naming 'arn:aws:sns:us-west-2:000000000000:endpoint/Baidu/sim2real-behav-sns-appevt-app-.../58958eb2-...' arrived within 90s
Cause 2: AssertionError: the SNS-delivered message carrying the 4096-byte body never arrived within 90s
Cause 3: AssertionError: envelope fields {'Type': 'Notification', 'MessageId': '...', 'TopicArn': '...', 'Subject': 'Order shipped: #42 (ünïcode)', 'Message': '{"marker": ..., "text": "Grüße 中文 📦"}', 'SignatureVersion': None}, expected the same with 'SignatureVersion': '1'

## Steps to Reproduce
Cause 1: CreatePlatformApplication with EventEndpointCreated=<topic arn> (SQS subscriber on the topic), then CreatePlatformEndpoint; poll the queue.
Cause 2: SetQueueAttributes MaximumMessageSize below the envelope size, subscribe the queue to a topic, Publish a 4096-byte body, poll the queue.
Cause 3: Publish with a Subject and typed MessageAttributes to a topic with a (non-raw) SQS subscription; read the envelope keys.
