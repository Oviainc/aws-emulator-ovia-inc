Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution
Issue-Key: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution|run#22|AssertionError|assert two_events == [], (
Issue-Commit: 6537d79e9f76a83fe65f7d2499927078c6ec62b1

# cloudformation__cloudwatch: a change set executed with a RollbackConfiguration trigger alarm already in ALARM provisions the new resource before rolling back, instead of aborting untouched

> _Note: this issue was identified against the code base at commit `6537d79e9f76a83fe65f7d2499927078c6ec62b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: the rollback trigger is evaluated after the change set's resource changes are applied, not before.

AWS: recorded behavior only for the ordering (the Smithy model types RollbackConfiguration/RollbackTriggers on CreateChangeSet but does not state when it is evaluated). When a change set carries RollbackConfiguration.RollbackTriggers naming an AWS::CloudWatch::Alarm that is ALREADY in ALARM state at execution time, ExecuteChangeSet takes the stack to UPDATE_ROLLBACK_COMPLETE WITHOUT ever touching the resources the change set would have added: DescribeStackEvents for the execution shows no event at all for the added logical resource, and ListStackResources afterwards contains only the pre-existing resources. The rollback reason on the stack's UPDATE_ROLLBACK_IN_PROGRESS event is "The following CloudWatch Alarms were in ALARM state: <AlarmName>" — fixed text with the alarm name(s) from the request's RollbackTriggers. Control: the same update with RollbackTriggers=[] completes (UPDATE_COMPLETE) and does add the resource, even while the alarm is still in ALARM.

Emulator: the status and the reason are right, but the ordering is not. The added logical resource is fully provisioned first and only then rolled back: DescribeStackEvents for the execution contains [('Two','CREATE_IN_PROGRESS'), ('Two','CREATE_IN_PROGRESS'), ('Two','CREATE_COMPLETE')] for a resource AWS never touches. The replay reports: AssertionError: an alarm already in ALARM aborts before the added Two is ever touched: [('Two', 'CREATE_IN_PROGRESS'), ('Two', 'CREATE_IN_PROGRESS'), ('Two', 'CREATE_COMPLETE')]. ('Two' is the check's logical id; the requirement is that no event for the change set's added resources appears.)

Why: a rollback trigger exists so that a known-bad condition prevents the change from being applied at all. A client that arms an alarm as a deployment guard gets the side effects it was guarding against (the resource is created, then deleted again), and anything reading the stack's event stream sees a create that real AWS never performs — so event-driven automation and audit trails diverge even though the final stack status matches.

Fix: the ExecuteChangeSet handler's rollback-trigger evaluation — the RollbackConfiguration triggers must be sampled before the first resource action of the execution and short-circuit straight to UPDATE_ROLLBACK_IN_PROGRESS/UPDATE_ROLLBACK_COMPLETE when any is already breaching, rather than being polled only during the post-apply monitoring window.

Traces: cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution

## Evidence
Check cloudformation__cloudwatch.change-set-rollback-configuration-alarm-rolls-back-the-execution [mismatch]

expected: for the executed change set, the events for the newly added logical resource are [] (AWS aborts before touching it) and ListStackResources afterwards == {the pre-existing resource only}.
actual:   AssertionError: an alarm already in ALARM aborts before the added Two is ever touched: [('Two', 'CREATE_IN_PROGRESS'), ('Two', 'CREATE_IN_PROGRESS'), ('Two', 'CREATE_COMPLETE')]

The earlier assertions of the same check passed, which localises the defect precisely:
  - DescribeChangeSet echoed RollbackConfiguration verbatim;
  - the stack did end UPDATE_ROLLBACK_COMPLETE;
  - the UPDATE_ROLLBACK_IN_PROGRESS ResourceStatusReason was exactly 'The following CloudWatch Alarms were in ALARM state: <alarm>'.
Only the ordering (apply-then-roll-back vs abort-before-apply) diverges.

## Steps to Reproduce
1. cloudwatch: PutMetricAlarm(AlarmName=A, Namespace=..., Statistic=Sum, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=GreaterThanOrEqualToThreshold, TreatMissingData=breaching); SetAlarmState(AlarmName=A, StateValue=ALARM).
2. cloudformation: CreateStack with Resources {One: AWS::CloudFormation::WaitConditionHandle}; wait for CREATE_COMPLETE.
3. CreateChangeSet(StackName=<stack id>, ChangeSetType=UPDATE, TemplateBody with Resources {One, Two: AWS::CloudFormation::WaitConditionHandle}, RollbackConfiguration={RollbackTriggers:[{Arn:<alarm arn>, Type:'AWS::CloudWatch::Alarm'}], MonitoringTimeInMinutes:1}); wait for CREATE_COMPLETE.
4. ExecuteChangeSet; wait for the stack to leave *_IN_PROGRESS.
5. DescribeStackEvents and filter to events whose LogicalResourceId == 'Two'. The emulator returns CREATE_IN_PROGRESS/CREATE_COMPLETE events; AWS returns none.
