Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.filter-policy-string-operators-decide-delivery sns.unsubscribe-stops-delivery
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.filter-policy-string-operators-decide-delivery|_await_bodies#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: sns.unsubscribe-stops-delivery|_drain_for#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sns: filter-policy string operators do not decide delivery, and Unsubscribe's effect on delivery is unobservable — both checks hang without a verdict (2 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Two net-new root causes on the SNS data plane. Several SNS issues are open (notably 20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md and 20261025T064350Z-sns-9-behavioral-divergences-unsigned-notification-envelopes-unvalidated.md) but neither carries these two checks' replay keys.

Cause 1 of 2: a subscription filter policy's string operators do not select which publishes are delivered.
AWS: a subscription's FilterPolicy is evaluated at PUBLISH time against the message attributes (or, with FilterPolicyScope=MessageBody, against the body). The string operator vocabulary - exact value match, prefix, suffix, equals-ignore-case, anything-but (including anything-but with prefix) - decides delivery per subscription: a matching publish is delivered, a non-matching publish is dropped and never queued. Publish models InvalidParameterException (400/InvalidParameter) for a malformed policy, so a policy that is accepted is a policy AWS will apply. Recorded behavior only for the per-operator latency.
Emulator: the check runs to the harness cap and is terminated at 60 s. Deliveries that the operators should admit do not arrive, so the matching/non-matching differential the check measures across the operator set never materialises and no operator's behaviour can be confirmed.
Why: message filtering is the mechanism clients use to fan one topic out to many differently-interested subscribers. If matching publishes are not delivered, consumers starve; and because non-matching publishes are also absent, every "filtered out" assertion passes vacuously, so a wrong policy cannot be detected here.
Fix: the Publish delivery path's filter-policy evaluator - implement the full string operator vocabulary (exact, prefix, suffix, equals-ignore-case, anything-but and anything-but+prefix) for both MessageAttributes and MessageBody scope, and apply it per subscription before enqueueing.
Traces: sns.filter-policy-string-operators-decide-delivery ("Check timed out after 60.0s").

Cause 2 of 2: the before/after effect of Unsubscribe on delivery cannot be established.
AWS: Unsubscribe (Smithy: SubscriptionArn required; errors AuthorizationErrorException 403/AuthorizationError, InvalidParameterException 400, NotFoundException 404) deletes the subscription. A Publish issued before Unsubscribe is delivered to the endpoint; a Publish issued after it is not delivered to that endpoint at all.
Emulator: the check runs to the harness cap and is terminated at 60 s - the pre-Unsubscribe delivery that the check needs as its baseline never arrives, so the post-Unsubscribe silence proves nothing and the check cannot conclude.
Why: subscription lifecycle is unverifiable: a client cannot confirm that tearing a subscription down actually stops delivery, and the "stopped" state is indistinguishable from "never delivered" - exactly the ambiguity a subscriber-teardown test exists to remove.
Fix: the Publish delivery path to subscribed endpoints and the Unsubscribe handler's removal of the subscription from the delivery set.
Traces: sns.unsubscribe-stops-delivery ("Check timed out after 60.0s").

## Evidence
Cause 1 - expected: for each string operator, the matching publish is delivered to the subscriber and the non-matching publish is not; actual: no matching delivery ever arrives and the harness terminates the check: "Error: Check timed out after 60.0s". Cause 2 - expected: a publish before Unsubscribe is delivered and a publish after it is not; actual: the baseline delivery never arrives and the harness terminates the check: "Error: Check timed out after 60.0s".

## Steps to Reproduce
Cause 1: CreateTopic, Subscribe an endpoint per operator with SetSubscriptionAttributes(FilterPolicy) using exact/prefix/suffix/equals-ignore-case/anything-but forms, then Publish one matching and one non-matching message per operator - no matching delivery arrives.
Cause 2: CreateTopic, Subscribe an SQS queue, Publish (expect delivery), Unsubscribe, Publish again (expect no delivery) - the first publish is never delivered.
