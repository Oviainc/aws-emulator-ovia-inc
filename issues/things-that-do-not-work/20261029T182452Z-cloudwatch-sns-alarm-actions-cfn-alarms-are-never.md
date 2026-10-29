Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery cloudwatch__sns.cfn-cloudwatch-alarm-wrong-region-arn-no-delivery cloudwatch__sns.cfn-deploy-cloudwatch-alarm-sns-topic cloudwatch__sns.cloudwatch-alarm-sns-kms-key-policy-missing-no-delivery-rejected cloudwatch__sns.ok-action-missing-sns-permission-rejected cloudwatch__sns.sns-topic-policy-missing-cloudwatch-source-arn-rejected cloudwatch__sns.iam-role-trusts-sns-service
Issue-Key: cloudwatch|SetAlarmState|ResourceNotFound
Issue-Key: cloudwatch__sns.cloudwatch-alarm-sns-kms-key-policy-missing-no-delivery-rejected|run#26|AssertionError|assert not matching, (
Issue-Key: cloudwatch__sns.ok-action-missing-sns-permission-rejected|run#27|AssertionError|assert not any(p.get("AlarmName") == alarm_name for p in seen), (
Issue-Key: cloudwatch__sns.sns-topic-policy-missing-cloudwatch-source-arn-rejected|run#26|AssertionError|assert not any(p.get("AlarmName") == alarm_name for p in seen)
Issue-Key: cloudwatch__sns.iam-role-trusts-sns-service|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CloudWatch→SNS alarm actions: CFN alarms are never created (SetAlarmState ResourceNotFound with an empty message), delivery ignores topic/KMS/IAM authorization, and SetTopicAttributes accepts an untrusted feedback role

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetAlarmState

## Description
This issue bundles 3 net-new root causes. All 3 must be closed before submitting.

Cause 1 of 3: a CloudFormation stack declaring AWS::CloudWatch::Alarm creates no alarm, and the subsequent SetAlarmState is refused with `ResourceNotFound` carrying an EMPTY message.
AWS: after the stack reaches CREATE_COMPLETE the alarm exists, so SetAlarmState on it succeeds. When an alarm genuinely does not exist, SetAlarmState's modelled refusal is `ResourceNotFound` (awsQueryError code `ResourceNotFound`, HTTP 404) and its message NAMES the missing alarm — an empty message is never AWS's answer.
Emulator: `botocore.errorfactory.ResourceNotFound: An error occurred (ResourceNotFound) when calling the SetAlarmState operation:` — the code is right, the message is the empty string, and the alarm the stack declared is absent.
Why: alarm-bearing IaC deploys green and monitors nothing; and the empty message leaves the operator with a 404 that does not say WHAT was not found. The open issue 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md records the missing-alarm half for service `cloudwatch`; these three cloudwatch__sns replay keys (and the empty-message defect) are not on it.
Fix: the CloudFormation provider for AWS::CloudWatch::Alarm (write through to the CloudWatch alarm store) and SetAlarmState's ResourceNotFound message construction.
Traces: cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery, cloudwatch__sns.cfn-cloudwatch-alarm-wrong-region-arn-no-delivery, cloudwatch__sns.cfn-deploy-cloudwatch-alarm-sns-topic.

Cause 2 of 3: alarm-action delivery to SNS is unauthorized — it ignores the topic policy, the topic's KMS key policy and the missing-permission case.
AWS: an alarm action only delivers if SNS authorizes it. A topic whose policy does not allow `SNS:Publish` from cloudwatch.amazonaws.com for the alarm's SourceArn receives NOTHING; a topic encrypted with a CMK whose key policy denies cloudwatch `kms:GenerateDataKey*`/`kms:Decrypt` receives NOTHING; and an OK-action pointing at a topic the alarm may not publish to delivers NOTHING. Each of these is asserted as an emptiness held over a window scaled from the positive control's measured delivery latency.
Emulator: the alarm notification JSON is delivered in all three cases — the full `{"Version":"1.0","AlarmName":...,"NewStateValue":"ALARM",...}` payload arrives on the queue subscribed to the topic that should have refused it.
Why: a customer's least-privilege topic policy and CMK key policy are silently ineffective, so the emulator reports successful alerting on a configuration that is mute in the real account — and the inverse, a genuinely broken policy, is never caught in testing.
Fix: the alarm action dispatcher must authorize the publish against the destination topic's resource policy and the topic's KMS key policy before delivering.
Traces: cloudwatch__sns.cloudwatch-alarm-sns-kms-key-policy-missing-no-delivery-rejected, cloudwatch__sns.ok-action-missing-sns-permission-rejected, cloudwatch__sns.sns-topic-policy-missing-cloudwatch-source-arn-rejected.

Cause 3 of 3: SetTopicAttributes accepts a delivery-status feedback role that does not trust sns.amazonaws.com.
AWS: `SQSSuccessFeedbackRoleArn` is validated when it is set: a role whose trust policy does not name the `sns.amazonaws.com` service principal is refused with error code `InvalidParameter` and the message `Invalid parameter: SQSSuccessFeedbackRoleArn: <roleArn> is not a valid role to allow SNS to write to Cloudwatch Logs` (fixed text around the role ARN from the request), and the attribute does not land on the topic.
Emulator: the call succeeds (HTTP 200, text/xml) and the untrusted role is stored as the topic's SQSSuccessFeedbackRoleArn. The twin role that DOES trust SNS is accepted, so the check isolates the trust statement as the only difference.
Why: delivery-status logging is configured with a role SNS cannot assume; the customer sees a configured topic and then no delivery records at all, with no error at configuration time.
Fix: SetTopicAttributes' validation of the *FeedbackRoleArn attributes — resolve the role and require a trust statement for sns.amazonaws.com.
Traces: cloudwatch__sns.iam-role-trusts-sns-service.

## Evidence
Cause 1 — cloudwatch__sns.cfn-alarm-missing-sns-topic-policy-no-delivery, cloudwatch__sns.cfn-cloudwatch-alarm-wrong-region-arn-no-delivery, cloudwatch__sns.cfn-deploy-cloudwatch-alarm-sns-topic all fail with:
  botocore.errorfactory.ResourceNotFound: An error occurred (ResourceNotFound) when calling the SetAlarmState operation:
(note the message after the colon is empty)

Cause 2 — cloudwatch__sns.cloudwatch-alarm-sns-kms-key-policy-missing-no-delivery-rejected:
  AssertionError: expected NO delivery to the topic encrypted with a key that denies cloudwatch kms:GenerateDataKey*/kms:Decrypt, but saw: [{'Version': '1.0', 'AlarmName': ..., 'NewStateValue': 'ALARM', 'AlarmActions': [...], 'Trigger': {...}}]
cloudwatch__sns.ok-action-missing-sns-permission-rejected:
  AssertionError: expected no OK-notification ... to reach the unpermissioned topic's queue within 45.0s (>= 3x the measured positive-control latency), but saw: [{'Version': '1.0', ..., 'NewStateValue': 'OK', 'OKActions': [...]}]
cloudwatch__sns.sns-topic-policy-missing-cloudwatch-source-arn-rejected:
  AssertionError (assert not any(p.get("AlarmName") == alarm_name for p in seen)) — the notification for the alarm was present on the queue

Cause 3 — cloudwatch__sns.iam-role-trusts-sns-service:
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '255', ...}}}

## Steps to Reproduce
Replay the trace_ids. The three cfn-* checks CreateStack a template with AWS::CloudWatch::Alarm + AWS::SNS::Topic, wait for CREATE_COMPLETE and then call SetAlarmState on the declared alarm. The three delivery checks create a topic (one CMK-encrypted with a deny-cloudwatch key policy, one with a policy lacking the CloudWatch SourceArn condition, one unpermissioned as an OK-action target), subscribe an SQS queue, drive the alarm with SetAlarmState and poll the queue for emptiness against a positive control. The feedback-role check creates two IAM roles with identical logs permissions but different trust policies and calls SetTopicAttributes with SQSSuccessFeedbackRoleArn for each.
