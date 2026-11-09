Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__sns__sqs.cdk-deploy-alarm-sns-sqs-ok-transition-without-ok-actions-delivers-nothing
Issue-Key: cloudformation__cloudwatch__sns__sqs.cdk-deploy-alarm-sns-sqs-ok-transition-without-ok-actions-delivers-nothing|run#18|IndexError|description = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# An AWS::CloudWatch::Alarm created by a CDK/CloudFormation deploy is not visible to DescribeAlarms

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: after a successful stack deploy that contains an AWS::CloudWatch::Alarm, DescribeAlarms(AlarmNames=[<alarm>]) returns an empty MetricAlarms list, so the alarm the stack reports as created does not exist in CloudWatch's own state.

AWS: an AWS::CloudWatch::Alarm resource is provisioned by calling PutMetricAlarm, so once the stack reaches CREATE_COMPLETE the alarm is a first-class CloudWatch alarm: DescribeAlarms(AlarmNames=[name]) returns exactly one MetricAlarms entry (with AlarmArn, StateValue INSUFFICIENT_DATA at creation, and the configured actions). Smithy: DescribeAlarms output MetricAlarms/CompositeAlarms; no modeled error for a name that exists. Recorded behavior: the alarm is describable immediately after the deploy.
Emulator: MetricAlarms is empty — the check dies on `cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]` with IndexError: list index out of range, so the deployed alarm was never registered with CloudWatch (or was registered under a different name).
Why: every alarm-driven flow authored as IaC is dead on arrival — the client cannot read the alarm's state, cannot set it, and no OK/ALARM action can ever fire, while CloudFormation reports the resource as created.
Fix: the AWS::CloudWatch::Alarm resource handler (it must create the alarm in CloudWatch state under the resolved AlarmName) and/or DescribeAlarms' name lookup.
Traces: cloudformation__cloudwatch__sns__sqs.cdk-deploy-alarm-sns-sqs-ok-transition-without-ok-actions-delivers-nothing.

## Evidence
[error] cloudformation__cloudwatch__sns__sqs.cdk-deploy-alarm-sns-sqs-ok-transition-without-ok-actions-delivers-nothing
File check_...cdk_deploy_alarm_sns_sqs_ok_transition_without_ok_actions_delivers_nothing.py, line 85, in run
  description = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]
IndexError: list index out of range

Expected: one MetricAlarms entry for the alarm the stack just created.
Actual: MetricAlarms == [].

## Steps to Reproduce
1. Deploy a stack (cdk deploy or CreateStack) containing an AWS::CloudWatch::Alarm with AlarmActions on an SNS topic and no OKActions.
2. Wait for CREATE_COMPLETE.
3. cloudwatch:DescribeAlarms(AlarmNames=[<the alarm name>]) — expect one MetricAlarms entry; the emulator returns an empty list.
