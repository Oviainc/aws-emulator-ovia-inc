Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.unsubscribe-stops-delivery
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.unsubscribe-stops-delivery|_drain_for#16|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=wait)
Issue-Commit: 4ac27352e7b3d7cddd4c7ab08e294db28bfa593a

# sns: Unsubscribe's effect on SQS delivery is still unobservable — the pre-Unsubscribe baseline publish is not delivered and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `4ac27352e7b3d7cddd4c7ab08e294db28bfa593a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Unsubscribe

## Description
One net-new root cause on the SNS data plane, grounded in the one surviving failing check of this suite run that no open issue carries. Context: the resolved issue 20261029T183635Z-sns-filter-policy-string-operators-do-not-decide.md listed this check's replay key (Issue-Key: sns.unsubscribe-stops-delivery|_drain_for#16|Timeout). Its sibling cause (sns.filter-policy-string-operators-decide-delivery) now passes and is gone from the failing list, but this leg still produces no verdict, so the subscription-teardown behaviour it covers is still not emulated. This is NOT a refusal (no error code is returned), so it is filed as the remaining open cause rather than as a refused call.

Cause 1 of 1: a Publish issued while an SQS subscription exists is not delivered to that subscriber, so the before/after effect of Unsubscribe on delivery cannot be established. Failing check: sns.unsubscribe-stops-delivery.
AWS: Subscribe with Protocol=sqs and an Endpoint naming a queue whose policy allows sns.amazonaws.com:SendMessage for aws:SourceArn = the topic returns an auto-confirmed SubscriptionArn (of the form <TopicArn>:<uuid>), and Publish (Smithy: Publish, Message required, TopicArn optional-but-used; errors AuthorizationErrorException 403/AuthorizationError, InvalidParameterException 400/InvalidParameter, NotFoundException 404/NotFound, KMS* 400) delivers the notification to that queue. With the subscription attribute RawMessageDelivery=true the queue body is the bare Message string. Unsubscribe (Smithy: SubscriptionArn required; errors AuthorizationErrorException 403/AuthorizationError, InvalidParameterException 400/InvalidParameter, NotFoundException 404/NotFound, InvalidSecurityException 403/InvalidSecurity, InternalErrorException 500) removes the subscription from the topic's delivery set: a Publish issued after it is delivered to no endpoint, the queue depth (ApproximateNumberOfMessages) stays 0, and GetSubscriptionAttributes on the removed ARN answers HTTP 404 with error code NotFound and message "Subscription does not exist" (that whole sentence is fixed text, with no variable part).
Emulator: the check is terminated by the harness at the 60 s cap ("Error: Check timed out after 60.0s") before it can reach any of those assertions. The baseline leg long-polls the subscribed queue for a 30 s window for the message published while the subscription was live; the message does not show up in time for the check to proceed, and the check never gets to the post-Unsubscribe silence window, the ApproximateNumberOfMessages == 0 reading, or the GetSubscriptionAttributes NotFound assertion. Nothing is refused: no SNS or SQS call returns an error, delivery simply does not become observable.
Hypothesis: the per-check wall clock is dominated by the baseline delivery latency, because the check sizes its post-Unsubscribe hold window at 3x the latency it observed on the baseline leg; a baseline that only settles at (or past) the end of its 30 s drain window pushes the total past the 60 s cap even when the later legs would succeed. Only the emulator's SNS->SQS delivery latency on a live, auto-confirmed raw subscription is in question here; the model states nothing about latency, so this part is recorded behaviour only.
Why: subscription lifecycle becomes unverifiable for a client. A consumer cannot confirm that tearing a subscription down actually stops delivery, because "stopped" is indistinguishable from "never delivered" — the exact ambiguity a teardown test exists to remove — and a client that waits on a notification published to a live subscription waits forever instead of receiving it.
Fix: the Publish delivery path that enqueues notifications to subscribed sqs endpoints (it must deliver promptly to an auto-confirmed subscription, honouring RawMessageDelivery), and the Unsubscribe handler's removal of the subscription from that delivery set plus the NotFound (404, "Subscription does not exist") answer GetSubscriptionAttributes must then give for the removed ARN.
Traces: sns.unsubscribe-stops-delivery.

## Evidence
Expected: (1) the publish issued while the sqs subscription is live arrives in the queue as the bare message body (RawMessageDelivery=true); (2) after Unsubscribe, a further publish never arrives and ApproximateNumberOfMessages reads 0; (3) GetSubscriptionAttributes on the unsubscribed ARN fails with HTTP 404, Error.Code "NotFound" and Error.Message "Subscription does not exist". Actual: the check never reaches assertion (2) or (3) — the harness terminates it with "Error: Check timed out after 60.0s" while draining the queue for the baseline publish (the timeout is raised inside the drain loop's sqs.receive_message long poll, matching the replay key _drain_for#16|Timeout recorded on the resolved issue 20261029T183635Z-sns-filter-policy-string-operators-do-not-decide.md). No SNS/SQS call in the check returns an error status or code.

## Steps to Reproduce
1. sns.CreateTopic(Name=<unique>).
2. sqs.CreateQueue(<unique>), read QueueArn, and SetQueueAttributes with a policy allowing Principal Service sns.amazonaws.com, Action sqs:SendMessage on that queue ARN, Condition ArnEquals aws:SourceArn = the topic ARN.
3. sns.Subscribe(TopicArn=<topic>, Protocol="sqs", Endpoint=<queue arn>, Attributes={"RawMessageDelivery":"true"}) and observe the returned SubscriptionArn starts with "<TopicArn>:".
4. sns.Publish(TopicArn=<topic>, Message="baseline-<uuid>") and long-poll sqs.ReceiveMessage on the queue — on AWS the bare body arrives within seconds; on the emulator the drain window elapses without it and the check is killed at the 60 s cap.
5. (Unreached) sns.Unsubscribe(SubscriptionArn=<sub>), sns.Publish again, long-poll: nothing should arrive and ApproximateNumberOfMessages should be 0.
6. (Unreached) sns.GetSubscriptionAttributes(SubscriptionArn=<sub>) must fail 404 NotFound / "Subscription does not exist".
