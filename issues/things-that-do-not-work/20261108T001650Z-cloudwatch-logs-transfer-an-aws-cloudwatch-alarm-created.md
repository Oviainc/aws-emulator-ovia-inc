Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: f42419e01630977583bc09726b6f41b0d8951344

# cloudwatch__logs__transfer: an AWS::CloudWatch::Alarm created by a CREATE_COMPLETE stack is not reported by DescribeAlarms at all

> _Note: this issue was identified against the code base at commit `f42419e01630977583bc09726b6f41b0d8951344`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: a CloudFormation stack that reaches CREATE_COMPLETE with an AWS::CloudWatch::Alarm resource and outputs its name leaves no alarm behind — DescribeAlarms does not return it under any alarm type.

AWS: Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json) — DescribeAlarms takes AlarmNames (list, max 100) and AlarmTypes and returns MetricAlarms / CompositeAlarms; its only modeled error is InvalidNextToken, so a named alarm that exists is returned and one that does not is simply absent from the lists. Recorded behavior: once CloudFormation reports CREATE_COMPLETE for a stack containing an AWS::CloudWatch::Alarm, that alarm exists on the control plane — DescribeAlarms(AlarmNames=[<the stack's alarm>]) returns it in MetricAlarms with the template's MetricName, Statistic, Threshold, ComparisonOperator and TreatMissingData, it evaluates (an alarm over a metric with no datapoints under TreatMissingData=notBreaching settles to OK and records the single history transition "Alarm updated from INSUFFICIENT_DATA to OK"), and it transitions to ALARM once the metric crosses the threshold. CREATE_COMPLETE is CloudFormation's own statement that every declared resource was created.
Emulator: the same stack reports CREATE_COMPLETE and its sibling resources are real — the declared AWS::Logs::LogGroup answers DescribeLogGroups with the declared retentionInDays and metricFilterCount 1, and the AWS::Transfer::Server output is a well-formed s-<17 hex> id — but the alarm named by the stack output is invisible: DescribeAlarms does not report it at all, so nothing about it (its configuration, its settled state, its history, its empty AlarmActions) can be read. The scenario's whole diagnosis phase stops on the first read.
Why: a stack whose status says the alarm was created, and whose alarm cannot be found, is worse than a failed create: every subsequent step a client takes (describe it, read its state, wait for it to fire, attach it to a dashboard, delete it) fails or silently reads nothing, while the deployment looks green. Any CloudFormation-deployed monitoring topology is unusable and untestable against the emulator.
Fix: the CloudFormation provisioning path for AWS::CloudWatch::Alarm — the resource handler must create the alarm in CloudWatch's alarm state (the same state DescribeAlarms/DescribeAlarmHistory serve, with the declared metric, statistic, period, threshold, comparison operator, TreatMissingData and actions) before the stack reports CREATE_COMPLETE, and the created alarm must be evaluated like one created through PutMetricAlarm.
Related open issue that does NOT carry this check: 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md describes the same family under the ### cloudwatch heading and owns other checks' replay keys; this failure is reported under ### cloudwatch__logs__transfer with its own key (phase assertion in the SFTP-alarm lifecycle scenario), so it is filed separately and should be closed together with it.
Traces: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle.

## Evidence
Behavioral suite simparity-behavioral-aws-v1. Service heading: ### cloudwatch__logs__transfer (14/19 passed, 5 failed; this is the only one not marked ALREADY FILED / OWNED).

[mismatch] cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle, check line 444:
`AssertionError: [phase 2/4 solve] alarm 'sim2real-refarch-sftp-alarm-<generated suffix>' is not reported by DescribeAlarms at all`

The phase-1 `deploy` assertions all PASSED before this, which is what makes the failure attributable:
- the stack reached CREATE_COMPLETE and produced all three outputs (ServerId, LogGroupName, AlarmName);
- the ServerId output is a well-formed server id ('s-' + 17 characters);
- DescribeLogGroups for the declared group reports the declared retentionInDays and metricFilterCount == 1, so the group and its AWS::Logs::MetricFilter were really created by the same stack.

Phase 2 then reads the alarm by the name the stack output and finds nothing. Expected (per the scenario it replays): DescribeAlarms returns a metric alarm whose MetricName/Statistic/Threshold/ComparisonOperator/TreatMissingData match the template, whose AlarmActions are empty, whose settled StateValue over a metric with no datapoints is OK with history exactly ['Alarm updated from INSUFFICIENT_DATA to OK'], and which transitions to ALARM once one matching log line makes the metric-filter metric reach the threshold.

Per-run values that are NOT requirements: the generated alarm name and its hex suffix, the server id, the log group name and the stack name. The requirement is that an alarm declared by a stack that reports CREATE_COMPLETE is returned by DescribeAlarms.

## Steps to Reproduce
1. create_stack with a template containing an AWS::Logs::LogGroup (with RetentionInDays), an AWS::Logs::MetricFilter on it, and an AWS::CloudWatch::Alarm over that filter's metric (Statistic Sum, Period 60, Threshold 1, GreaterThanOrEqualToThreshold, TreatMissingData notBreaching, no actions), with the alarm name as a stack Output.
2. Poll describe_stacks until StackStatus is CREATE_COMPLETE (observed).
3. cloudwatch.describe_alarms(AlarmNames=[<the AlarmName output>]). Expected: one entry in MetricAlarms carrying the declared configuration. Observed: both MetricAlarms and CompositeAlarms are empty — the alarm is not reported at all.
4. Control that already works: logs.describe_log_groups(logGroupNamePrefix=<the declared group>) reports the declared retentionInDays and metricFilterCount 1.
5. Re-replay to confirm the fix: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle.
