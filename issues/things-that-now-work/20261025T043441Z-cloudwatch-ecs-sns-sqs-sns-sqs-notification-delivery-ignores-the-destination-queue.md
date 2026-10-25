Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy
Issue-Key: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy|run#33|AssertionError|assert not stray, (
Issue-Commit: 77162043f7392d23f93660cf14c6adb796d45cd4

# SNS->SQS notification delivery ignores the destination queue's resource policy: a queue with no sqs:SendMessage grant for SNS receives every notification

> _Note: this issue was identified against the code base at commit `77162043f7392d23f93660cf14c6adb796d45cd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause, grounded in the one surviving (not ALREADY FILED, not OWNED) failure of the cloudwatch__ecs__sns__sqs bucket in this sweep.

This is NOT the previously filed cause for this check id. 20261025T020208Z-cloudwatch-ecs-sns-sqs-an-ecs-runningtaskcount-alarm-never-fires-so-sns.md reports that the alarm never transitioned and nothing was ever delivered; in this sweep the alarm DOES transition and notifications ARE fanned out, which uncovers a different divergence one layer down and under a different replay key (the suite therefore did not mark the check OWNED). Also related but distinct: 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md covers the TOPIC policy and the topic's KMS key policy on Publish; the cause below is about the DESTINATION queue's policy at delivery time.

Cause 1 of 1: SNS fan-out to an SQS subscriber performs no authorization against the destination queue's resource policy, so an unauthorized queue is written to.
AWS: Smithy SNS Subscribe(Protocol='sqs', Endpoint=<queue ARN>) succeeds regardless of the destination queue's policy and returns a SubscriptionArn — the subscription is accepted at creation. Authorization is evaluated at DELIVERY time, when SNS calls sqs:SendMessage on the destination queue as the sns.amazonaws.com service principal. The Smithy model says nothing about delivery-time authorization, so for the effect this is recorded behavior only: on real AWS, a queue whose policy grants sns.amazonaws.com sqs:SendMessage (conditioned on the topic ARN) receives one Notification per topic publish, while a queue with NO resource policy at all receives nothing and stays at depth 0 (ApproximateNumberOfMessages + ApproximateNumberOfMessagesNotVisible == 0) across repeated publishes; adding the grant afterwards makes the same queue start receiving, proving the queue was reachable and only the missing grant suppressed delivery. The withheld delivery produces no client-visible error on Publish (Publish still returns a MessageId).
Emulator: both queues receive. In the recorded run one SNS topic with two SQS subscriptions was published to three times by alarm actions; the granted queue received 3 notifications and the queue with NO Policy attribute at all (verified in the same run: get_queue_attributes(AttributeNames=['Policy']) returns no 'Policy' key) received 4 notification bodies of the same TopicArn. Delivery is therefore unconditional: the destination queue's policy is never consulted.
Why: a client cannot rely on an SQS queue policy as an access-control boundary. Every emulator-side test of a least-privilege notification pipeline passes vacuously — a missing or deny-shaped queue policy is indistinguishable from a correct one — so a configuration that silently drops messages in production looks healthy here, and conversely a queue that should be isolated from a topic receives its data.
Fix: the SNS delivery path for the sqs protocol — before enqueuing to the destination queue, evaluate that queue's resource policy for sqs:SendMessage by the sns.amazonaws.com service principal (including the aws:SourceArn/topic condition) and withhold the delivery when it does not allow, leaving the queue's depth unchanged while Publish still succeeds. Belongs in the SNS crate's subscription-delivery component plus the SQS queue-policy evaluation it must call; no change is needed in Subscribe, which correctly accepts the subscription.
Traces: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy.

## Evidence
Replay report (simparity-behavioral-ga-prod-v1), bucket cloudwatch__ecs__sns__sqs 0/2, 2 failed (one OWNED and skipped).

[mismatch] cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy, at `assert not stray`:
  AssertionError: the queue with no SNS grant received 4 notification(s) while the granted queue received 3: ['{"Type":"Notification","MessageId":"ec33b635-...","TopicArn":"arn:aws:sns:<region>:<account>:sim2real-xsvc-rtcneg-t-<id>","Subject":"ALARM: ..."}', ... x4]

The same run asserts, and the emulator agrees, that the unauthorized queue carries no policy at all: `pol = sqs.get_queue_attributes(QueueUrl=bad_url, AttributeNames=["Policy"]).get("Attributes", {}); assert "Policy" not in pol`. Expected (recorded on real AWS): granted queue receives each round; unauthorized queue holds 0 messages until its policy is repaired, then receives. Observed: unauthorized queue received every round.

## Steps to Reproduce
1. sqs.create_queue twice ('good', 'bad'); read each QueueArn.
2. sns.create_topic; attach to the GOOD queue only a policy granting Principal {'Service':'sns.amazonaws.com'} sqs:SendMessage on its ARN with Condition aws:SourceArn = the topic ARN. Leave the BAD queue with no policy (confirm get_queue_attributes(AttributeNames=['Policy']) returns no Policy).
3. sns.subscribe(Protocol='sqs', Endpoint=<arn>) for both queues — both succeed.
4. Publish to the topic (directly, or via a CloudWatch alarm action as in the check) three times.
5. sqs.receive_message on both queues. AWS: only the good queue has messages; the bad queue's ApproximateNumberOfMessages stays 0. Emulator: both queues receive every notification.
6. Add the same grant to the bad queue and publish again: on AWS it now receives (proving the grant was the only thing withholding delivery); the emulator shows no difference before and after.
