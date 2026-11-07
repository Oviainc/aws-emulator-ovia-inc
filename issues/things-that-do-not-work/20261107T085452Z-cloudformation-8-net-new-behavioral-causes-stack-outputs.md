Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.nested-stack-parentage-and-cascade-delete cloudformation.deep-cloudformation-2-deployment-mode-express cloudformation.delete-stack-retain-resources-invalid-list-rejected cloudformation.execute-changeset-missing-capabilities-rejected cloudformation.stack-policy-blocks-unauthorized-updates cloudformation.stack-policy-during-update-body-overrides-for-one-update-only cloudformation.op-contract-setStackPolicy cloudformation.stack-export-removal-while-imported-rolls-back-producer cloudformation.changeset-execute-invalid-status-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.nested-stack-parentage-and-cascade-delete|run#30|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in parent_obj.get("Outputs", [])}
Issue-Key: cloudformation.deep-cloudformation-2-deployment-mode-express|run#13|AssertionError|assert stack.get("StackStatusReason") is None, stack.get("StackStatusReason")
Issue-Key: cloudformation.delete-stack-retain-resources-invalid-list-rejected|_expect_error#12|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.execute-changeset-missing-capabilities-rejected|run#34|AssertionError|assert nested_reason == "Requires capabilities : [CAPABILITY_NAMED_IAM]", (
Issue-Key: cloudformation.stack-policy-blocks-unauthorized-updates|run#27|AssertionError|assert reasons == [expected_reason], (
Issue-Key: cloudformation.stack-policy-during-update-body-overrides-for-one-update-only|run#17|AssertionError|assert blocked == {"outcome": "UPDATE_ROLLBACK_COMPLETE", "evidence": _DENIED_REASON}, (
Issue-Key: cloudformation.op-contract-setStackPolicy|_error#30|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.stack-export-removal-while-imported-rolls-back-producer|run#18|AssertionError|assert stack_rows == [
Issue-Key: cloudformation.changeset-execute-invalid-status-rejected|_wait_change_set_status#12|Timeout|raise AssertionError(
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# cloudformation: 8 net-new behavioral causes — stack Outputs without OutputValue, rollback keeps the failure reason, DeleteStack RetainResources/SetStackPolicy accept invalid input, wrapped nested-stack reason, stack-policy statement numbering, in-use-export event sequence, macro change set never settles

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Eight distinct root causes on the CloudFormation-led behavioral surface. A fixer must close all eight before submitting.

Cause 1 of 8: DescribeStacks returns Outputs entries without OutputValue.
AWS: Smithy com.amazonaws.cloudformation#Output declares OutputKey/OutputValue/Description/ExportName, and DescribeStacks on a stack that reached a terminal complete state returns each Output with its RESOLVED OutputValue (here a parent stack whose output is fed by a nested child stack's output).
Emulator: the Output entry exists but the value member is absent, so every consumer raises KeyError: 'OutputValue'.
Why: the deploy-then-read pattern is dead — a client cannot take an id/url out of a stack and drive the next call with it.
Fix: the DescribeStacks serializer and stack-output resolution in CloudFormation state (nested-stack output propagation included).
Traces: cloudformation.nested-stack-parentage-and-cascade-delete. Open issue 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md describes the same symptom (its Cause 1) but its Issue-Trace-Ids do NOT carry this check id, so this replay key is not covered by it.

Cause 2 of 8: a stack that rolled back keeps the per-resource create failure in the stack-level StackStatusReason.
AWS: with DeploymentConfig Mode=STANDARD, a create whose resource fails rolls back and the terminal ROLLBACK_COMPLETE stack carries NO StackStatusReason — the failure text lives on the resource's CREATE_FAILED event (Smithy Stack.StackStatusReason is optional: 'Success/failure message associated with the stack status'). The EXPRESS arms of the same check pass, which pins this to the STANDARD rollback path.
Emulator: DescribeStacks reports StackStatusReason 'The following resource(s) failed to create: [B].' on the ROLLBACK_COMPLETE stack — fixed text 'The following resource(s) failed to create: [' + the failed logical ids + '].'
Why: a client that reads StackStatusReason to tell an un-rolled-back express failure from a cleanly rolled-back stack misreads the rolled-back one as still carrying a live failure.
Fix: the rollback-completion path of the stack engine must clear the stack-level status reason when it reaches ROLLBACK_COMPLETE.
Traces: cloudformation.deep-cloudformation-2-deployment-mode-express.

Cause 3 of 8: DeleteStack with RetainResources naming a logical id that is not a resource of the stack is accepted instead of refused.
AWS: 400 ValidationError with Message 'The specified resources to retain must be in a valid state. Do not specify resources that are in the DELETE_COMPLETE state.' — entirely fixed text for this error class. Smithy: DeleteStack declares only TokenAlreadyExistsException, so the refusal is the service-common ValidationError; RetainResources is a plain list with no modeled content constraint, so the content check is the service's.
Emulator: HTTP 200 with an empty DeleteStackResult; the bogus retain list is silently ignored.
Why: a caller that names a stale or mistyped logical id believes a resource is being retained while CloudFormation deletes it (or, in the DELETE_FAILED case here, believes it re-drove the delete).
Fix: DeleteStack's RetainResources validation against the stack's current resource set, ahead of scheduling the delete.
Traces: cloudformation.delete-stack-retain-resources-invalid-list-rejected.

Cause 4 of 8: a native nested-stack resource failure reason is wrapped in the third-party resource-handler envelope.
AWS: when an executed change set's AWS::CloudFormation::Stack child needs a capability the caller did not pass, the nested resource's failure event carries exactly 'Requires capabilities : [CAPABILITY_NAMED_IAM]' — fixed text 'Requires capabilities : [' plus the list of capabilities the nested template requires.
Emulator: ResourceStatusReason 'Resource handler returned message: "Requires capabilities : [CAPABILITY_NAMED_IAM]" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)' — an envelope with an invented RequestToken and HandlerErrorCode that AWS emits only for registered third-party resource types.
Why: clients and humans that match the capability reason on native AWS:: resources see a shape AWS never produces, and the extra fields are per-run noise.
Fix: stack-event reason propagation for native AWS:: resource provisioners must not wrap the reason in the resource-handler envelope.
Traces: cloudformation.execute-changeset-missing-capabilities-rejected.

Cause 5 of 8: the stack-policy denial reason numbers the wrong statement.
AWS: the refusal reason is 'Action denied by stack policy: Statement [#N] does not allow [<action>] for resource [<resource>];' — fixed text 'Action denied by stack policy: Statement [#', '] does not allow [', '] for resource [' and '];', with the statement number, the denied action from the policy and the protected resource as its variable parts. Recorded behavior: a policy of a single Deny statement AND a policy of [Allow Update:*, Deny Update:Replace] BOTH report Statement [#1] — the number is the position among the policy's Deny statements, not the index of the statement in the whole Statement array.
Emulator: the wording and the action/resource are right, but the two-statement policy's denial reports Statement [#2] (the array index).
Why: a client that parses the reason to point an operator at the offending statement blames the wrong statement.
Fix: statement numbering in the stack-policy evaluator's denial reason.
Traces: cloudformation.stack-policy-blocks-unauthorized-updates; cloudformation.stack-policy-during-update-body-overrides-for-one-update-only.

Cause 6 of 8: SetStackPolicy accepts a malformed stack policy body.
AWS: 400 ValidationError with Message 'Error validating stack policy: Invalid stack policy' — entirely fixed text. Smithy: SetStackPolicy's StackPolicyBody is length{min:1,max:16384} with no content constraint and the operation declares no operation-specific error, so the content refusal is the service-common ValidationError.
Emulator: HTTP 200 for StackPolicyBody '{not valid json'; the body is stored.
Why: a caller believes its update protection was replaced when AWS refused the document outright, and later reads back a policy AWS does not hold.
Fix: SetStackPolicy's stack-policy document validation ahead of the write to stack state.
Traces: cloudformation.op-contract-setStackPolicy. The open wire issue 20261103T171935Z-cloudformation-never-validates-a-stack-policy-so-invalid.md carries the wire keys for this defect (its Cause 2) but not this behavioral replay key.

Cause 7 of 8: an update that drops an imported export produces the wrong stack-event sequence and the wrong reason.
AWS stack-level events for an UpdateStack removing an export another stack imports: ('UPDATE_IN_PROGRESS', 'User Initiated'), ('UPDATE_ROLLBACK_IN_PROGRESS', 'Delete canceled. Cannot delete export <export name> as it is in use by <importing stack name>.'), ('UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS', no reason), ('UPDATE_ROLLBACK_COMPLETE', no reason). Fixed text: 'Delete canceled. Cannot delete export ' / ' as it is in use by ' / '.'; the variables are the export name and the importer's stack name.
Emulator: emits a stack-level UPDATE_FAILED event AWS does not emit, words the reason 'Update canceled. Cannot update export <name> as it is in use by <importer>.', repeats that reason on UPDATE_ROLLBACK_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE (AWS leaves both without a reason) and never emits UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS.
Why: event-driven clients that wait for the cleanup phase never see it, and anything matching the documented 'Delete canceled. Cannot delete export' text reads the wrong cause.
Fix: the export-in-use guard on UpdateStack and the stack-event emission of the update-rollback path.
Traces: cloudformation.stack-export-removal-while-imported-rolls-back-producer.

Cause 8 of 8: a change set over a macro-transformed template never settles, so its CREATE_IN_PROGRESS refusal is unobservable.
AWS: CreateChangeSet returns immediately and the change set stays CREATE_IN_PROGRESS while the macro's Lambda runs; ExecuteChangeSet inside that window is refused with error code 'InvalidChangeSetStatus' (no 'Exception' suffix on the wire) and Message 'ChangeSet [<change set ARN>] cannot be executed in its current status of [CREATE_IN_PROGRESS]' — fixed text 'ChangeSet [' / '] cannot be executed in its current status of [' / ']', with the change set ARN and the current status as variables.
Emulator: recorded behavior only — the check makes no progress and the harness reports 'Check timed out after 60.0s', so no response was observed for the step it blocked on. Hypothesis: the macro-backed path (the macro stack reaching CREATE_COMPLETE, or CreateChangeSet invoking the macro synchronously instead of leaving the change set in CREATE_IN_PROGRESS) never settles.
Why: a client cannot create a change set over a template that uses a macro transform, nor observe the in-progress execute refusal.
Fix: the change-set creation path's transform/macro handling — it must return immediately and drive the change set's status asynchronously.
Traces: cloudformation.changeset-execute-invalid-status-rejected.

## Evidence
Cause 1 — expected Outputs[].OutputValue on the parent stack; actual KeyError: 'OutputValue' at `outputs = {o["OutputKey"]: o["OutputValue"] for o in parent_obj.get("Outputs", [])}`.
Cause 2 — expected StackStatusReason absent on ROLLBACK_COMPLETE; actual AssertionError: 'The following resource(s) failed to create: [B].'
Cause 3 — expected ('ValidationError', 'The specified resources to retain must be in a valid state. Do not specify resources that are in the DELETE_COMPLETE state.'); actual 'expected rejection, but the call SUCCEEDED and returned {..."HTTPStatusCode": 200...}'.
Cause 4 — expected ResourceStatusReason 'Requires capabilities : [CAPABILITY_NAMED_IAM]'; actual 'Resource handler returned message: "Requires capabilities : [CAPABILITY_NAMED_IAM]" (RequestToken: 0b11c152-2d58-8750-63fe-4e5ceb952055, HandlerErrorCode: InvalidRequest)' (the token is a per-run value, the envelope is the defect).
Cause 5 — expected ['Action denied by stack policy: Statement [#1] does not allow [Update:Replace] for resource [LogicalResourceId/Topic];']; actual ['... Statement [#2] ...']; and expected '... Statement [#1] does not allow [Update:Modify] for resource [LogicalResourceId/Protected];' actual '... Statement [#2] ...'.
Cause 6 — expected ValidationError 'Error validating stack policy: Invalid stack policy' for StackPolicyBody '{not valid json'; actual HTTP 200.
Cause 7 — expected [('UPDATE_IN_PROGRESS','User Initiated'), ('UPDATE_ROLLBACK_IN_PROGRESS','Delete canceled. Cannot delete export <used> as it is in use by <consumer>.'), ('UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS',None), ('UPDATE_ROLLBACK_COMPLETE',None)]; actual [('UPDATE_IN_PROGRESS','User Initiated'), ('UPDATE_FAILED','Update canceled. Cannot update export <used> as it is in use by <consumer>.'), ('UPDATE_ROLLBACK_IN_PROGRESS','Update canceled. Cannot update export ...'), ('UPDATE_ROLLBACK_COMPLETE','Update canceled. Cannot update export ...')].
Cause 8 — expected InvalidChangeSetStatus 'ChangeSet [<arn>] cannot be executed in its current status of [CREATE_IN_PROGRESS]'; actual 'Error: Check timed out after 60.0s' with no response observed.

## Steps to Reproduce
Replay the behavioral suite and re-run the check ids in trace_ids. Minimal repros: (1) CreateStack a parent template with a nested AWS::CloudFormation::Stack and an Output fed from the child, then DescribeStacks and read Stacks[0].Outputs[0].OutputValue. (2) CreateStack with DeploymentConfig {Mode: STANDARD} over a template whose second SSM parameter reuses the first's Name, wait for ROLLBACK_COMPLETE and read StackStatusReason. (3) Create a stack with an S3 bucket, put an object in it, DeleteStack to DELETE_FAILED, then DeleteStack RetainResources=['NoSuchLogicalId']. (4) ExecuteChangeSet without CAPABILITY_NAMED_IAM over a template with a nested stack that declares a named IAM role, then DescribeStackEvents for the nested resource's reason. (5) SetStackPolicy with [{Allow Update:*},{Deny Update:Replace on LogicalResourceId/Topic}], force a replacement update and read the Topic's UPDATE_FAILED reason. (6) SetStackPolicy with StackPolicyBody '{not valid json'. (7) Create a producer stack exporting a name, a consumer importing it, then UpdateStack the producer dropping the export and read DescribeStackEvents. (8) Deploy a macro stack, CreateChangeSet over a template using that macro's Transform and call ExecuteChangeSet immediately.
