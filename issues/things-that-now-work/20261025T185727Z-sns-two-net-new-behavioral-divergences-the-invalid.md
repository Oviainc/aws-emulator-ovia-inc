Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.list-topics-and-subscriptions-forged-next-token-rejected sns.list-subscriptions-by-topic-pagination-exactness sns.cross-region-sqs-subscription-delivers-with-the-topic-region-envelope
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.list-topics-and-subscriptions-forged-next-token-rejected|run#6|AssertionError|assert message == "Invalid parameter: NextToken", (
Issue-Key: sns.list-subscriptions-by-topic-pagination-exactness|run#19|AssertionError|assert message == "Invalid parameter: NextToken", (
Issue-Key: sns.cross-region-sqs-subscription-delivers-with-the-topic-region-envelope|run#19|AssertionError|assert sub not in away_listing, "the queue region's ListSubscriptions lists the topic region's subscription"
Issue-Commit: 5dcd1bd7969c2f584c7eb42c4a26285019dddcd0

# SNS: two net-new behavioral divergences — the invalid-NextToken refusal message carries an extra 'Reason:' clause, and a subscription is listed by another region's account-wide ListSubscriptions (2 causes)

> _Note: this issue was identified against the code base at commit `5dcd1bd7969c2f584c7eb42c4a26285019dddcd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptions

## Description
Two net-new root causes in the SNS emulator. Neither is covered by any open SNS issue (the closest, 20261025T025317Z-sns-sqs-listsubscriptionsbytopic-serves-another-region-s-topic.md, is about ListSubscriptionsByTopic ACCEPTING a foreign-region TopicArn, not about an account-wide listing in a second region returning another region's subscriptions; and no open SNS issue mentions NextToken at all).

Cause 1 of 2 — the InvalidParameter refusal for a NextToken SNS never issued appends a 'Reason:' clause AWS does not send.
AWS: ListTopics, ListSubscriptions and ListSubscriptionsByTopic all take a NextToken (Smithy: sns#ListTopics / sns#ListSubscriptions / sns#ListSubscriptionsByTopic, input member NextToken -> target `nextToken`, paginated inputToken=NextToken/outputToken=NextToken) and all three model InvalidParameterException ('Indicates that a request parameter does not comply with the associated constraints.') with httpError 400 and awsQueryError code `InvalidParameter`, httpResponseCode 400. For a token the service never minted, AWS answers HTTP 400, Error.Code `InvalidParameter`, and Error.Message exactly `Invalid parameter: NextToken`. That message is entirely fixed text — the only variable part AWS ever substitutes into this `Invalid parameter: <member>` template is the offending member name (here `NextToken`); the rejected token value and the reason are NOT part of the message.
Emulator: the code and status are right but the message is `Invalid parameter: NextToken Reason: token is invalid or expired` — the fixed text is suffixed with a ` Reason: token is invalid or expired` clause that AWS never emits. Replay: `AssertionError: unexpected ListTopics forged-token message: 'Invalid parameter: NextToken Reason: token is invalid or expired'` (check_sns_list_topics_and_subscriptions_forged_next_token_rejected.py line 58, assert message == "Invalid parameter: NextToken") and `AssertionError: expected the exact InvalidParameter message AWS returns, got 'Invalid parameter: NextToken Reason: token is invalid or expired'` (check_sns_list_subscriptions_by_topic_pagination_exactness.py line 204). So all three list operations share one message builder with the same divergence.
Why: clients and test suites that assert on the refusal text (or that map SNS error messages into their own user-facing errors, log matchers or alarm filters) see a message that never occurs against real SNS; the divergence silently inverts a negative test that passes locally and fails in production. The suffix also leaks an implementation detail ('expired') for a condition SNS does not distinguish.
Fix: the shared NextToken-validation/refusal path for the SNS list handlers in the SNS service crate (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/list_topics.rs, list_subscriptions.rs, list_subscriptions_by_topic.rs, or the common pagination helper they call) — emit InvalidParameterException with awsQuery code `InvalidParameter`, HTTP 400, and the message `Invalid parameter: <member>` with no trailing reason clause.
Traces: sns.list-topics-and-subscriptions-forged-next-token-rejected, sns.list-subscriptions-by-topic-pagination-exactness.

Cause 2 of 2 — account-wide ListSubscriptions in one region returns subscriptions that belong to a topic in a different region.
AWS: SNS is a regional service; a subscription belongs to the region of the topic it extends (its SubscriptionArn is the TopicArn plus a ':<uuid>' suffix, so it carries the topic's region), and the account-wide ListSubscriptions served by a given regional endpoint returns only that region's subscriptions. Smithy: sns#ListSubscriptions takes no scoping input other than NextToken, so the regional endpoint IS the scope. A cross-region subscription (a topic in region A subscribing an SQS queue in region B) is therefore listed by ListSubscriptions in region A and NOT by ListSubscriptions in region B. Recorded behavior only for the per-region partitioning itself (the model does not state it); the recording shows the away-region listing not containing the ARN.
Emulator: the subscription created against the topic's home-region endpoint is also returned by ListSubscriptions on the other region's endpoint. Replay: `AssertionError: the queue region's ListSubscriptions lists the topic region's subscription` (check_sns_cross_region_sqs_subscription_delivers_with_the_topic_region_envelope.py line 65). The preceding assertions passed, so the subscription ARN is correctly minted in the topic's region and the topic's own ListSubscriptionsByTopic is correct — only the account-wide, per-region store is not partitioned by region.
Why: a client enumerating subscriptions per region to reconcile, audit or clean up its topics double-counts every subscription (once per region it polls) and can delete or re-point a subscription it believes belongs to the region it is operating in. Region isolation is a correctness boundary: code that is safe against real SNS because a region only ever sees its own resources becomes destructive here.
Fix: the SNS subscription state store and the ListSubscriptions handler (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/state.rs and src/ops/list_subscriptions.rs) — scope the account-wide listing to the region of the serving endpoint, keying subscriptions by the region of their topic ARN.
Traces: sns.cross-region-sqs-subscription-delivers-with-the-topic-region-envelope.

## Evidence
Cause 1 — invalid-NextToken refusal message.
Expected (AWS): sns.list_topics(NextToken='c2ltMnJlYWwtbm90LWEtcmVhbC10b2tlbg==') -> HTTP 400, Error.Code 'InvalidParameter', Error.Message 'Invalid parameter: NextToken'. Same for sns.list_subscriptions(NextToken=<forged>) and sns.list_subscriptions_by_topic(TopicArn=<arn>, NextToken=<forged>). Fixed text: 'Invalid parameter: ' + the member name; variable part: the member name only ('NextToken').
Actual (emulator): Error.Code 'InvalidParameter' (correct), HTTP 400 (correct), Error.Message 'Invalid parameter: NextToken Reason: token is invalid or expired' — the ' Reason: token is invalid or expired' suffix is not AWS text.
  AssertionError: unexpected ListTopics forged-token message: 'Invalid parameter: NextToken Reason: token is invalid or expired'  (check_sns_list_topics_and_subscriptions_forged_next_token_rejected.py:58)
  AssertionError: expected the exact InvalidParameter message AWS returns, got 'Invalid parameter: NextToken Reason: token is invalid or expired'  (check_sns_list_subscriptions_by_topic_pagination_exactness.py:204)
Smithy grounding: sns#ListTopics / sns#ListSubscriptions / sns#ListSubscriptionsByTopic each list InvalidParameterException (errorKind client, httpError 400, awsQueryError {code: 'InvalidParameter', httpResponseCode: 400}); each has input member NextToken -> nextToken and paginated {inputToken: NextToken, outputToken: NextToken}.

Cause 2 — cross-region subscription leaks into the other region's account-wide listing.
Expected (AWS): with topic T in region A and queue Q in region B, sns(A).subscribe(TopicArn=T, Protocol='sqs', Endpoint=<Q arn>) returns a SubscriptionArn whose region is A and which starts with T + ':'; sns(A).list_subscriptions_by_topic(TopicArn=T) lists exactly that subscription; sns(B).list_subscriptions() does NOT contain it.
Actual (emulator): the ARN and the topic-region listing are correct, but sns(B).list_subscriptions() contains the subscription ARN.
  AssertionError: the queue region's ListSubscriptions lists the topic region's subscription  (check_sns_cross_region_sqs_subscription_delivers_with_the_topic_region_envelope.py:65)

## Steps to Reproduce
Cause 1:
1. sns = boto3.client('sns', endpoint_url=<emulator>, region_name='us-west-2')
2. try: sns.list_topics(NextToken='c2ltMnJlYWwtbm90LWEtcmVhbC10b2tlbg==')
   except ClientError as e: print(e.response['Error']['Code'], repr(e.response['Error']['Message']))
   Observed: InvalidParameter 'Invalid parameter: NextToken Reason: token is invalid or expired'
   Expected: InvalidParameter 'Invalid parameter: NextToken'
3. Repeat with sns.list_subscriptions(NextToken=<same forged token>) and, after creating a topic, sns.list_subscriptions_by_topic(TopicArn=<arn>, NextToken=<same forged token>) — all three return the same extra ' Reason: ...' suffix.
4. Replay: sns.list-topics-and-subscriptions-forged-next-token-rejected, sns.list-subscriptions-by-topic-pagination-exactness

Cause 2:
1. home = 'us-west-2'; away = 'us-east-1'; sns_home = boto3.client('sns', region_name=home, endpoint_url=<emulator>); sns_away = boto3.client('sns', region_name=away, endpoint_url=<emulator>); sqs_away = boto3.client('sqs', region_name=away, endpoint_url=<emulator>)
2. topic = sns_home.create_topic(Name='xr-topic')['TopicArn']
3. url = sqs_away.create_queue(QueueName='xr-queue')['QueueUrl']; q_arn = sqs_away.get_queue_attributes(QueueUrl=url, AttributeNames=['QueueArn'])['Attributes']['QueueArn']
4. sub = sns_home.subscribe(TopicArn=topic, Protocol='sqs', Endpoint=q_arn, ReturnSubscriptionArn=True)['SubscriptionArn']  (ARN is in `home` — correct)
5. print(sub in {s['SubscriptionArn'] for p in sns_away.get_paginator('list_subscriptions').paginate() for s in p['Subscriptions']})
   Observed: True. Expected: False — the away region's account-wide ListSubscriptions must not see the home region's subscription.
6. Replay: sns.cross-region-sqs-subscription-delivers-with-the-topic-region-envelope
