Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.delete-stack-duplicate-token-rejected cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it cloudformation.rollback-cancel-continue-flow
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.delete-stack-duplicate-token-rejected|run#19|AssertionError|assert msg == busy_message, msg
Issue-Key: cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it|run#12|AssertionError|assert "ResourceStatusReason" in bad, (
Issue-Key: cloudformation.rollback-cancel-continue-flow|run#36|AssertionError|assert message_bad_state == expected, f"expected message {expected!r}, got {message_bad_state!r}"
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# cloudformation: 3 net-new causes — DeleteStack's busy-stack refusal is reworded, a CREATE_FAILED resource carries no ResourceStatusReason, and ContinueUpdateRollback's bad-state refusal is reworded

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStack, ListStackResources, ContinueUpdateRollback

## Description
This issue carries 3 net-new root causes in the `cloudformation` suite section. Each must be closed before submitting.

Cause 1 of 3: DeleteStack against a stack that has an operation in flight is refused with the right class but the wrong message.
Failing check ids for this cause: cloudformation.delete-stack-duplicate-token-rejected.
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): DeleteStack(StackName required, ClientRequestToken @length 1..128 @pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$) models only TokenAlreadyExistsException; a state refusal is CloudFormation's common ValidationError (HTTP 400). Recorded behavior: DeleteStack while the stack is UPDATE_IN_PROGRESS is refused with Error.Code `ValidationError` and Error.Message `Stack [<StackId>] cannot be deleted while in status <StackStatus>` — fixed text `Stack [`, `] cannot be deleted while in status `; the variable parts are the stack's StackId (the full stack ARN as CreateStack returned it) and the stack's current status, both taken from emulator state. The same text is accepted when the in-flight operation's own ClientRequestToken is reused (the alternative there being TokenAlreadyExistsException with `The client request token <token> has already been used for this stack. Retry the request with a different client request token`).
Emulator: HTTP 400 and Error.Code `ValidationError` are already right; the message is `Stack:<StackId> is in <StackStatus> state and can not be deleted.` — a different template (`Stack:` with no space and no brackets, `is in `, ` state and can not be deleted.` with a trailing period) around the same two values.
Why: the message is the only part of this refusal that distinguishes "the stack is busy, retry" from any other ValidationError. A client or deploy wrapper that matches CloudFormation's wording to decide whether to back off and retry a delete never recognizes it and treats a transient busy-stack condition as a permanent validation failure.
Fix: the state guard of the DeleteStack handler (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/delete_stack.rs) and the shared message builder it uses for the busy-stack refusal — emit AWS's `Stack [<StackId>] cannot be deleted while in status <StackStatus>` template.

Cause 2 of 3: a CREATE_FAILED resource is reported with NO ResourceStatusReason at all, so the owning service's refusal never survives into CloudFormation's resource views.
Failing check ids for this cause: cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it.
AWS: Smithy: StackResourceSummary.ResourceStatusReason and StackResource.ResourceStatusReason are optional members documented "Success/failure message associated with the resource". Recorded behavior: the asymmetry is load-bearing — a resource in a failure status carries the key, populated with CloudFormation's wrap of the owning service's rejection (it names the downstream operation that was refused, here SSM's `PutParameter`), while a resource with nothing to explain OMITS the key entirely rather than carrying an empty string. The same asymmetry holds across ListStackResources, DescribeStackResources and DescribeStackResource, and the stack-level StackStatusReason names the logical id of the resource that failed.
Emulator: for a stack created with OnFailure=DO_NOTHING whose AWS::SSM::Parameter is correctly CREATE_FAILED, the summary row is `{LogicalResourceId, PhysicalResourceId, ResourceType, LastUpdatedTimestamp, ResourceStatus: 'CREATE_FAILED', DriftInformation}` — the ResourceStatusReason key is absent. The stack status, the resource status and the resource set are all correct; only the reason is missing.
Why: the reason is the only place a failing resource's cause is exposed, so the entire diagnose-a-failed-deploy workflow is impossible — a client cannot learn which downstream operation was refused or why, and cannot distinguish an SSM AllowedPattern rejection from a permissions failure or a timeout.
Fix: resource-failure reason propagation in the CloudFormation provisioner's resource-record writer (the ops that serialize these rows — list_stack_resources.rs / describe_stack_resources.rs / describe_stack_resource.rs under services/service-com-amazonaws-cloudformation-cloudformation/src/ops/ — must carry the stored reason, and the provisioner must store the owning service's refusal when a resource fails), keeping the key OMITTED for resources in a success status.

Cause 3 of 3: ContinueUpdateRollback called from a status it is not valid from is refused with the right class but the wrong message.
Failing check ids for this cause: cloudformation.rollback-cancel-continue-flow.
AWS: Smithy: ContinueUpdateRollback(StackName required) models only TokenAlreadyExistsException; a state refusal is the common ValidationError (HTTP 400). Recorded behavior: ContinueUpdateRollback on a stack that is not UPDATE_ROLLBACK_FAILED (here UPDATE_ROLLBACK_COMPLETE) is refused with Error.Code `ValidationError` and Error.Message exactly `RollbackUpdatedStack cannot be called from current stack status` — 100% FIXED text with NO variable part: it names CloudFormation's internal operation name `RollbackUpdatedStack`, not the public operation name, and it does not interpolate the stack's status. The sibling assertion in the same check is the control for the family and passes: CancelUpdateStack from a status it is invalid from answers ValidationError / `CancelUpdateStack cannot be called from current stack status`, and RollbackStack on a stable stack answers ValidationError / `RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.`
Emulator: HTTP 400 / `ValidationError` are right; the message is `ContinueUpdateRollback cannot be called from stack with <StackStatus> status.` — it uses the public operation name, interpolates the stack's current status into text AWS keeps invariant, and adds a trailing period.
Why: a client that keys on this exact sentence to decide "this stack is not stuck, nothing to continue" cannot match it; worse, the emulator's text varies with the stack's status where AWS's is constant, so no exact match is possible at all. Fixing this also exposes the already-correct neighbours as the reference wording.
Fix: the state guard of the ContinueUpdateRollback handler (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/continue_update_rollback.rs) — emit the invariant `RollbackUpdatedStack cannot be called from current stack status`, and leave CancelUpdateStack's and RollbackStack's messages as they are.

Related open issue, named for the fixer but NOT covering any of these: 20261107T184737Z-cloudformation-4-net-new-causes-activatetype-refuses-aws.md carries cloudformation.continue-update-rollback-on-stable-stack-fails and cloudformation.stack-continue-update-rollback-lifecycle, but its cause there is that SNS-backed custom resources are never invoked (the stack never reaches UPDATE_ROLLBACK_FAILED) — it says nothing about ContinueUpdateRollback's refusal wording. 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md carries the two neighbouring ec2mr-cfnres checks but not the ResourceStatusReason omission above.

Traces: the three check ids in trace_ids, one per cause.

## Evidence
Cause 1 — cloudformation.delete-stack-duplicate-token-rejected:
Expected (AWS): cfn.delete_stack(StackName=<stack in UPDATE_IN_PROGRESS>, ClientRequestToken=<fresh token>) -> HTTP 400, Error.Code 'ValidationError', Error.Message 'Stack [<StackId>] cannot be deleted while in status UPDATE_IN_PROGRESS'.
Actual (emulator): HTTP 400, Error.Code 'ValidationError', Error.Message 'Stack:<StackId> is in UPDATE_IN_PROGRESS state and can not be deleted.'
`AssertionError: Stack:arn:aws:cloudformation:<region>:<account>:stack/<stack name>/<stack uuid> is in UPDATE_IN_PROGRESS state and can not be deleted.`
Message anatomy: fixed text is 'Stack [' / '] cannot be deleted while in status '; variable parts are the StackId (the stack ARN) and the current StackStatus. The ARN and uuid in the replay line are per-run values filling that template.

Cause 2 — cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it:
`AssertionError: the CREATE_FAILED resource carries no ResourceStatusReason: {'LogicalResourceId': 'RejectedParam', 'PhysicalResourceId': '<stack>-RejectedParam-<suffix>', 'ResourceType': 'AWS::SSM::Parameter', 'LastUpdatedTimestamp': <ts>, 'ResourceStatus': 'CREATE_FAILED', 'DriftInformation': {'StackResourceDriftStatus': 'NOT_CHECKED'}} — the reason is the only place the owning service's refusal survives into CloudFormation's view`
Expected (AWS): the CREATE_FAILED row carries ResourceStatusReason, and that text quotes the owning service's refused operation (the check asserts 'PutParameter' appears in it); the successful sibling row OMITS the key; the same asymmetry holds in DescribeStackResources and DescribeStackResource; the stack's own StackStatusReason names the failed logical id.
Actual (emulator): the key is absent from the CREATE_FAILED row.

Cause 3 — cloudformation.rollback-cancel-continue-flow:
`AssertionError: expected message 'RollbackUpdatedStack cannot be called from current stack status', got 'ContinueUpdateRollback cannot be called from stack with UPDATE_ROLLBACK_COMPLETE status.'`
Expected (AWS): cfn.continue_update_rollback(StackName=<stack in UPDATE_ROLLBACK_COMPLETE>) -> HTTP 400, 'ValidationError', message exactly 'RollbackUpdatedStack cannot be called from current stack status' (no variable part).
Actual (emulator): HTTP 400, 'ValidationError', 'ContinueUpdateRollback cannot be called from stack with UPDATE_ROLLBACK_COMPLETE status.'
Controls in the same check that PASS and must stay passing: CancelUpdateStack -> 'CancelUpdateStack cannot be called from current stack status'; RollbackStack on a stable stack -> 'RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.'; and the rejected calls correctly leave the stack's status unchanged.

## Steps to Reproduce
Cause 1: CreateStack a stack with one AWS::SNS::Topic and wait for CREATE_COMPLETE. UpdateStack it to a template adding an AWS::CloudFormation::WaitConditionHandle + AWS::CloudFormation::WaitCondition (Count 1, Timeout 300) so the stack parks in UPDATE_IN_PROGRESS. Call DeleteStack(StackName, ClientRequestToken=<a token never used>) and read Error.Code/Error.Message. Then signal the wait-condition handle, let the update finish, and confirm DeleteStack now succeeds.

Cause 2: CreateStack with OnFailure=DO_NOTHING on a template with two AWS::SSM::Parameter resources — GoodParam {Type: String, Value: '42'} and RejectedParam {DependsOn: GoodParam, Type: String, Value: 'not-a-number', AllowedPattern: '^[0-9]+$'} — so SSM refuses only the second. Wait for CREATE_FAILED, then ListStackResources (and DescribeStackResources / DescribeStackResource) and inspect the RejectedParam row: ResourceStatusReason must be present and must quote PutParameter, while the GoodParam row must omit the key.

Cause 3: CreateStack a one-topic stack, wait CREATE_COMPLETE; UpdateStack to a wait-condition template, wait UPDATE_IN_PROGRESS, CancelUpdateStack, wait UPDATE_ROLLBACK_COMPLETE. Then call ContinueUpdateRollback(StackName) and read Error.Code/Error.Message.

Or replay the three check ids in trace_ids.
