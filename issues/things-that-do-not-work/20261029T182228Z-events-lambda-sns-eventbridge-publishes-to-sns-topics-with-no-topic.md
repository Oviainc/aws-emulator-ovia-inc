Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published events__lambda__sns.iam-role-wrong-principal-sns-trust-rejected events__lambda__sns.cfn-deploy-eventbridge-fanout-lambda-sns events__lambda__sns.cfn-deploy-eventbridge-lambda-target-missing-permission events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission
Issue-Key: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published|run#16|AssertionError|assert messages == [], f"message was published without a topic policy grant: {messages!r}"
Issue-Key: events__lambda__sns.iam-role-wrong-principal-sns-trust-rejected|_expect_invalid_role_rejection#19|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sns.cfn-deploy-eventbridge-fanout-lambda-sns|_collect_messages#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Key: events__lambda__sns.cfn-deploy-eventbridge-lambda-target-missing-permission|_collect_messages#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission|_collect_messages#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# EventBridge publishes to SNS topics with no topic-policy grant, SetTopicAttributes accepts a feedback role that does not trust sns.amazonaws.com, and the EventBridge→Lambda/SNS paths are inert (3 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTopicAttributes

## Description
Three root causes. 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md and 20261029T065537Z-sns-delivery-status-logging-is-inert-for-the.md own other checks in this service; the checks below were not bound to them.

Cause 1 of 3: an EventBridge SNS target publishes to a topic whose resource policy does not grant the service principal.
AWS: EventBridge calls SNS:Publish on an SNS target as the service principal events.amazonaws.com. A topic whose policy has no Allow statement for that principal refuses the call: no subscriber receives anything, and the invocation is dead-lettered / counted as a FailedInvocation.
Emulator: the subscriber receives the notification anyway, carrying the full EventBridge envelope as Message. "AssertionError: message was published without a topic policy grant: [{'Type': 'Notification', ..., 'Message': '{\"version\":\"0\",...}'}]" — the requirement is that no notification is produced; the MessageId, TopicArn and timestamps in the recording are per-run values.
Why: the emulator fabricates a publish AWS refuses, so a missing topic policy is never caught and a least-privilege rollout that breaks fan-out in AWS passes locally.
Fix: resource-policy authorization in the EventBridge SNS-target invoker and in the SNS Publish path it calls.
Traces: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published

Cause 2 of 3: SetTopicAttributes accepts a delivery-status feedback role whose trust policy does not name sns.amazonaws.com.
AWS: setting a *FeedbackRoleArn attribute (e.g. SQSSuccessFeedbackRoleArn / LambdaFailureFeedbackRoleArn) with a role that cannot be assumed by sns.amazonaws.com is refused with HTTP 400 InvalidParameterException, message of the fixed form "Invalid parameter: Unable to assume the role <role ARN>" — the role ARN comes from the request.
Emulator: the call succeeds. "AssertionError: SetTopicAttributes accepted a feedback role whose trust policy does not name sns.amazonaws.com -- the wrongly-trusted role was not rejected".
Why: delivery-status logging is configured with a role SNS can never assume; the caller gets success and then silently no logs.
Fix: trust-policy validation in the SNS SetTopicAttributes handler for the feedback-role attributes.
Traces: events__lambda__sns.iam-role-wrong-principal-sns-trust-rejected

Cause 3 of 3: EventBridge rules deployed by CloudFormation/CDK never invoke their Lambda or SNS targets, and SNS→Lambda subscriptions never deliver.
AWS: after the stack reaches CREATE_COMPLETE, a matching PutEvents invokes the Lambda and publishes to the SNS topic; an SNS topic with a Lambda subscription invokes the function on Publish; a subscription whose function lacks the sns.amazonaws.com invoke grant does not.
Emulator: nothing is ever invoked; the checks end "Check timed out after 60.0s" and the missing-permission negatives are ungraded.
Why: both IaC-deployed EventBridge fan-out and SNS→Lambda are unusable, and a permission regression cannot be distinguished from the inert path.
Fix: the EventBridge Lambda- and SNS-target invokers, and the SNS Lambda-subscription dispatcher (including its invoke-permission check).
Traces: events__lambda__sns.cfn-deploy-eventbridge-fanout-lambda-sns, events__lambda__sns.cfn-deploy-eventbridge-lambda-target-missing-permission, events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission

## Evidence
Cause 1: "AssertionError: message was published without a topic policy grant: [{'Type': 'Notification', 'MessageId': ..., 'TopicArn': 'arn:aws:sns:...', 'Message': '{\"version\":\"0\",\"id\":...}'}]" — expected no notification at all plus a failed/dead-lettered invocation.
Cause 2: "AssertionError: SetTopicAttributes accepted a feedback role whose trust policy does not name sns.amazonaws.com -- the wrongly-trusted role was not rejected" — expected HTTP 400 InvalidParameterException "Invalid parameter: Unable to assume the role <role ARN>".
Cause 3: "Error: Check timed out after 60.0s" on all three stack-deployed checks.

## Steps to Reproduce
(1) CreateTopic with no policy statement for events.amazonaws.com, subscribe SQS, PutRule + PutTargets with the topic ARN, PutEvents a matching entry — the SQS subscriber receives the notification. (2) SetTopicAttributes with SQSSuccessFeedbackRoleArn set to a role whose trust policy names ec2.amazonaws.com — accepted. (3) Deploy an EventBridge rule with Lambda and SNS targets by CloudFormation/CDK and PutEvents — nothing is invoked.
