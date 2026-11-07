Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish
Issue-Key: lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure|run#27|AssertionError|assert envelope.get("Message") == published_text, f"unexpected DLQ envelope: {envelope!r}"
Issue-Key: lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery|_drain#16|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish|_drain#18|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: dcaafa445ca563c5947bd284ba4541134d1f4f9e

# lambda__sns__sqs: 3 net-new root causes — an SNS subscription's dead-letter queue receives the Lambda invocation payload instead of the notification, and neither a SourceArn-scoped invoke grant nor a MessageBody-scoped filter policy ever reaches a verdict

> _Note: this issue was identified against the code base at commit `dcaafa445ca563c5947bd284ba4541134d1f4f9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Three net-new root causes found by replaying the behavioral suite. Each is numbered with its own failing check ids; all three must be closed before this issue is submitted.

=== Cause 1 of 3: the subscription dead-letter queue receives the Lambda-protocol invocation payload instead of the SNS notification ===
AWS: recorded behavior only (the dead-letter document is out-of-band and not in the Smithy model; only SetSubscriptionAttributes' RedrivePolicy attribute is modeled). AWS (SNS dead-letter queues) moves a message SNS could not deliver to the subscribed endpoint into the subscription's deadLetterTargetArn queue AS THE NOTIFICATION IT TRIED TO DELIVER: a JSON document whose top level is the notification — Type ('Notification'), MessageId, TopicArn (the publishing topic), Subject when one was published, Message (the published body verbatim), Timestamp, SignatureVersion, Signature, SigningCertURL, UnsubscribeURL. Its fixed text is those member names; the values come from the publish and the topic. There is no Records/EventSource wrapper at the top level — that wrapper exists only in the payload handed to a Lambda endpoint.
Emulator: the DLQ body is the Lambda invocation payload instead: {"Records": [{"EventSource": "aws:sns", "EventVersion": "1.0", "EventSubscriptionArn": "<sub arn>", "Sns": {"Type": "Notification", "MessageId": "<id>", "TopicArn": "<topic>", "Message": "undeliverable-because-no-invoke-permission", "Timestamp": "...", "MessageAttributes": {}, "UnsubscribeUrl": "..."}}]}. Read at the top level, Message and TopicArn are therefore absent (None). The dead-lettering itself does happen and the message content is intact one level down.
Why: a DLQ consumer or redriver parses the notification at the top level — it reads Message to recover the published body and TopicArn/MessageId to replay or correlate it. Against this emulator every dead-lettered message fails that parse (or silently yields None), so an operator's redrive tooling drops exactly the traffic SNS could not deliver.
Fix: the SNS subscription dead-letter path in the delivery pipeline — it must enqueue the notification document the delivery attempt carried (the same document the envelope serializer builds), not the protocol-specific endpoint payload.
Traces: lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure.

=== Cause 2 of 3: a lambda:InvokeFunction grant scoped by SourceArn to one topic never produces an observable outcome ===
AWS: recorded behavior only. AddPermission(FunctionName=f, Principal='sns.amazonaws.com', Action='lambda:InvokeFunction', SourceArn=topicA) admits deliveries only from topicA: with the SAME function subscribed to both topicA and topicB, publishes to topicA invoke it within seconds (each invocation arriving as a one-record aws:sns event naming its own TopicArn), publishes to topicB never invoke it, and a second AddPermission statement with SourceArn=topicB then makes topicB's publishes invoke it. The recording settles inside the suite's 60 s budget.
Emulator: the check is cut off — 'Error: Check timed out after 60.0s'. The scenario never reaches a verdict: the control barrier (publish to both topics, drain the forwarder function's SQS sink, require at least one record whose Sns.TopicArn is topicA) does not complete, so neither the admitted delivery nor the SourceArn refusal is ever observable. Hypothesis: SNS->Lambda delivery for these subscriptions either does not occur or does not occur promptly, so the per-topic authorization decision cannot be exercised at all.
Why: an enforced SourceArn scope and a dead delivery path are indistinguishable to a client; a grant that is missing, or over-broad (admitting every topic), cannot be detected, and the client only ever sees its own timeout with no error to diagnose.
Fix: the SNS Publish delivery path for lambda-protocol subscriptions and the per-subscription evaluation of the target function's resource policy (Principal/SourceArn) — delivery must complete promptly for admitted topics and be withheld only for topics the policy does not admit.
Traces: lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery.

=== Cause 3 of 3: a FilterPolicyScope=MessageBody filter policy on a Lambda subscription never produces an observable outcome ===
AWS: recorded behavior only for the delivery effect; SetSubscriptionAttributes accepts the subscription attributes FilterPolicyScope ('MessageBody' or 'MessageAttributes') and FilterPolicy (a JSON policy document). With FilterPolicyScope='MessageBody' and FilterPolicy {"kind":["order"]}, a publish whose JSON message body has kind='order' IS delivered to the subscribed function, a body with kind='audit' is NOT, and an unfiltered twin subscription on the same topic receives both. The recording settles inside the suite's 60 s budget.
Emulator: the check is cut off — 'Error: Check timed out after 60.0s'. The first barrier (a matching kind=order body reaching the filtered function's sink) and/or the twin's audit delivery never arrives, so the filter's admit/withhold decision is never observable.
Why: a subscriber that scopes filtering to the message body gets nothing and no error on this emulator; worse, because the matching case cannot be observed either, a client cannot tell "my filter is wrong" from "body-scoped filtering is not applied at all".
Fix: the subscription filter-policy evaluator's MessageBody scope (parse the published Message as JSON and match the policy against its leaf keys) together with the lambda-protocol delivery path that must deliver the matching publish.
Traces: lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish.

## Evidence
Cause 1 — lambda__sns__sqs.sns-lambda-dlq-on-invoke-failure [mismatch]
  Setup: topic with two lambda subscriptions; only the twin has lambda:InvokeFunction for sns.amazonaws.com; the ungranted subscription carries RedrivePolicy {"deadLetterTargetArn": <dlq>}; one Publish with Message='undeliverable-because-no-invoke-permission'.
  Expected (AWS): the DLQ body parses to a notification whose top-level Message == the published body and TopicArn == the topic.
  Actual (emulator): AssertionError: unexpected DLQ envelope: {'Records': [{'EventSource': 'aws:sns', 'EventVersion': '1.0', 'EventSubscriptionArn': 'arn:aws:sns:<region>:<account>:<topic>:<subid>', 'Sns': {'Type': 'Notification', 'MessageId': '<id>', 'TopicArn': 'arn:aws:sns:<region>:<account>:<topic>', 'Message': 'undeliverable-because-no-invoke-permission', 'Timestamp': '<ts>', 'MessageAttributes': {}, 'UnsubscribeUrl': '<url>'}}]} — top-level 'Message' and 'TopicArn' are absent. (ids/arns/timestamps above are per-run values, not requirements.) The positive-control twin WAS invoked in the same run, so SNS->Lambda delivery and the in-function SDK call work here.

Cause 2 — lambda__sns__sqs.snstrig-grant-scoped-to-another-topic-blocked-delivery [mismatch]
  Expected (AWS): >=1 sink record whose Sns.TopicArn is the granted topic, 0 records from the ungranted topic over a window sized off that latency, then >=1 from the ungranted topic after a second AddPermission scoped to it.
  Actual (emulator): 'Error: Check timed out after 60.0s' — no verdict; the granted-topic delivery barrier never completed.

Cause 3 — lambda__sns__sqs.snstrig-message-body-filter-blocked-a-non-matching-publish [mismatch]
  Expected (AWS): the filtered function receives the kind='order' body; the unfiltered twin receives the kind='audit' body; the filtered function receives 0 audit bodies.
  Actual (emulator): 'Error: Check timed out after 60.0s' — no verdict; no delivery satisfying the first barrier was observed.

Related open issues that do NOT cover these (none carries these replay keys): 20261107T014900Z-lambda-sns-sqs-async-destinations-and-dead-letter.md (Lambda's own async destinations/DLQ, not an SNS subscription RedrivePolicy), 20261104T044826Z-lambda-sns-revoking-one-sns-to-lambda-invoke-permission-is.md (RemovePermission in the lambda__sns group), 20261102T093047Z-lambda-sns-3-net-new-root-causes-a.md (a non-JSON FilterPolicy refusal, not MessageBody scope).

## Steps to Reproduce
1. git_clone the repo, then run_behavioral_suite and re-replay exactly the three check ids in trace_ids.
2. Cause 1: CreateQueue (DLQ) and allow sns.amazonaws.com SQS:SendMessage on it with ArnEquals aws:SourceArn = the topic. CreateTopic. Create two python3.12 functions; AddPermission(lambda:InvokeFunction, Principal=sns.amazonaws.com, SourceArn=topic) for ONE of them only. Subscribe both with Protocol='lambda'. SetSubscriptionAttributes(RedrivePolicy={"deadLetterTargetArn": <dlq arn>}) on the ungranted subscription. Publish once, then ReceiveMessage from the DLQ and json.loads the body: observe the {"Records":[...]} wrapper instead of the notification.
3. Cause 2: one function that forwards its event to an SQS sink, subscribed to two topics, with AddPermission SourceArn-scoped to topic A only. Publish to both and drain the sink: no record from A arrives, so the scenario never settles; the harness kills it at 60 s.
4. Cause 3: two forwarder functions subscribed to one topic, both granted; on one subscription SetSubscriptionAttributes FilterPolicyScope='MessageBody' then FilterPolicy={"kind":["order"]}. Publish {"kind":"order",...} and {"kind":"audit",...} and drain the sink: the matching delivery to the filtered function is never observed; the harness kills it at 60 s.
