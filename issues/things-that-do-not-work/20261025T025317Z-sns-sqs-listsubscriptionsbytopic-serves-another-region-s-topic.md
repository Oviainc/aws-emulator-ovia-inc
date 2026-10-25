Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.list-subscriptions-by-topic-with-another-regions-topic-arn-rejected sns__sqs.e2e-count-sns-topic-sqs-subscribers-lifecycle
Issue-Key: sns__sqs.list-subscriptions-by-topic-with-another-regions-topic-arn-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: sns__sqs.e2e-count-sns-topic-sqs-subscribers-lifecycle|_receive#5|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# sns__sqs: ListSubscriptionsByTopic serves another region's topic ARN, and the topic/queue subscriber lifecycle never completes (2 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptionsByTopic

## Description
Cause 1 of 2: a regional SNS endpoint answers ListSubscriptionsByTopic for a topic ARN belonging to another region.
AWS: the call is refused (recorded behavior: real AWS rejects it, so there is no success and no subscription list); a topic ARN must name the endpoint's own region.
Emulator: HTTP 200 with a subscription list.
Why: region isolation is not modeled, so a client's region mistake silently reads a different region's topology and cross-region tests cannot tell the two regions apart.
Fix: the ARN-region check in the ListSubscriptionsByTopic handler (shared with the other SNS topic lookups).
Traces: sns__sqs.list-subscriptions-by-topic-with-another-regions-topic-arn-rejected.

Cause 2 of 2: the count-subscribers lifecycle over a topic with SQS subscribers never completes.
AWS: subscribing queues to a topic, publishing and tearing them down is observable end to end within seconds — the published message reaches each subscribed queue and the topic's subscription counts follow the lifecycle.
Emulator: the check hangs to the 60 s cap, because no notification is ever delivered to the subscribed queues (same missing SNS delivery path reported for the sns service in the "SNS performs no out-of-band delivery" issue filed in this run).
Why: an SNS→SQS fan-out cannot be exercised or counted at all.
Fix: the SNS delivery path invoked by Publish (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs).
Traces: sns__sqs.e2e-count-sns-topic-sqs-subscribers-lifecycle.

## Evidence
- sns__sqs.list-subscriptions-by-topic-with-another-regions-topic-arn-rejected: AssertionError: the call succeeded; real AWS rejects it, so there is no error code
- sns__sqs.e2e-count-sns-topic-sqs-subscribers-lifecycle: Error: Check timed out after 60.0s

## Steps to Reproduce
1. CreateTopic in us-west-2; call ListSubscriptionsByTopic against the us-east-1 endpoint with the us-west-2 topic ARN -> 200 with a list (AWS: refused).
2. CreateTopic + two CreateQueue; Subscribe both (Protocol=sqs); Publish; ReceiveMessage on each queue in a loop -> nothing arrives before the 60 s cap.
