Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives
Issue-Key: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs|_receive#20|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event|_receive#26|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives|_receive#20|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# RDS publishes no DB instance events to the default EventBridge bus, so no aws.rds event is ever routed to a target

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RebootDBInstance

## Description
One cause, surfaced by three checks. AWS: an RDS state-machine transition publishes a DB Instance Event on the account's DEFAULT EventBridge bus. RebootDBInstance emits an event with source 'aws.rds', detail-type 'RDS DB Instance Event', resources [the instance ARN], and a detail carrying EventID 'RDS-EVENT-0006', Message 'DB instance restarted', EventCategories including 'availability', SourceType 'DB_INSTANCE', SourceIdentifier and SourceArn of the instance, plus Date. A rule on the default bus whose pattern matches that event delivers it to its targets (raw, or rendered through an InputTransformer), and the same event is readable from the RDS control plane through DescribeEvents (SourceType 'db-instance') with the same Date. Recorded behavior only for the EventID/Message literals, which the checks pin from real AWS.

Emulator: after CreateDBInstance reaches 'available' and RebootDBInstance is accepted, nothing is ever published: the subscribed SQS queues receive no message, so all three checks sit in their delivery loop until the harness's wall clock expires ('Check timed out after 60.0s'). The emulator materializes its own creating->available transition in 300ms (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs STATUS_TRANSITION_DELAY), so the 60s budget is ample and the timeout is an absence of events, not slow provisioning. Because nothing is ever delivered, the differential check cannot even establish its positive control, so the near-miss (absence) assertions are vacuous too.

Why: every event-driven RDS integration is dead — an EventBridge rule, an InputTransformer target, an SQS/Lambda/SNS consumer of aws.rds events, and anything that reads the same event stream from DescribeEvents. A client observes an instance change state through the API but receives no notification of it, which is exactly the signal automation is built on.

Fix: the RDS state-transition path (RebootDBInstance, and the create/modify/delete transitions that emit the other RDS-EVENT-xxxx events) must publish an aws.rds / 'RDS DB Instance Event' envelope onto the default EventBridge bus of the request's account and region, with the detail members listed above, and must record the same event in the store DescribeEvents projects.

Traces: events__rds__sqs.xsvc-rds-1-reboot-emits-db-instance-restarted-event-to-sqs, events__rds__sqs.xsvc-rds-1-input-transformer-renders-db-and-message-from-rds-event, events__rds__sqs.xsvc-rds-1-near-miss-rules-not-delivered-after-matching-rule-receives.

## Evidence
All three checks: verdict 'mismatch', 'Check timed out after 60.0s'. Each builds a rule on the default bus with pattern {'source': ['aws.rds'], 'detail': {'SourceArn': [<db arn>]}} (plus detail-type 'RDS DB Instance Event' and detail.EventID 'RDS-EVENT-0006' in two of them), targets an SQS queue whose policy allows events.amazonaws.com, waits for the instance to become available, calls RebootDBInstance, and then polls ReceiveMessage for up to DELIVERY_BUDGET_S=600 for a detail.EventID == 'RDS-EVENT-0006' dated at/after the reboot. No message ever arrives, so the loop never exits and the harness kills the check. The assertion the checks would have reached is: 'no RDS-EVENT-0006 dated after the reboot reached the queue within 600s; received []'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. sqs: CreateQueue and set a queue policy allowing events.amazonaws.com sqs:SendMessage.
3. events: PutRule on the default bus with EventPattern {"source":["aws.rds"],"detail":{"SourceArn":["arn:aws:rds:<region>:<acct>:db:<db-id>"]}} and PutTargets pointing at that queue.
4. rds: CreateDBInstance <db-id>, wait for DBInstanceStatus 'available', then Action=RebootDBInstance.
5. Poll sqs ReceiveMessage: no message is ever delivered. Also call Action=DescribeEvents with SourceIdentifier=<db-id>, SourceType=db-instance: the restart event is not listed.
