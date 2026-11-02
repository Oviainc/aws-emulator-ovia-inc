Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution
Issue-Key: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution|run#17|AssertionError|assert stack["StackStatus"] == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/cloudwatch: a RollbackConfiguration alarm in ALARM does not roll back the change-set execution

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: the stack's RollbackConfiguration RollbackTriggers are not evaluated during an operation.
AWS: Smithy: CreateChangeSet/CreateStack/UpdateStack accept RollbackConfiguration (RollbackTriggers + MonitoringTimeInMinutes). Recorded behavior: if a trigger alarm is in ALARM during the monitoring period, CloudFormation rolls the operation back and the stack settles UPDATE_ROLLBACK_COMPLETE.
Emulator: with the alarm driven into ALARM the executed change set still settles UPDATE_COMPLETE — the trigger is accepted and ignored.
Why: the only automatic health gate a client can attach to a deploy is inert, so a bad deploy is kept instead of being reverted.
Fix: RollbackConfiguration/RollbackTrigger monitoring in the ExecuteChangeSet / UpdateStack operation path (alarm state poll during MonitoringTimeInMinutes).
Traces: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution.

## Evidence
AssertionError: "the ALARM trigger must roll the execution back, got 'UPDATE_COMPLETE'" — expected UPDATE_ROLLBACK_COMPLETE for the stack whose RollbackConfiguration names the CloudWatch alarm that was put into ALARM.

## Steps to Reproduce
Create a stack, create a change set with RollbackConfiguration.RollbackTriggers naming a CloudWatch alarm and a MonitoringTimeInMinutes, put the alarm into ALARM (SetAlarmState), execute the change set, then poll DescribeStacks for the terminal status.
