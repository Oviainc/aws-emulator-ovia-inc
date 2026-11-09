Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution
Issue-Key: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution|run#20|AssertionError|assert rolling == [f"The following CloudWatch Alarms were in ALARM state: {alarm}"], rolling
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudformation__cloudwatch: the rollback-trigger stack event reason is emulator-authored — "The following rollback trigger(s) went to ALARM state: [<alarm ARN>]. " instead of AWS's "The following CloudWatch Alarms were in ALARM state: <alarm name>"

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: when a change-set execution is rolled back by a RollbackConfiguration trigger, the UPDATE_ROLLBACK_IN_PROGRESS stack event carries a ResourceStatusReason the emulator authors itself.
AWS: a change set created with RollbackConfiguration={RollbackTriggers:[{Arn:<alarm ARN>, Type:"AWS::CloudWatch::Alarm"}], MonitoringTimeInMinutes:n} and executed while that alarm is already in ALARM aborts the update: the stack reaches UPDATE_ROLLBACK_COMPLETE and the single UPDATE_ROLLBACK_IN_PROGRESS event names the offending alarms as "The following CloudWatch Alarms were in ALARM state: <alarm names>". The fixed text is "The following CloudWatch Alarms were in ALARM state: "; the only variable part is the alarm NAME (as created by cloudwatch:PutMetricAlarm and named in the trigger), not its ARN, with no surrounding brackets and no trailing period or space. (Message: recorded behavior; the model does not carry stack-event reason strings. Reference: AWS CloudFormation rollback-triggers documentation.)
Emulator: the rollback itself happens (the stack does reach UPDATE_ROLLBACK_COMPLETE), but the event reason reads "The following rollback trigger(s) went to ALARM state: [<alarm ARN>]. " — different fixed text, the ARN in place of the name, square brackets around it, and a trailing ". ".
Why: the stack-event reason is the only machine-readable record of WHY an execution was rolled back. A client (or an operator's log scraper) that matches AWS's sentence, or that extracts the alarm name from it to re-check the alarm, finds neither the sentence nor a name — it gets an ARN inside brackets from a sentence that does not exist in AWS.
Fix: the CloudFormation rollback-trigger evaluation path that writes the UPDATE_ROLLBACK_IN_PROGRESS stack event — render AWS's sentence with the triggers' alarm names.
Traces: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution. Open issue 20261102T101202Z-cloudformation-cloudwatch-a-rollbackconfiguration-alarm-in-alarm-does.md reports that the alarm did not roll the execution back at all; the rollback now happens, this check fails only on the reason wording, and that issue does not carry this check, so it does not cover this cause.

## Evidence
From the behavioral replay — cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution [mismatch]:
  assert rolling == [f"The following CloudWatch Alarms were in ALARM state: {alarm}"], rolling
  AssertionError: ['The following rollback trigger(s) went to ALARM state: [arn:aws:cloudwatch:us-west-2:000000000000:alarm:sim2real-xsvc-csrbcfg-39aaf5a58689]. ']

Expected (AWS): exactly one UPDATE_ROLLBACK_IN_PROGRESS event whose ResourceStatusReason is "The following CloudWatch Alarms were in ALARM state: <alarm name>" (fixed text + the alarm name from the request).
Actual (emulator): "The following rollback trigger(s) went to ALARM state: [<alarm ARN>]. " — emulator-authored wording, ARN instead of name, brackets, trailing ". ".
The preceding assertions passed, so the echo of RollbackConfiguration on the change set and the UPDATE_ROLLBACK_COMPLETE status are correct; only the reason diverges.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution. Manually: cloudwatch:PutMetricAlarm(AlarmName=A, TreatMissingData=breaching, ...) then SetAlarmState(A, ALARM); CreateStack with one AWS::CloudFormation::WaitConditionHandle and wait for CREATE_COMPLETE; CreateChangeSet(ChangeSetType=UPDATE) adding a second handle with RollbackConfiguration={RollbackTriggers:[{Arn:<A's ARN>,Type:"AWS::CloudWatch::Alarm"}],MonitoringTimeInMinutes:1}; ExecuteChangeSet; then DescribeStackEvents and read the ResourceStatusReason of the UPDATE_ROLLBACK_IN_PROGRESS event.
