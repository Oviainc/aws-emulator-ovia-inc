Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__sns__sqs.https-delivery-content-type-defaults-to-text-plain-and-follows-headercontenttype iam__lambda__sns__sqs.unsubscribe-confirmation-posts-only-for-the-anonymous-flow-and-its-token-reconfirms
Issue-Key: iam__lambda__sns__sqs.https-delivery-content-type-defaults-to-text-plain-and-follows-headercontenttype|_captured#5|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: iam__lambda__sns__sqs.unsubscribe-confirmation-posts-only-for-the-anonymous-flow-and-its-token-reconfirms|_captured#5|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# SNS never delivers to an HTTPS subscription endpoint: neither notifications nor the unsubscribe confirmation POST ever arrive

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: SNS's HTTPS delivery path does not POST to a subscribed https endpoint, so no notification and no unsubscribe-confirmation request is ever made.

AWS: for a confirmed https subscription, Publish POSTs the message to the endpoint with the x-amz-sns-* headers (Message-Type, Message-Id, Topic-Arn, Subscription-Arn) and a Content-Type that defaults to `text/plain; charset=UTF-8` and follows the subscription's HeaderContentType delivery attribute when set. Separately, an Unsubscribe performed through the anonymous (token) flow — and only that flow — causes SNS to POST an `UnsubscribeConfirmation` message to the endpoint, whose Token can be replayed to ConfirmSubscription to re-confirm the subscription. Smithy: sns#Publish and sns#Unsubscribe/ConfirmSubscription model the control calls; the HTTP delivery headers and the confirmation POST are service behaviour — recorded behavior only, and the recordings show the endpoint receiving each request.

Emulator: both checks hang and are killed at their 180 s cap — the endpoint receives nothing, so neither the default/overridden Content-Type nor the unsubscribe-confirmation token can be observed.

Why: a client with an HTTPS subscriber receives no messages at all while Publish reports success: silent, total data loss on a first-class SNS delivery protocol. The unsubscribe-confirmation path, which is how an accidental anonymous unsubscribe is reversed, is likewise unavailable.

Fix: the SNS HTTPS/HTTP delivery worker — it must POST notifications (with the signed x-amz-sns-* headers and the Content-Type rules) and emit UnsubscribeConfirmation for the anonymous unsubscribe flow.

Traces: iam__lambda__sns__sqs.https-delivery-content-type-defaults-to-text-plain-and-follows-headercontenttype, iam__lambda__sns__sqs.unsubscribe-confirmation-posts-only-for-the-anonymous-flow-and-its-token-reconfirms.

## Evidence
Expected: the subscribed https endpoint receives the published notification (Content-Type 'text/plain; charset=UTF-8' by default, HeaderContentType when configured) and an UnsubscribeConfirmation POST after an anonymous Unsubscribe.
Actual: Error: Check timed out after 180.0s (both checks) — no request reached the endpoint.

## Steps to Reproduce
1. Stand up an https endpoint that records the requests it receives (e.g. a Lambda function URL) and Subscribe it to a topic; confirm the subscription.
2. Publish to the topic and inspect the endpoint's captured requests.
3. Unsubscribe through the anonymous token flow and look for an UnsubscribeConfirmation POST.
4. Observe that no request ever arrives.
