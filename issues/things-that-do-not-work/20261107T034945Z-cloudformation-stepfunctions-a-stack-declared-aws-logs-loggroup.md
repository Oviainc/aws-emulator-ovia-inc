Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted cloudformation__stepfunctions.e2e-missing-state-machine-lifecycle cloudformation__stepfunctions.deploy-cfn-detect-drift-reports-out-of-band-logging-level-as-modified-then-in-sync
Issue-Key: cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted|run#17|AssertionError|assert est["log_group"] in groups, (
Issue-Key: cloudformation__stepfunctions.e2e-missing-state-machine-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation__stepfunctions.deploy-cfn-detect-drift-reports-out-of-band-logging-level-as-modified-then-in-sync|run#7|AssertionError|assert drifted["resources"] == {
Issue-Commit: 04fdd45ea890c13ea5d31cc41f99075331e6a703

# cloudformation__stepfunctions: a stack-declared AWS::Logs::LogGroup is never created, and DetectStackDrift reports a state machine IN_SYNC after an out-of-band LoggingConfiguration change (2 causes)

> _Note: this issue was identified against the code base at commit `04fdd45ea890c13ea5d31cc41f99075331e6a703`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackDrift

## Description
Two root causes in the CloudFormation side of the Step Functions deployment suite.

Cause 1 of 2 — an AWS::Logs::LogGroup declared in a template is reported as created but does not exist.
AWS: recorded behavior: a stack containing an AWS::Logs::LogGroup whose LogGroupName is given (here /aws/vendedlogs/states/<name>) reaches CREATE_COMPLETE only once that group exists; DescribeLogGroups(logGroupNamePrefix=<that name>) then returns it, and it survives until the stack deletes it. The resource appears in DescribeStackResources and in DescribeStackResourceDrifts alongside the stack's other resources.
Emulator: the stack reaches CREATE_COMPLETE and reports the resource, but DescribeLogGroups with the declared prefix returns an empty list, and the logical resource is missing from the drift report entirely (only StateMachineRole and the state machine are listed).
Why: every Step Functions stack that vends its own log destination is broken — the state machine points at a log group that does not exist; and because the group is absent, a drift or lifecycle assertion about any other resource of the stack can no longer be attributed (an IN_SYNC sibling set of one says nothing).
Fix: the AWS::Logs::LogGroup resource provider's create path (and its registration in the stack's resource/drift inventory).
Traces: cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted, cloudformation__stepfunctions.e2e-missing-state-machine-lifecycle

Cause 2 of 2 — DetectStackDrift does not compare AWS::StepFunctions::StateMachine properties, so an out-of-band change reports IN_SYNC.
AWS: recorded behavior: after an out-of-band UpdateStateMachine lowers loggingConfiguration.level from ALL to ERROR, DetectStackDrift/DescribeStackResourceDrifts reports that resource MODIFIED with exactly one PropertyDifference — PropertyPath /LoggingConfiguration/Level, ExpectedValue ALL, ActualValue ERROR, DifferenceType NOT_EQUAL — while the untouched role and log group stay IN_SYNC; DescribeStackDriftDetectionStatus reports StackDriftStatus DRIFTED with DriftedStackResourceCount 1 and DescribeStacks' DriftInformation records DRIFTED. Restoring the level makes the next detection IN_SYNC with count 0 (drift is evaluated live, not latched).
Emulator: the same detection reports {'StateMachineRole': 'IN_SYNC', 'TaskOrchestrator': 'IN_SYNC'} — the state machine is IN_SYNC with no PropertyDifferences, and the log-group resource is absent from the report (cause 1).
Why: drift detection is the control that tells an operator a deployed workflow no longer matches its template; here it always says everything is fine, so an out-of-band logging downgrade is invisible.
Fix: the drift-detection comparator for AWS::StepFunctions::StateMachine — read the live resource's properties and emit PropertyDifferences, feeding the stack-level drift status and counts.
Traces: cloudformation__stepfunctions.deploy-cfn-detect-drift-reports-out-of-band-logging-level-as-modified-then-in-sync

## Evidence
From this replay of simparity-behavioral-aws-v1:

Cause 1a: AssertionError: the stack's log group '/aws/vendedlogs/states/<name>' is not in []; the other two resources must still exist for the disagreement to be about one of them — the stack had reached CREATE_COMPLETE with that LogGroupName as a template parameter, and the IAM role of the same stack was found by GetRole.
Cause 1b: AssertionError: [phase 4/7 solve] the resources detected IN_SYNC are ['StateMachineRole']; without untouched siblings 'DRIFTED' says nothing about which resource is missing.

Cause 2: AssertionError: only the state machine must drift, and as MODIFIED; got {'StateMachineRole': 'IN_SYNC', 'TaskOrchestrator': 'IN_SYNC'}. Expected {'StateMachineLogGroup': 'IN_SYNC', 'StateMachineRole': 'IN_SYNC', 'TaskOrchestrator': 'MODIFIED'} with PropertyDifferences [('/LoggingConfiguration/Level', 'ALL', 'ERROR', 'NOT_EQUAL')], StackDriftStatus DRIFTED and DriftedStackResourceCount 1.

## Steps to Reproduce
1. CreateStack from a template declaring an AWS::IAM::Role, an AWS::Logs::LogGroup (LogGroupName /aws/vendedlogs/states/<name>) and an AWS::StepFunctions::StateMachine with LoggingConfiguration Level ALL pointing at that group; wait for CREATE_COMPLETE.
2. Call logs:DescribeLogGroups(logGroupNamePrefix=<that name>) — the group must be returned (cause 1).
3. Call DetectStackDrift and poll DescribeStackDriftDetectionStatus to DETECTION_COMPLETE; DescribeStackResourceDrifts must list all three logical resources (cause 1).
4. Call states:UpdateStateMachine on the stack's state machine with loggingConfiguration.level=ERROR, then repeat step 3: the state machine must be MODIFIED with the /LoggingConfiguration/Level NOT_EQUAL difference and the stack DRIFTED (cause 2).
5. Set the level back to ALL and detect again: IN_SYNC with 0 drifted resources.
