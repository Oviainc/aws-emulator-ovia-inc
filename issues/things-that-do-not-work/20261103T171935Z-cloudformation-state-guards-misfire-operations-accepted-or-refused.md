Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward cloudformation-tsrb-inactive-stacks-rolled-back-and-review stack-policy-input-validation cloudformation-cfnuf-change-set-preview-then-execute cloudformation-cfnuf-reference-diagnose-and-rollback cloudformation-kbdeps-replace-kb-export-while-imported cloudformation-sfn-out-of-band-delete-drift cloudformation-sfn-owning-stack-reverse-lookup change-set-on-stack-failure-disable-rollback-conflict
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|UpdateStack|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|UpdateStack|4xx->2xx|status
Issue-Key: cloudformation|ExecuteChangeSet|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|ExecuteChangeSet|4xx->2xx|status
Issue-Key: cloudformation|ExecuteChangeSet|2xx->4xx|$.ExecuteChangeSetResponse
Issue-Key: cloudformation|ExecuteChangeSet|2xx->4xx|status
Issue-Key: cloudformation|RollbackStack|2xx->4xx|$.RollbackStackResponse
Issue-Key: cloudformation|RollbackStack|2xx->4xx|status
Issue-Key: cloudformation|DetectStackDrift|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DetectStackDrift|4xx->2xx|status
Issue-Key: cloudformation|UpdateStack|2xx->4xx|$.UpdateStackResponse
Issue-Key: cloudformation|UpdateStack|2xx->4xx|status
Issue-Key: cloudformation|DetectStackResourceDrift|2xx->4xx|$.DetectStackResourceDriftResponse
Issue-Key: cloudformation|DetectStackResourceDrift|2xx->4xx|status
Issue-Key: cloudformation|DescribeStackResources|2xx->4xx|$.DescribeStackResourcesResponse
Issue-Key: cloudformation|DescribeStackResources|2xx->4xx|status
Issue-Key: cloudformation|ExecuteChangeSet|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DeploymentConfig.DisableRollback
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DisableRollback
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation state guards misfire: operations accepted or refused in the wrong stack state

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
The emulator's stack-state pre-conditions diverge in both directions: calls AWS refuses in a given state are accepted, and calls AWS allows are refused. This bundle carries 10 distinct causes. Related open issue 20261029T214321Z-cloudformation-the-change-set-status-surface-diverges-delete.md covers change-set status wording and owns none of these replay keys (owned_by is empty for all ten), so each must be closed here.

Cause 1 of 10 - UpdateStack on a stack in CREATE_FAILED created with OnFailure=DO_NOTHING.
AWS: 400 Sender/ValidationError, Message 'This stack cannot be updated from [<stackStatus>] state because it contains [on-failure: <onFailureValue>]. Please use disable-rollback option instead during stack creation.' - the bracketed values come from the stack's state and the create's OnFailure.
Emulator: 200, the update proceeds.
Why: AWS makes a DO_NOTHING-created failed stack un-updatable on purpose; accepting the update lets a caller mutate a stack AWS would force it to recreate.
Fix: UpdateStack's stack-state pre-condition check.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#8 (cluster also covers cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#8, cloudformation-tsrb-inactive-stacks-rolled-back-and-review#task#5, stack-policy-input-validation#task#6).

Cause 2 of 10 - ExecuteChangeSet against a stack in a non-terminal failed state.
AWS: 400 Sender/ValidationError, Message 'This stack is currently in a non-terminal [<stackStatus>] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API' - only the bracketed status varies.
Emulator: 200.
Why: executing a change set from a failed state is what AWS blocks to force an explicit recovery choice.
Fix: ExecuteChangeSet's stack-state pre-condition.
Traces: cloudformation-cfnuf-change-set-preview-then-execute#task#9.

Cause 3 of 10 - ExecuteChangeSet with DisableRollback=true on that same failed stack.
AWS: ALLOWED - 200 with an empty ExecuteChangeSetResult. Smithy: ExecuteChangeSet takes DisableRollback and has an empty output shape.
Emulator: 400.
Why: this is the documented escape hatch the refusal in cause 2 points callers to; refusing it leaves no way forward.
Fix: ExecuteChangeSet's pre-condition must honour DisableRollback.
Traces: cloudformation-cfnuf-change-set-preview-then-execute#task#10.

Cause 4 of 10 - RollbackStack on a stack in a failed update state.
AWS: ALLOWED - 200 returning StackId and OperationId. Smithy: RollbackStackOutput carries StackId and OperationId.
Emulator: 400.
Why: rollback-stack is the other documented recovery verb; refusing it strands the stack.
Fix: RollbackStack's stack-state pre-condition.
Traces: cloudformation-cfnuf-reference-diagnose-and-rollback#task#9.

Cause 5 of 10 - DetectStackDrift while the stack is in a failed state.
AWS: 400 Sender/ValidationError, Message 'Drift detection cannot be performed on stack [<stackId>] while it is in [<stackStatus>] state' - variable parts the stack ARN and its status.
Emulator: 200, drift detection starts.
Why: a drift result computed against a half-applied stack is meaningless, which is why AWS refuses it.
Fix: DetectStackDrift's stack-state pre-condition.
Traces: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#7.

Cause 6 of 10 - UpdateStack with DisableRollback=true on a failed stack: AWS allows it, 200 with StackId and OperationId (Smithy: UpdateStackOutput StackId/OperationId); emulator 400.
Fix: UpdateStack's pre-condition must honour DisableRollback.
Traces: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#9 (cluster also covers cloudformation-kbdeps-replace-kb-export-while-imported#task#18).

Cause 7 of 10 - DetectStackResourceDrift for a resource deleted outside CloudFormation.
AWS: 200 with StackResourceDrift.StackResourceDriftStatus 'DELETED' (PhysicalResourceId, LogicalResourceId, ResourceType and Timestamp populated).
Emulator: 400.
Why: DELETED is the drift verdict for an out-of-band deletion; refusing the call hides it.
Fix: DetectStackResourceDrift's handler must report a missing live resource as DELETED instead of erroring.
Traces: cloudformation-sfn-out-of-band-delete-drift#task#8 (cluster also covers #task#12).

Cause 8 of 10 - DescribeStackResources on a live stack answers 400 where AWS answers 200 with the stack's resource list.
Why: the basic resource read of a healthy stack fails, so nothing downstream of it can run.
Fix: DescribeStackResources' handler/stack lookup for this stack shape.
Traces: cloudformation-sfn-owning-stack-reverse-lookup#task#3 (cluster also covers #task#7).

Cause 9 of 10 - ExecuteChangeSet passing DisableRollback for a change set created with OnStackFailure.
AWS: 400 Sender/ValidationError, Message 'Either DisableRollback or OnStackFailure can be specified, but not both. Validation error as OnStackFailure parameter was specified during CreateChangeSet.' - entirely fixed text.
Emulator: 400 but Message 'DisableRollback cannot be specified when the change set was created with OnStackFailure'.
Why: the refusal is correct but reworded, so message-matching callers do not recognise it.
Fix: ExecuteChangeSet's conflict-validation message.
Traces: change-set-on-stack-failure-disable-rollback-conflict#task#3.

Cause 10 of 10 - DescribeStacks reports DisableRollback (and DeploymentConfig.DisableRollback) as true for a stack created with OnFailure=DO_NOTHING and no DisableRollback.
AWS: both read false - DisableRollback reflects the flag the create actually passed, not an equivalent OnFailure value.
Emulator: both read true.
Why: callers branch on DisableRollback to pick a recovery verb; conflating it with OnFailure sends them down the wrong branch (and contradicts the refusal in cause 1).
Fix: CreateStack's stack-state write for DisableRollback / DeploymentConfig.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#4.

## Evidence
Replay of simparity-wire-aws-v1, 10 divergence clusters, all stub_path='' and owned_by=[].
Cause 1: expected 400 ValidationError 'This stack cannot be updated from [CREATE_FAILED] state because it contains [on-failure: DO_NOTHING]. Please use disable-rollback option instead during stack creation.', got 200.
Cause 2: expected 400 ValidationError 'This stack is currently in a non-terminal [UPDATE_FAILED] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API', got 200.
Cause 3: expected 200 with an empty ExecuteChangeSetResult, got 400.
Cause 4: expected 200 with RollbackStackResult{StackId, OperationId}, got 400.
Cause 5: expected 400 ValidationError 'Drift detection cannot be performed on stack [<stackId>] while it is in [UPDATE_FAILED] state', got 200.
Cause 6: expected 200 with UpdateStackResult{StackId, OperationId}, got 400.
Cause 7: expected 200 with StackResourceDrift.StackResourceDriftStatus 'DELETED', got 400.
Cause 8: expected 200 with two StackResources members, got 400.
Cause 9: expected Message 'Either DisableRollback or OnStackFailure can be specified, but not both. Validation error as OnStackFailure parameter was specified during CreateChangeSet.', got 'DisableRollback cannot be specified when the change set was created with OnStackFailure'.
Cause 10: expected DisableRollback 'false' and DeploymentConfig.DisableRollback 'false', got 'true' for both.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket, step task#8 - create a stack with OnFailure=DO_NOTHING that fails, then call UpdateStack on it.
3. Causes 2-10: replay the fixture#step bindings in trace_ids (ExecuteChangeSet with and without DisableRollback on a failed stack, RollbackStack, DetectStackDrift, UpdateStack with DisableRollback, DetectStackResourceDrift after an out-of-band delete, DescribeStackResources on the Step Functions stack, the DisableRollback/OnStackFailure conflict, and DescribeStacks' DisableRollback value).
4. Compare each response's status, Error.Code/Message and the named fields against the recorded AWS responses.
