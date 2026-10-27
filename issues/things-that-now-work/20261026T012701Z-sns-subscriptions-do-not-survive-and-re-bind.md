Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.recreated-queue-with-the-same-name-resumes-delivery-on-the-old-subscription
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.recreated-queue-with-the-same-name-resumes-delivery-on-the-old-subscription|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: a2be7a3586c8a34d922aa1b24d2f445b80a06855

# SNS subscriptions do not survive and re-bind to their endpoint ARN across a queue delete/recreate, and an undeliverable publish is never dead-lettered to the subscription RedrivePolicy DLQ

> _Note: this issue was identified against the code base at commit `a2be7a3586c8a34d922aa1b24d2f445b80a06855`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new behavioral root cause, grounded in the only sns check the replay report left unattributed to an existing issue.

Cause 1 of 1: a subscription's binding to a deleted-and-recreated endpoint, and the dead-lettering of the publishes made in the gap, are both unobservable because no publish ever leaves SNS.

AWS: Smithy/Publish doc — 'If you send a message to a topic, Amazon SNS delivers the message to each endpoint that is subscribed to the topic. ... When a messageId is returned, the message is saved and Amazon SNS immediately delivers it to subscribers.' A subscription binds to the ENDPOINT ARN it was created with, not to any internal identity of the endpoint: deleting the subscribed SQS queue does not delete, unconfirm or re-point the subscription (GetSubscriptionAttributes keeps Endpoint = the original queue ARN and PendingConfirmation = 'false'; Smithy: GetSubscriptionAttributes -> Attributes map, errors AuthorizationErrorException 403/AuthorizationError, InvalidParameterException 400/InvalidParameter, NotFoundException 404/NotFound). While the queue is gone the delivery attempt fails and, with a subscription RedrivePolicy set by SetSubscriptionAttributes(AttributeName='RedrivePolicy', AttributeValue='{"deadLetterTargetArn":<dlq arn>}'), the exhausted notification is moved to that dead-letter queue as a Type=Notification envelope. A queue created again under the SAME name has the SAME ARN, so once it exists and its policy allows sns.amazonaws.com:sqs:SendMessage for the topic, the UNCHANGED subscription resumes delivering to it — and the messages published during the gap stay in the DLQ: they are NOT buffered and replayed into the recreated queue.

Emulator: nothing is ever handed to the subscribed queue in either phase. The check publishes up to six gap markers while the queue is deleted and long-polls the DLQ for each (20 s window per attempt); no envelope ever lands in the DLQ, so the loop runs until the harness cuts it: 'Error: Check timed out after 60.0s'. The recreate/ARN-identity half (CreateQueue under the same name must yield the same QueueArn, GetSubscriptionAttributes must still report the original Endpoint with PendingConfirmation='false', and the unchanged subscription must deliver the next publish into the new queue while no gap marker is replayed into it) is therefore never reached. Publish itself answers 200 with a MessageId throughout.

This is grounded in the same missing out-of-band delivery path as the open issue 20261025T025317Z-sns-performs-no-out-of-band-delivery-a.md (its cause 1, no fan-out to subscribed endpoints; its cause 4, no retry/dead-lettering) — but the replay report did NOT mark this check as owned by it, so that issue's replay key no longer binds this check and a fixer closing it will not re-run this scenario. It also adds a property no filed issue states: that a subscription binds by endpoint ARN and must keep working across a delete/recreate of the endpoint under the same name, with no replay of gap messages into the new queue. An emulator that binds a subscription to an internal queue id, or that buffers withheld notifications and flushes them into the recreated queue, would be wrong here and right everywhere else.

Why: tearing a queue down and recreating it under a fixed name is the everyday local-development loop. A client that does it gets a topic whose subscription looks healthy (GetSubscriptionAttributes reports the right Endpoint and a confirmed subscription) but that delivers nothing, with no dead-letter evidence of the messages lost in the gap — and, if gap messages were buffered instead, would silently receive stale messages after the recreate. Both failure modes are invisible against the emulator because no delivery happens at all.

Fix: the SNS delivery path — the Publish handler's fan-out step must resolve each confirmed subscription's Endpoint ARN at delivery time (so a recreated endpoint with the same ARN is reached and a currently-absent one fails the attempt) and the retry/redrive stage must move an exhausted notification to the subscription RedrivePolicy's deadLetterTargetArn instead of buffering it for later replay.

Traces: sns.recreated-queue-with-the-same-name-resumes-delivery-on-the-old-subscription.

## Evidence
Expected (AWS): with a confirmed sqs subscription whose RedrivePolicy names a DLQ, after DeleteQueue on the subscribed queue a Publish is dead-lettered — the DLQ receives {"Type":"Notification","MessageId":<the id Publish returned>,"TopicArn":...,"Message":<the gap marker verbatim>,...}. CreateQueue under the deleted queue's name then returns a queue whose QueueArn equals the original ARN; GetSubscriptionAttributes still reports (Endpoint=<that ARN>, PendingConfirmation='false'); the next Publish is delivered into the recreated queue, and draining it yields no gap marker.

Actual (emulator): sns.recreated-queue-with-the-same-name-resumes-delivery-on-the-old-subscription — verdict 'mismatch', 'Error: Check timed out after 60.0s'. The DLQ long-poll (ReceiveMessage MaxNumberOfMessages=10, WaitTimeSeconds=5, 20 s window, repeated for up to six gap publishes) returns no messages, so the check never reaches its 'no publish made while the queue was deleted reached the DLQ' assertion nor any of the recreate assertions. Publish returns 200 with a MessageId for every gap marker. The open issue 20261025T025317Z-sns-performs-no-out-of-band-delivery-a.md documents the same underlying path (Publish calls store.record_delivery into SNS's own state and never contacts the subscribed queue) but no longer carries this check's replay key.

## Steps to Reproduce
1. CreateTopic. CreateQueue twice (a DLQ and a subscriber queue), reading each QueueArn from GetQueueAttributes, and set each queue's Policy to allow Principal {Service: sns.amazonaws.com} sqs:SendMessage on that ARN with Condition ArnEquals aws:SourceArn = the topic ARN.
2. Subscribe(TopicArn, Protocol='sqs', Endpoint=<subscriber queue arn>, ReturnSubscriptionArn=true); poll GetSubscriptionAttributes until PendingConfirmation='false'.
3. SetSubscriptionAttributes(SubscriptionArn, AttributeName='RedrivePolicy', AttributeValue='{"deadLetterTargetArn":"<dlq arn>"}') and poll GetSubscriptionAttributes until the read-back agrees.
4. DeleteQueue on the subscriber queue. Publish a unique marker and long-poll the DLQ for up to 20 s: AWS dead-letters the notification, the emulator's DLQ stays empty (repeat up to six times).
5. Poll CreateQueue(QueueName=<the deleted queue's name>) until it stops answering AWS.SimpleQueueService.QueueDeletedRecently, then GetQueueAttributes(QueueArn) — it must equal the original ARN — and set the same SNS-allowing Policy on it.
6. GetSubscriptionAttributes: Endpoint must still be that ARN and PendingConfirmation must still be 'false'. Publish a new marker and long-poll the recreated queue: AWS delivers it. Then drain the queue and confirm none of the gap markers from step 4 was replayed into it.
