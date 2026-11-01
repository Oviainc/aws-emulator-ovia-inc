Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.update-rollback-restores-pre-update-state
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.update-rollback-restores-pre-update-state|_wait_terminal#11|Timeout|time.sleep(interval_s)
Issue-Commit: 65165f71b3b3c14984c314d4becd7f5d11cc460b

# cloudformation: an UpdateStack gated by a never-signalled AWS::CloudFormation::WaitCondition never reaches a terminal status, so UPDATE_ROLLBACK is unobservable

> _Note: this issue was identified against the code base at commit `65165f71b3b3c14984c314d4becd7f5d11cc460b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new behavioral root cause of service `cloudformation`, grounded in one failing behavioral check. A fixer must close it before submitting.

Cause 1 of 1: an UpdateStack whose new template adds an AWS::CloudFormation::WaitCondition that is never signalled never drives the stack to a terminal StackStatus. The check polls DescribeStacks for a terminal status and is cut off on the suite's per-check deadline with no terminal value ever observed, so the whole UPDATE_ROLLBACK contract behind it (restored property value, removal of the resource the failed update added, the UPDATE_ROLLBACK_IN_PROGRESS stack event, and the stack still accepting a later valid update) is unreachable.
Failing check: cloudformation.update-rollback-restores-pre-update-state.

AWS: recorded behavior only for the WaitCondition timeout rule — the Smithy model (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models no CloudFormation resource types and no timing. What the model does state, and what the sequence must land on, is the status vocabulary: UpdateStack -> UpdateStackOutput{StackId}; DescribeStacks -> Stack.StackStatus, whose terminal update values are UPDATE_COMPLETE, UPDATE_FAILED, UPDATE_ROLLBACK_COMPLETE and UPDATE_ROLLBACK_FAILED, with Stack.StackStatusReason carrying the cause; DescribeStackEvents -> StackEvent.ResourceStatus; ListStackResources -> StackResourceSummary.LogicalResourceId. On AWS, an AWS::CloudFormation::WaitCondition with Properties.Timeout and Count whose handle receives no signal within Timeout fails the resource; the update is cancelled and the stack rolls back, ending UPDATE_ROLLBACK_COMPLETE. The rollback undoes the half of the update already applied (the AWS::SSM::Parameter Value returns to its pre-update value), the resource the failed update introduced is gone from ListStackResources, DescribeStackEvents contains an UPDATE_ROLLBACK_IN_PROGRESS entry, and the rolled-back stack is not wedged — a subsequent valid UpdateStack reaches UPDATE_COMPLETE. The fixed parts of that contract are the status tokens and the event status token; the stack name, the parameter path and the property values ('v1'/'v2'/'v3') are the check's own per-run inputs and are not part of the requirement.

Emulator: the check is terminated with `Error: Check timed out after 60.0s` while polling DescribeStacks; no terminal StackStatus is reported for the gated update, so no status, no StatusReason and no event stream can be diffed against AWS — only that the operation does not converge. The 60.0s figure is the suite's per-check budget and is NOT the requirement; the requirement is that the unsignalled WaitCondition's Timeout expires, the update fails and the stack settles on a terminal UPDATE_ROLLBACK_* status. Note the emulator does hold an unsignalled WaitCondition in flight (see 20261029T065537Z-cloudformation-an-unsignalled-aws-cloudformation-waitcondition-vanishes-from.md), so the gate is entered but never times out into a failure.

Why: a client relying on AWS's behavior drives deployments with waiters. `aws cloudformation wait stack-update-complete`, botocore's `stack_update_complete` waiter, CDK/SAM progress reporting and any script that polls DescribeStacks all hang instead of being told the update failed and was rolled back. The client is left unable to distinguish 'still working' from 'failed and reverted', and the post-rollback state it would rely on (prior property values restored, the added resource dropped, the stack still updatable) is never established.

Fix: the cloudformation service's stack-update/rollback engine and the provisioning timeline that owns resource-signal gates — an AWS::CloudFormation::WaitCondition whose Timeout elapses unsignalled must fail the resource, cancel the enclosing update, run the rollback (restoring the pre-update resource properties and deleting resources the failed update created), emit the UPDATE_ROLLBACK_IN_PROGRESS stack event and settle Stack.StackStatus on UPDATE_ROLLBACK_COMPLETE, leaving the stack able to accept a further update. Open issue 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md is the same family but does not carry this check's replay key, and open issue 20261029T214321Z-cloudformation-five-behavioral-checks-exhaust-the-harness-s.md lists five other non-converging checks and not this one, so this is filed as a distinct work item and named here rather than folded into either.

Traces: cloudformation.update-rollback-restores-pre-update-state.

## Evidence
[mismatch] cloudformation.update-rollback-restores-pre-update-state
  Error: Check timed out after 60.0s

No emulator response is available to diff: the deadline fires while the check is inside its DescribeStacks poll loop (`_wait_terminal`, which waits for StackStatus in {CREATE_COMPLETE, CREATE_FAILED, DELETE_COMPLETE, DELETE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED, UPDATE_COMPLETE, UPDATE_FAILED, UPDATE_ROLLBACK_COMPLETE, UPDATE_ROLLBACK_FAILED, IMPORT_COMPLETE, IMPORT_ROLLBACK_COMPLETE, IMPORT_ROLLBACK_FAILED}).

Expected (AWS), for the sequence the check drives:
  - UpdateStack with a template that changes AWS::SSM::Parameter 'Param' and adds 'GateHandle' (AWS::CloudFormation::WaitConditionHandle) plus 'Gate' (AWS::CloudFormation::WaitCondition, DependsOn Param, Properties {Handle: {Ref: GateHandle}, Timeout: '60', Count: 1}) that is never signalled.
  - DescribeStacks eventually reports StackStatus == 'UPDATE_ROLLBACK_COMPLETE' (with StackStatusReason naming the failed wait condition).
  - ssm:GetParameter on the parameter returns the PRE-update value.
  - ListStackResources no longer contains logical id 'Gate'; it still contains 'Param'.
  - DescribeStackEvents contains a StackEvent with ResourceStatus == 'UPDATE_ROLLBACK_IN_PROGRESS'.
  - A subsequent valid UpdateStack reaches StackStatus == 'UPDATE_COMPLETE'.

Actual (emulator): no terminal StackStatus is ever reported for the gated update; the check dies on the harness deadline.

Smithy consulted: UpdateStack / DescribeStacks / DescribeStackEvents / ListStackResources in com.amazonaws.cloudformation — Stack.StackStatus and StackEvent.ResourceStatus carry the UPDATE_ROLLBACK_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE values, Stack.StackStatusReason carries the cause, and no CloudFormation resource type or timing is modelled, hence 'recorded behavior only' for the WaitCondition Timeout rule itself.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator (region us-west-2).
2. CreateStack with TemplateBody {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{"Name":"<param>","Type":"String","Value":"v1"}}}} and poll DescribeStacks to CREATE_COMPLETE; confirm ssm:GetParameter returns 'v1'.
3. UpdateStack on the same stack with the same Param set to 'v2' PLUS "GateHandle" {"Type":"AWS::CloudFormation::WaitConditionHandle"} and "Gate" {"Type":"AWS::CloudFormation::WaitCondition","DependsOn":"Param","Properties":{"Handle":{"Ref":"GateHandle"},"Timeout":"60","Count":1}}. Send NO signal to the handle.
4. Poll DescribeStacks for a terminal StackStatus. Observe the stack never settles — against AWS it ends UPDATE_ROLLBACK_COMPLETE once the WaitCondition's 60s Timeout expires unsignalled.
5. After a terminal status exists, check the rollback effects: ssm:GetParameter must read back 'v1'; ListStackResources must not list 'Gate' and must still list 'Param'; DescribeStackEvents must contain ResourceStatus 'UPDATE_ROLLBACK_IN_PROGRESS'.
6. UpdateStack again with Param set to 'v3' and poll to UPDATE_COMPLETE, confirming the rolled-back stack is not wedged.
7. Re-replay: cloudformation.update-rollback-restores-pre-update-state.
