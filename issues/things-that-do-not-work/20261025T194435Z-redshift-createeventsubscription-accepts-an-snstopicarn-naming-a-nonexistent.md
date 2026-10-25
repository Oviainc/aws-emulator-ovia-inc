Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.event-subscription-missing-topic-fails
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.event-subscription-missing-topic-fails|_refusal#10|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Commit: a2837dbdebef5b9a30be37f112a3c5b63a2de779

# Redshift CreateEventSubscription accepts an SnsTopicArn naming a nonexistent SNS topic and stores an unusable subscription (1 cause)

> _Note: this issue was identified against the code base at commit `a2837dbdebef5b9a30be37f112a3c5b63a2de779`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSubscription

## Description
One root cause in the Redshift event-notification surface, reached from the redshift negative-class check.

Cause 1 of 1: CreateEventSubscription performs no existence check on SnsTopicArn, so a subscription naming a topic that does not exist is created (HTTP 200) instead of being refused.

AWS: CreateEventSubscription validates that the named SNS topic exists before registering the subscription. Smithy (com-amazonaws-redshift-redshiftserviceversion20121201): CreateEventSubscription input requires SubscriptionName and SnsTopicArn; its modeled errors include SNSTopicArnNotFoundFault — awsQueryError code `SNSTopicArnNotFound`, HTTP 404 — documented as "An Amazon SNS topic with the specified Amazon Resource Name (ARN) does not exist." Recorded behavior for the message: HTTP 404, error code `SNSTopicArnNotFound`, message `Failed to create Subscription. Topic: <SnsTopicArn> not found.` — the fixed text is "Failed to create Subscription. Topic: " and " not found."; the single variable part is the SnsTopicArn copied verbatim from the request (never a value pinned from one run). Because the call is refused, no subscription exists afterwards, so a subsequent DescribeEventSubscriptions(SubscriptionName=<name>) must answer SubscriptionNotFoundFault — awsQueryError code `SubscriptionNotFound`, HTTP 404 (Smithy: DescribeEventSubscriptions errors are InvalidTagFault and SubscriptionNotFoundFault) — with message `Event Subscription <SubscriptionName> not found`, fixed text "Event Subscription " + " not found" around the requested subscription name.

Emulator: the identical call SUCCEEDS. The check's refusal helper reports `AssertionError: expected the call to be refused, but it succeeded` — no error code, no 404; the subscription is registered against a topic ARN that resolves to nothing. (Note this also shows CreateEventSubscription is no longer a NotImplemented scaffold: the open issue 20261024T024248Z-redshift-sqs-redshift-event-notification-operations-answer-notimplemented-so-no.md describes the earlier NotImplemented behavior of this operation and does not cover the missing topic validation.)

Why: a client that relies on AWS's validation gets a silently dead notification path — the subscription appears healthy in DescribeEventSubscriptions while no event can ever be delivered, and the SNSTopicArnNotFound error that tells the caller to create the topic first never arrives. Code written to branch on that error code has an unreachable path, and the emulator's accept-everything behavior masks exactly the misconfiguration the operation exists to catch.

Fix: the CreateEventSubscription handler and its request validation in services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_event_subscription.rs — resolve SnsTopicArn against SNS topic state before mutating the subscription registry, and return the modeled SNSTopicArnNotFoundFault (404 / `SNSTopicArnNotFound`) with the message template above; leave no subscription behind on the refusal path so DescribeEventSubscriptions still answers SubscriptionNotFound for that name.

Traces: redshift.event-subscription-missing-topic-fails.

## Evidence
Cause 1 — redshift.event-subscription-missing-topic-fails:

  File "check_redshift_event_subscription_missing_topic_fails.py", line 82, in run
    code, message = _refusal(_create)
  File "check_redshift_event_subscription_missing_topic_fails.py", line 51, in _refusal
    raise AssertionError("expected the call to be refused, but it succeeded")
  AssertionError: expected the call to be refused, but it succeeded

Call made: redshift.create_event_subscription(SubscriptionName=<generated name>, SnsTopicArn=arn:aws:sns:<region>:<account>:<topic that was never created>, SourceType="cluster", Severity="INFO", Tags=[...]).

Expected (AWS): HTTP 404, Error.Code = "SNSTopicArnNotFound", Error.Message = "Failed to create Subscription. Topic: <SnsTopicArn> not found." — then DescribeEventSubscriptions(SubscriptionName=<name>) answers HTTP 404, Error.Code = "SubscriptionNotFound", Error.Message = "Event Subscription <SubscriptionName> not found".
Actual (emulator): HTTP 200 with an EventSubscription; no error code and no message at all; the subscription is stored.

Positive control the same check carries: after the named topic is actually created with sns.create_topic, the identical CreateEventSubscription call must succeed and DescribeEventSubscriptions must show exactly one subscription with that SnsTopicArn — so the required refusal is provably about the missing topic, not a blanket rejection of the operation.

(Subscription names, topic names, region and account id are per-run values that fill the message templates; only the fixed text and the request-derived substitution are requirements.)

## Steps to Reproduce
1. Boot the emulator and point redshift, sns and sts clients at it.
2. Build a syntactically valid but nonexistent topic ARN: arn:aws:sns:<region>:<account>:<name-never-created>.
3. Call redshift CreateEventSubscription with SubscriptionName=<name>, SnsTopicArn=<that ARN>, SourceType="cluster", Severity="INFO". Expect HTTP 404 with error code SNSTopicArnNotFound and message "Failed to create Subscription. Topic: <arn> not found."; observe a successful 200 response instead.
4. Call redshift DescribeEventSubscriptions(SubscriptionName=<name>). Expect HTTP 404 SubscriptionNotFound with "Event Subscription <name> not found"; observe the subscription present.
5. Create the SNS topic with that exact name and repeat step 3 — it must now succeed (positive control).
6. Re-replay redshift.event-subscription-missing-topic-fails to confirm the fix.
