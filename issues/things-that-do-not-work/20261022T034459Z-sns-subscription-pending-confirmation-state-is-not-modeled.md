Issue-Source: trace-sweep-divergence
Issue-Trace: sns-subscription-pending-and-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-subscription-pending-and-error-paths sns-subscribe-lambda-protocol-auto-confirms
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|ListSubscriptionsByTopic|2xx->2xx|$.ListSubscriptionsByTopicResponse.ListSubscriptionsByTopicResult.Subscriptions.member.SubscriptionArn
Issue-Key: sns|Subscribe|4xx->2xx|$.ErrorResponse
Issue-Key: sns|Subscribe|4xx->2xx|status
Issue-Key: sns|Unsubscribe|4xx->2xx|$.ErrorResponse
Issue-Key: sns|Unsubscribe|4xx->2xx|status
Issue-Key: sns|Unsubscribe|2xx->4xx|$.UnsubscribeResponse
Issue-Key: sns|Unsubscribe|2xx->4xx|status
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# SNS subscription pending-confirmation state is not modeled: endpoint validation, listed ARN and Unsubscribe gating

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptionsByTopic

## Description
The emulator treats every subscription as immediately confirmed and accepts any endpoint value, so the whole pending-confirmation half of the SNS subscription model is missing. This issue bundles 4 causes in the subscription lifecycle.

Cause 1 of 4 — a pending subscription is listed with a real ARN instead of the PendingConfirmation sentinel.
AWS: Smithy ListSubscriptionsByTopic returns SubscriptionsList of Subscription; the SubscribeInput.ReturnSubscriptionArn documentation states that for subscriptions that are not yet confirmed the response carries the "pending subscription" ARN value rather than the real ARN, and "a subscription becomes confirmed when the subscriber calls ConfirmSubscription". Recorded ListSubscriptionsByTopic for an unconfirmed http/email subscription returns SubscriptionArn = 'PendingConfirmation' with status 200.
Emulator: status 200 but SubscriptionArn is a fully-formed subscription ARN (arn:aws:sns:<region>:<account>:<topic>:<uuid>), i.e. the subscription is reported as confirmed.
Why: a client that polls ListSubscriptionsByTopic for the sentinel to decide whether a subscriber still has to confirm never sees a pending subscription, so confirmation flows (and tests of them) report success without any confirmation having happened; it also hands out an ARN AWS would not yet publish.
Fix: subscription state must carry a pending/confirmed flag, and the ListSubscriptionsByTopic (and Subscribe) serializer must project 'PendingConfirmation' while unconfirmed.
Traces: sns-subscription-pending-and-error-paths#task#4.

Cause 2 of 4 — Subscribe accepts an endpoint that is not a valid Lambda function ARN.
AWS: Smithy Subscribe takes Protocol (required) and Endpoint and declares InvalidParameterException (awsQueryError code 'InvalidParameter', httpResponseCode 400). For Protocol='lambda' the endpoint must be a Lambda function ARN; the recorded response for a non-ARN endpoint is 400 with Error.Type 'Sender', Error.Code 'InvalidParameter' (message example: "Invalid parameter: Lambda endpoint ARN").
Emulator: 200 with no error body at all — the subscription is created.
Why: the request that AWS refuses succeeds, so a client wires up a subscription to an endpoint that cannot exist and discovers it only when deliveries silently go nowhere; negative-path tests of endpoint validation pass against the emulator and fail against AWS.
Fix: per-protocol endpoint validation in the Subscribe handler, raising InvalidParameterException before state is mutated.
Traces: sns-subscribe-lambda-protocol-auto-confirms#task#5.

Cause 3 of 4 — Unsubscribe on a pending subscription succeeds instead of being refused.
AWS: Smithy Unsubscribe declares InvalidParameterException ('InvalidParameter', 400). A subscription still awaiting confirmation cannot be unsubscribed via the API; the recorded response is 400 with Error.Type 'Sender', Error.Code 'InvalidParameter' (message example: "Invalid parameter: SubscriptionArn Reason: Cannot unsubscribe a subscription that is pending confirmation").
Emulator: 200 with no ErrorResponse — the pending subscription is deleted.
Why: callers can delete state AWS protects, so cleanup paths behave differently from production, and a client that relies on the 400 to detect "still pending" gets no signal.
Fix: the Unsubscribe handler must gate on the subscription's pending/confirmed state (the state introduced for cause 1).
Traces: sns-subscription-pending-and-error-paths#task#6.

Cause 4 of 4 — a later Unsubscribe of a live subscription answers 404 instead of 200.
AWS: Smithy Unsubscribe output is Unit; a successful unsubscribe of an existing subscription is 200 with an UnsubscribeResponse/ResponseMetadata body. Recorded response: 200.
Emulator: 404 and no UnsubscribeResponse body.
Why: a client deleting a subscription it holds a valid ARN for is told the subscription does not exist, so teardown reports failure and retry/cleanup logic diverges. Hypothesis: this is downstream of cause 3 — the earlier Unsubscribe of the pending subscription (step task#6) wrongly removed the record, so by this step the subscription is already gone; it must be re-replayed after cause 3 is fixed rather than assumed resolved.
Fix: the Unsubscribe handler and subscription state (same component as cause 3).
Traces: sns-subscription-pending-and-error-paths#task#9.

## Evidence
Cause 1 (sns-subscription-pending-and-error-paths#task#4, expected 200 / actual 200): $.ListSubscriptionsByTopicResponse.ListSubscriptionsByTopicResult.Subscriptions.member.SubscriptionArn expected 'PendingConfirmation', got 'arn:aws:sns:us-west-2:000000000000:sim2real-probe-sns-topic-98518abb:8de1df3a-1804-43fb-8292-9743ed80d2da'.
Cause 2 (sns-subscribe-lambda-protocol-auto-confirms#task#5): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameter', 'Message': 'Invalid parameter: Lambda endpoint ARN'}, 'RequestId': <present>}, got None.
Cause 3 (sns-subscription-pending-and-error-paths#task#6): status expected 400, got 200; $.ErrorResponse missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameter', 'Message': 'Invalid parameter: SubscriptionArn Reason: Cannot unsubscribe a subscription that is pending confirmation'}, 'RequestId': 'd8ba7d68-1200-580b-a372-1f85a7d39c21'}, got None.
Cause 4 (sns-subscription-pending-and-error-paths#task#9): status expected 200, got 404; $.UnsubscribeResponse missing (expected an UnsubscribeResponse/ResponseMetadata body, got None).
The generated topic/subscription ids and request ids above are environment-specific examples; the requirements are the PendingConfirmation sentinel, the 400/InvalidParameter rejections and the 200 success.

## Steps to Reproduce
1. Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator.
2. Cause 1: fixture sns-subscription-pending-and-error-paths, step task#4 — Subscribe a non-auto-confirming protocol to a topic, then ListSubscriptionsByTopic and read Subscriptions.member.SubscriptionArn.
3. Cause 2: fixture sns-subscribe-lambda-protocol-auto-confirms, step task#5 — call Subscribe with Protocol='lambda' and a non-ARN Endpoint; the call is accepted with 200.
4. Cause 3: fixture sns-subscription-pending-and-error-paths, step task#6 — call Unsubscribe with the ARN of a still-unconfirmed subscription; the call is accepted with 200.
5. Cause 4: same fixture, step task#9 — call Unsubscribe for the subscription the fixture still expects to exist; the emulator answers 404. Re-replay this step after cause 3 is fixed.
