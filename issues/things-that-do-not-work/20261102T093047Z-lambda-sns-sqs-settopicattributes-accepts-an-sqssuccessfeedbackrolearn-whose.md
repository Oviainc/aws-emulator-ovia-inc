Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.iam-role-trust-sns-service-assume
Issue-Key: lambda__sns__sqs.iam-role-trust-sns-service-assume|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# lambda__sns__sqs: SetTopicAttributes accepts an SQSSuccessFeedbackRoleArn whose trust policy does not let SNS assume it for THIS topic, instead of refusing with InvalidParameter

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTopicAttributes

## Description
ONE net-new root cause of service `lambda__sns__sqs`, grounded in the single failing check in trace_ids. The check was not marked ALREADY FILED or OWNED by the replay report, and none of this service's open issues carries it (20261025T022656Z-lambda-sns-sqs-sns-sqs-to-lambda-delivery.md, 20261025T064350Z-lambda-sns-sqs-a-cfn-deployed-sns-deliverystatuslogging.md, 20261029T183635Z-lambda-sns-sqs-sqs-event-source-mappings-never.md and 20261030T113113Z-lambda-sns-sqs-createeventsourcemapping-s-sqs-refusal-messages.md all describe different surfaces). The same validation gap is recorded for other service tokens in 20261029T182452Z-cloudwatch-sns-alarm-actions-cfn-alarms-are-never.md (its Cause 3) and 20261029T182228Z-events-lambda-sns-eventbridge-publishes-to-sns-topics-with-no-topic.md; those name the simple "role does not trust sns.amazonaws.com at all" form, while the case here is the CONDITION on an otherwise-trusting role, and they do not carry this check's id. They should be closed together.

=== Cause 1 of 1: the feedback role's trust policy is not evaluated for the topic being configured ===
AWS: Smithy (sns) SetTopicAttributes declares InvalidParameterException (errorKind client, httpError 400, awsQueryError code `InvalidParameter`). Setting `SQSSuccessFeedbackRoleArn` validates the role at configuration time by checking that SNS can actually assume it FOR THIS TOPIC: the role's trust policy must allow `sts:AssumeRole` to the `sns.amazonaws.com` service principal and its conditions must be satisfied for the topic in the request. A role whose trust statement is conditioned on `ArnEquals aws:SourceArn = <topic A>` and `StringEquals aws:SourceAccount = <owner>` is accepted on topic A and REFUSED on a sibling topic B with error code `InvalidParameter` and the message `Invalid parameter: SQSSuccessFeedbackRoleArn: <roleArn> is not a valid role to allow SNS to write to Cloudwatch Logs` — the fixed text is `Invalid parameter: `, the attribute name, `: ` and ` is not a valid role to allow SNS to write to Cloudwatch Logs`, and the only variable part is the role ARN taken from the request. The refused value does not land: GetTopicAttributes on topic B reports no SQSSuccessFeedbackRoleArn, while delivery on B itself is unaffected and its messages still reach the subscribed queue. Recorded behavior for the validation's existence (the model declares only the error channel).
Emulator: the call SUCCEEDS — HTTP 200, `content-type: text/xml` — and the role is stored as the sibling topic's SQSSuccessFeedbackRoleArn. The condition keys on the trust policy are never evaluated; the attribute is treated as an opaque string once its ARN parses.
Why: the whole point of scoping a feedback role with `aws:SourceArn`/`aws:SourceAccount` is confused-deputy protection, and that scoping is silently ineffective here. A client configures delivery-status logging with a role SNS cannot assume for that topic, is told it succeeded, and then sees no delivery records at all with no error to explain the silence; conversely a least-privilege trust policy a client deliberately scoped to one topic appears to work on every topic, so the over-broad configuration a real account refuses ships undetected.
Fix: the SetTopicAttributes validation path for the *FeedbackRoleArn attributes — resolve the role, evaluate its AssumeRolePolicyDocument for the sns.amazonaws.com service principal INCLUDING its condition keys against the topic in the request (aws:SourceArn = the topic ARN, aws:SourceAccount = the topic owner), and on failure refuse with InvalidParameter and the message above, storing nothing.
Traces: lambda__sns__sqs.iam-role-trust-sns-service-assume.

## Evidence
lambda__sns__sqs.iam-role-trust-sns-service-assume [mismatch]:
  File "check_lambda__sns__sqs_iam_role_trust_sns_service_assume.py", line 52, in run
    error = _error(lambda: sns.set_topic_attributes(
  AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'RequestId': '…', 'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'text/xml', 'content-length': '255', …}, 'RetryAttempts': 0}}

The check creates ONE role whose trust statement is {"Effect":"Allow","Principal":{"Service":"sns.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"ArnEquals":{"aws:SourceArn":"<topic A>"},"StringEquals":{"aws:SourceAccount":"<account>"}}} and two wired topics.
  expected: SetTopicAttributes(TopicArn=<topic A>, AttributeName='SQSSuccessFeedbackRoleArn', AttributeValue=<role>) succeeds and reads back; the SAME call on sibling topic B is refused with Code 'InvalidParameter' and Message 'Invalid parameter: SQSSuccessFeedbackRoleArn: <role arn> is not a valid role to allow SNS to write to Cloudwatch Logs', B's GetTopicAttributes reports no SQSSuccessFeedbackRoleArn, A's delivery writes exactly one SUCCESS delivery record for its published MessageId, and B writes none while still delivering to its queue.
  actual: the call on sibling topic B returned HTTP 200 and the role was accepted — the condition on the trust policy was never consulted; the downstream delivery-record assertions were therefore never reached.

## Steps to Reproduce
1. iam.CreateRole with an AssumeRolePolicyDocument allowing sts:AssumeRole to Principal {"Service":"sns.amazonaws.com"} with Condition {"ArnEquals":{"aws:SourceArn":"<topic A arn>"},"StringEquals":{"aws:SourceAccount":"<account id>"}}, plus an inline policy granting logs:CreateLogGroup/CreateLogStream/PutLogEvents on *.
2. sns.CreateTopic twice (topic A and sibling topic B), each with an SQS queue subscribed and a queue policy allowing sns.amazonaws.com.
3. sns.SetTopicAttributes(TopicArn=<topic A>, AttributeName='SQSSuccessFeedbackRoleArn', AttributeValue=<role arn>) — succeeds; GetTopicAttributes on A reports the role.
4. sns.SetTopicAttributes(TopicArn=<topic B>, AttributeName='SQSSuccessFeedbackRoleArn', AttributeValue=<the same role arn>) — against AWS this is refused 400 InvalidParameter with 'Invalid parameter: SQSSuccessFeedbackRoleArn: <role arn> is not a valid role to allow SNS to write to Cloudwatch Logs' and B keeps no SQSSuccessFeedbackRoleArn; against the emulator it returns HTTP 200 and stores the role.
5. Re-replay lambda__sns__sqs.iam-role-trust-sns-service-assume to confirm the fix.
