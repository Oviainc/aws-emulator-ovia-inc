Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative
Issue-Key: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative|run#17|IndexError|state = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]["StateValue"]
Issue-Commit: b8358f56f4f44e240ca755ea7919bfa62f43cb3d

# cloudwatch__dynamodb__lambda: an AWS::CloudWatch::Alarm declared in a CREATE_COMPLETE stack does not exist afterwards, so DescribeAlarms answers an empty list (1 cause)

> _Note: this issue was identified against the code base at commit `b8358f56f4f44e240ca755ea7919bfa62f43cb3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: a CloudFormation-declared metric alarm is never materialised in CloudWatch.
AWS: recorded behavior only (CloudFormation resource provisioning is not expressible in the CloudWatch Smithy model). A stack that declares an `AWS::CloudWatch::Alarm` and reaches CREATE_COMPLETE has created that alarm in CloudWatch: `DescribeAlarms(AlarmNames=[<the declared AlarmName>])` returns exactly one MetricAlarm, whose StateValue is readable (INSUFFICIENT_DATA until its series receives data). Smithy: DescribeAlarms returns MetricAlarms/CompositeAlarms lists and answers no error for a name that exists.
Emulator: after the stack deploys, `describe_alarms(AlarmNames=[<alarm>])['MetricAlarms']` is `[]` — indexing element 0 raises IndexError — so the alarm resource was reported created but does not exist in the CloudWatch store.
Why: every deploy-then-observe flow breaks: a client cannot read the state of an alarm its own template just created, cannot set or disable its actions, and cannot tell a missing alarm from an alarm that has no data. A stack that claims CREATE_COMPLETE while its alarm is absent makes the stack status unusable as a readiness signal.
Fix: the CloudFormation provisioning path for the AWS::CloudWatch::Alarm resource type — it must create the alarm in the CloudWatch service state (the same store PutMetricAlarm writes) before reporting the resource created.
Traces: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative.

Related open issue that does NOT carry this replay key and therefore does not cover this check; merge with it: 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md.

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch__dynamodb__lambda`), check deploy-cloudwatch-composite-alarm-missing-metric-negative:

  File "check_cloudwatch__dynamodb__lambda_deploy_cloudwatch_composite_alarm_missing_metric_negative.py", line 154, in run
    state = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]["StateValue"]
  IndexError: list index out of range

Expected: one MetricAlarm entry for the alarm the stack declared, with a readable StateValue (INSUFFICIENT_DATA for a series with no data). Actual: MetricAlarms is an empty list.

## Steps to Reproduce
1. Deploy a CloudFormation stack that declares an AWS::CloudWatch::Alarm (alongside an IAM role and a Lambda function) on a custom namespace/metric, and wait for CREATE_COMPLETE.
2. Call cloudwatch.describe_alarms(AlarmNames=[<the template's AlarmName>]).
3. Expect one MetricAlarm whose StateValue is readable; observe MetricAlarms == [].
