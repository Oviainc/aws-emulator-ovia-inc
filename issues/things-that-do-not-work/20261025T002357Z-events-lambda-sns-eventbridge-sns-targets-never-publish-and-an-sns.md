Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.eventbridge-rule-target-sns-arn-publish events__lambda__sns.events-rule-notification-sns-destination-delivery events__lambda__sns.iam-role-trusts-sns-service-assumed events__lambda__sns.cdk-deploy-sns-lambda-subscription-notification
Issue-Key: events__lambda__sns.eventbridge-rule-target-sns-arn-publish|_wait_for_matching_message#11|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sns.events-rule-notification-sns-destination-delivery|_poll_for_marker#27|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: events__lambda__sns.iam-role-trusts-sns-service-assumed|_poll_for_log_events#22|Timeout|time.sleep(6)
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-notification|_collect_messages#10|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge SNS targets never publish and an SNS Lambda subscription never delivers, so no notification pipeline produces an effect (2 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 2 — a rule whose target is an SNS topic never publishes to it.
AWS: PutTargets with `Arn: arn:aws:sns:<region>:<acct>:<topic>` makes EventBridge publish the event to that topic when the rule matches, provided the topic policy admits events.amazonaws.com; subscribers of the topic then receive the message. The same holds for a rule configured as a notification destination.
Emulator: nothing is published. Each check completes PutRule, the topic policy update and PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0), then polls the topic's subscriber; the message never arrives and the replay cuts the check off at 60 s. The companion check that an IAM role trusting sns.amazonaws.com is actually assumed for delivery is blocked at the same point, because there is no delivery to attribute to a role.
Why: EventBridge -> SNS fan-out is a primary alerting path. Every call reports success while no notification is ever sent, so a client has no signal that its alerts are being dropped.
Fix: the SNS target of the EventBridge target dispatcher (services/service-com-amazonaws-eventbridge-awsevents) and the SNS Publish path it calls, including the topic-policy check and the delivery role it assumes.
Traces: events__lambda__sns.eventbridge-rule-target-sns-arn-publish, events__lambda__sns.events-rule-notification-sns-destination-delivery, events__lambda__sns.iam-role-trusts-sns-service-assumed.

Cause 2 of 2 — an SNS subscription whose Protocol is `lambda` never invokes the function.
AWS: `Subscribe(Protocol='lambda', Endpoint=<function arn>)` plus a lambda:InvokeFunction permission for sns.amazonaws.com makes each Publish invoke the function with the SNS Records envelope. A CDK/CloudFormation-deployed topic + function + subscription behaves the same way.
Emulator: the function is never invoked, so the deployed notification pipeline produces no effect and the check is cut off at 60 s.
Why: topic-to-function fan-out — the way most SNS consumers are written — does nothing, and the stack that declares it still reports healthy.
Fix: the SNS subscription delivery path for the `lambda` protocol and the asynchronous invocation it issues. Related: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the Invoke API stub and does not carry this check's replay key or the SNS-side delivery.
Traces: events__lambda__sns.cdk-deploy-sns-lambda-subscription-notification.

## Evidence
All four checks report "Check timed out after 60.0s". In each, every control-plane call first returned 2xx: CreateTopic, SetTopicAttributes (policy admitting events.amazonaws.com), Subscribe, lambda AddPermission, PutRule, PutTargets (FailedEntryCount 0), and PutEvents (FailedEntryCount 0 with an EventId) or Publish. The terminal read — the subscriber's queue/function effect — never appears. Expected: the event published to the topic and delivered to its subscriber (and, for the lambda protocol, the function invoked with the SNS Records envelope) inside the check's delivery budget.

## Steps to Reproduce
Cause 1:
1. CreateTopic(t); SetTopicAttributes(t, Policy allowing events.amazonaws.com sns:Publish); Subscribe(t, Protocol='sqs', Endpoint=<queue arn>).
2. PutRule(Name=r, EventPattern='{"source":["x"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<topic arn>}]).
3. PutEvents a matching event -> the subscribed queue never receives a message.

Cause 2:
1. CreateTopic(t2); CreateFunction(fn); Subscribe(t2, Protocol='lambda', Endpoint=<fn arn>); lambda AddPermission(fn, Principal='sns.amazonaws.com').
2. Publish(TopicArn=t2, Message='m') -> the function is never invoked. Deploying the same three resources from a CDK/CFN stack behaves identically.
