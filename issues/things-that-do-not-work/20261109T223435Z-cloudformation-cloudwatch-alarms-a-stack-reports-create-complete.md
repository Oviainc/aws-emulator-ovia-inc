Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm cloudformation__cloudwatch.cdk-deploy-composite-alarm-or-rule-reflects-either-child-alarm cloudformation__cloudwatch.cfn-cloudwatch-alarm-nested-required-members-rejected
Issue-Key: cloudformation__cloudwatch.cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm|run#17|IndexError|alarm1_arn = cw.describe_alarms(AlarmNames=[alarm1])["MetricAlarms"][0]["AlarmArn"]
Issue-Key: cloudformation__cloudwatch.cdk-deploy-composite-alarm-or-rule-reflects-either-child-alarm|run#17|IndexError|alarm1_arn = cw.describe_alarms(AlarmNames=[alarm1])["MetricAlarms"][0]["AlarmArn"]
Issue-Key: cloudformation__cloudwatch.cfn-cloudwatch-alarm-nested-required-members-rejected|run#11|AssertionError|assert [a["AlarmName"] for a in alarms] == [good_name], (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__cloudwatch: alarms a stack reports CREATE_COMPLETE are not created in CloudWatch — DescribeAlarms returns no MetricAlarms for them

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: a CloudFormation/CDK stack that declares AWS::CloudWatch::Alarm (and AWS::CloudWatch::CompositeAlarm over those alarms) reaches CREATE_COMPLETE, but the alarms do not exist in CloudWatch.
AWS: after the stack completes, DescribeAlarms(AlarmNames=[<name the stack created>]) returns that alarm in MetricAlarms (composite alarms come back in CompositeAlarms), each carrying AlarmArn; a composite alarm's AlarmRule can then reference the children's ARNs.
Emulator: DescribeAlarms(AlarmNames=[...]) returns an empty MetricAlarms list for the alarm names the stack minted, and a control alarm declared directly in a template also reads back as [].
Why: a client that provisions alarms with CloudFormation and then reads or wires them through the CloudWatch API finds nothing; composite alarms cannot be built at all because their children have no ARNs.
Fix: the AWS::CloudWatch::Alarm and AWS::CloudWatch::CompositeAlarm resource handlers must create the alarm in CloudWatch state (PutMetricAlarm / PutCompositeAlarm) rather than only minting a physical id.
Traces: the three failing checks below.

## Evidence
cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm and cdk-deploy-composite-alarm-or-rule-reflects-either-child-alarm both die on `cw.describe_alarms(AlarmNames=[alarm1])["MetricAlarms"][0]["AlarmArn"]` with IndexError: list index out of range (the list is empty). cfn-cloudwatch-alarm-nested-required-members-rejected fails with AssertionError: 'control alarm did not read back, got []'. Expected: the alarm named in the template present in MetricAlarms with its AlarmArn.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Deploy a stack (or `cdk deploy`) declaring two AWS::CloudWatch::Alarm resources and an AWS::CloudWatch::CompositeAlarm whose AlarmRule references them.
3. After CREATE_COMPLETE, call DescribeAlarms(AlarmNames=[<child alarm name>]) and observe MetricAlarms == [].
