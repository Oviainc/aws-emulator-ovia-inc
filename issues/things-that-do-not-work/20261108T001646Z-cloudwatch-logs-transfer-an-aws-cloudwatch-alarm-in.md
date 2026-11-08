Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle
Issue-Key: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 3041302e43fa25593bf110b82f55871801d4259f

# cloudwatch__logs__transfer: an AWS::CloudWatch::Alarm in a CREATE_COMPLETE stack is invisible to DescribeAlarms, so the SFTP-alarm diagnosis never gets past its first read (1 cause)

> _Note: this issue was identified against the code base at commit `3041302e43fa25593bf110b82f55871801d4259f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: a CloudFormation-declared metric alarm is never materialised in the CloudWatch alarm store, so every later read of it answers as if the alarm does not exist.

AWS: recorded behavior only for the provisioning half (CloudFormation resource provisioning is not expressible in the CloudWatch Smithy model). A stack that declares an `AWS::CloudWatch::Alarm` and reaches CREATE_COMPLETE has created that alarm in CloudWatch, and `Ref` on that resource publishes the alarm's name; reading it back with that name must find it. Smithy (cloudwatch, DescribeAlarms): input `AlarmNames` (list, length 0-100) selects alarms by exact name and output carries `MetricAlarms`/`CompositeAlarms`/`LogAlarms` plus `NextToken`; the only modelled error is `InvalidNextToken` (400). So the correct answer for a name that exists is a 200 whose `MetricAlarms` holds exactly one entry for that name, carrying the template's `MetricName`, `Namespace`, `Statistic`, `Period`, `EvaluationPeriods`, `Threshold`, `ComparisonOperator` and `TreatMissingData`, and a readable `StateValue` (INSUFFICIENT_DATA until the alarm's first evaluation, then OK under TreatMissingData=notBreaching with no datapoints).

Emulator: the stack deploys to CREATE_COMPLETE and publishes the alarm name as a stack Output (the deploy phase of the check passes in full, including the declared log group's retention and its metricFilterCount of 1, so the Logs resources in the SAME template are provisioned correctly). Immediately afterwards, `DescribeAlarms(AlarmNames=[<that published AlarmName>])`, paginated to the end, returns a 200 whose `MetricAlarms` contains no entry with that name at all — the check fails in phase 2/4 with `alarm '<alarm-name>' is not reported by DescribeAlarms at all`. The alarm resource was therefore reported created while nothing was written to the store `PutMetricAlarm`/`DescribeAlarms` share.

Why: a client cannot read, state-check, mute, tag or act on an alarm its own template just created; a CREATE_COMPLETE status stops being a usable readiness signal, because the resources it claims exist cannot be found by the identifiers the stack itself published. Every deploy-then-observe monitoring flow (settle to OK, feed the metric, expect ALARM, inspect DescribeAlarmHistory) is cut off at its first call, and the absence is indistinguishable from an alarm that simply has no data.

Fix: the CloudFormation provisioning path for the `AWS::CloudWatch::Alarm` resource type — it must create the alarm in the CloudWatch service state (the same state PutMetricAlarm writes and DescribeAlarms reads) before reporting the resource CREATE_COMPLETE, with the template's properties carried through unchanged. CloudWatch's DescribeAlarms handler and its name filter are not themselves implicated: the store has no such alarm to return.

Related open issues that do NOT carry this check's replay key and therefore do not cover it; merge with them if the same provisioning path is the cause: 20261102T120041Z-cloudwatch-dynamodb-lambda-an-aws-cloudwatch-alarm-declared.md and 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md. The other failures under this suite heading are owned by 20261102T090835Z-cloudwatch-logs-transfer-transfer-user-mapping-validation-cloudwatch-alarm-and-logs.md, which does not carry this key either.

Traces: cloudwatch__logs__transfer.refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle (phase 2/4 'solve').

## Evidence
Replayed against the emulator (suite service heading `### cloudwatch__logs__transfer`), check refarch-sftp-e2e-diagnose-sftp-alarm-not-firing-lifecycle:

  File "check_cloudwatch__logs__transfer_refarch_sftp_e2e_diagnose_sftp_alarm_not_firing_lifecycle.py", line 444, in phase
    raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
  AssertionError: [phase 2/4 solve] alarm 'sim2real-refarch-sftp-alarm-<suffix>' is not reported by DescribeAlarms at all

(The alarm name is tokenised per run by the check; only the `sim2real-refarch-sftp-alarm-` prefix is fixed, the suffix is a per-run value and not part of the requirement.)

The template deployed in phase 1 declares, among its resources:
  "SftpErrorAlarm": {"Type": "AWS::CloudWatch::Alarm", "Properties": {"AlarmName": {"Ref": "AlarmName"}, "MetricName": "SftpErrors", "Namespace": {"Ref": "MetricNamespace"}, "Statistic": "Sum", "Period": 60, "EvaluationPeriods": 1, "Threshold": 1, "ComparisonOperator": "GreaterThanOrEqualToThreshold", "TreatMissingData": "notBreaching"}}
and exports it as the stack Output `AlarmName` = {"Ref": "SftpErrorAlarm"}.

Expected: after CREATE_COMPLETE, DescribeAlarms(AlarmNames=[<Output AlarmName>]) -> HTTP 200 with exactly one MetricAlarms entry whose AlarmName equals that value, MetricName "SftpErrors", Statistic "Sum", Threshold 1.0, ComparisonOperator "GreaterThanOrEqualToThreshold", TreatMissingData "notBreaching", and a readable StateValue.
Actual: HTTP 200, MetricAlarms (across all pages) contains no entry with that AlarmName — the alarm does not exist, even though the same stack's AWS::Logs::LogGroup and AWS::Logs::MetricFilter resources were created and are readable (retentionInDays 30, metricFilterCount 1).

## Steps to Reproduce
1. CreateStack a template containing an AWS::IAM::Role (Transfer logging role), an AWS::Logs::LogGroup (RetentionInDays 30), an AWS::Logs::MetricFilter (FilterPattern "ERROR", metric SftpErrors/<namespace>, MetricValue 1), an AWS::Transfer::Server (PUBLIC/SFTP/SERVICE_MANAGED with that LoggingRole) and an AWS::CloudWatch::Alarm named by a parameter (MetricName SftpErrors, Namespace <namespace>, Statistic Sum, Period 60, EvaluationPeriods 1, Threshold 1, ComparisonOperator GreaterThanOrEqualToThreshold, TreatMissingData notBreaching), with Outputs exporting Ref of the alarm and of the log group.
2. Poll DescribeStacks until the stack reaches CREATE_COMPLETE, then read the Outputs.
3. Confirm the Logs side is real: DescribeLogGroups(logGroupNamePrefix=<Output LogGroupName>) reports the group with retentionInDays 30 and metricFilterCount 1.
4. Call cloudwatch.DescribeAlarms(AlarmNames=[<Output AlarmName>]) and page to the end.
5. Expect one MetricAlarms entry carrying the template's alarm configuration and a readable StateValue; observe no entry for that name at all.
