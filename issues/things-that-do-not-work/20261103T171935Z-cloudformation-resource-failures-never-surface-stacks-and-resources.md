Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfnuf-change-set-preview-then-execute
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfnuf-change-set-preview-then-execute cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed cloudformation-cfnuf-reference-diagnose-and-rollback cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward cloudformation-kbdeps-replace-kb-export-while-imported cloudformation-tsrb-inactive-stacks-rolled-back-and-review cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.StackStatusReason
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.LastOperations.member
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.Outputs
Issue-Key: cloudformation|DescribeStacks|2xx->2xx|$.DescribeStacksResponse.DescribeStacksResult.Stacks.member.DeletionTime
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[*].ResourceStatus
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[*].ResourceStatusReason
Issue-Key: cloudformation|DescribeStackResource|2xx->2xx|$.DescribeStackResourceResponse.DescribeStackResourceResult.StackResourceDetail.ResourceStatus
Issue-Key: cloudformation|DescribeStackResource|2xx->2xx|$.DescribeStackResourceResponse.DescribeStackResourceResult.StackResourceDetail.ResourceStatusReason
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[LogicalResourceId="Tab"].ResourceStatus
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries.member[LogicalResourceId="Tab"].ResourceStatusReason
Issue-Key: cloudformation|ListStackResources|2xx->2xx|$.ListStackResourcesResponse.ListStackResourcesResult.StackResourceSummaries
Issue-Related: 20261029T171940Z-cloudformation-change-set-previews-operation-events-and-the.md
Issue-Related: 20261103T171935Z-cloudformation-reports-resource-names-where-aws-reports-resource.md
Issue-Related: 20261103T171935Z-cloudformation-synthesizes-physical-resource-ids-of-the-wrong.md
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation resource failures never surface: stacks and resources report success instead of FAILED or rollback states

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
The emulator marks every provisioned resource successful, so an operation whose resources cannot be created or updated still settles in a *_COMPLETE state and no rollback is ever recorded. This bundle carries 10 distinct causes. Related open issues 20261102T101202Z-cloudformation-7-net-new-root-causes-failed-creates.md and 20261029T214321Z-cloudformation-a-failed-create-parks-in-create-failed.md describe neighbouring symptoms but own none of these replay keys (owned_by is empty for all ten), so each must be closed here.

Cause 1 of 10 - DescribeStacks after an update that points AWS::Lambda::Alias at a nonexistent function version.
AWS: the resource update fails, the stack ends StackStatus 'UPDATE_FAILED' and StackStatusReason 'The following resource(s) failed to update: [<LogicalResourceId>].' - fixed text 'The following resource(s) failed to update: [' / '].', with the failing logical ids from the template. Smithy: StackStatus enum includes UPDATE_FAILED and the rollback states; recorded response confirms the reason text.
Emulator: StackStatus 'UPDATE_COMPLETE' with no StackStatusReason.
Why: every recovery decision (rollback, continue-update-rollback, fix-forward) is keyed on the stack reaching a failure state; reporting success means a broken deployment looks healthy.
Fix: the resource provisioning path for AWS::Lambda::Alias must fail when FunctionVersion does not resolve, and the stack state machine must transition the stack to UPDATE_FAILED and set the reason.
Traces: cloudformation-cfnuf-change-set-preview-then-execute#task#6 (cluster also covers cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed#task#4, cloudformation-cfnuf-reference-diagnose-and-rollback#task#6).

Cause 2 of 10 - DescribeStacks after the same failure driven through UpdateStack rather than a change set: expected UPDATE_FAILED with the same reason template, got UPDATE_COMPLETE.
Fix: same provisioning failure path, reached from UpdateStack.
Traces: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#4.

Cause 3 of 10 - DescribeStacks after RollbackStack on a failed update.
AWS: the stack ends 'UPDATE_ROLLBACK_COMPLETE' and LastOperations lists the ROLLBACK operation ahead of the UPDATE_STACK it reverted.
Emulator: 'UPDATE_COMPLETE' and LastOperations carries only the update.
Why: a caller polling for the rollback to settle never observes it and cannot tell whether the rollback ran.
Fix: RollbackStack's state transition and the LastOperations history in stack state.
Traces: cloudformation-cfnuf-reference-diagnose-and-rollback#task#10.

Cause 4 of 10 - DescribeStacks after an update canceled by the in-use-export guard: expected UPDATE_ROLLBACK_COMPLETE with a ROLLBACK operation recorded and the pre-update Outputs restored; got UPDATE_COMPLETE.
Fix: the export-guard cancellation path must roll the stack back rather than complete it.
Traces: cloudformation-kbdeps-replace-kb-export-while-imported#task#10.

Cause 5 of 10 - DescribeStacks for a create whose resources fail.
AWS: the stack ends 'ROLLBACK_COMPLETE', DeletionTime is set, and LastOperations lists ROLLBACK ahead of CREATE_STACK.
Emulator: 'CREATE_COMPLETE', no DeletionTime, LastOperations carries only CREATE_STACK.
Why: a stack that AWS considers inactive is reported as a healthy live stack, so inactive-stack filtering and review flows are wrong.
Fix: the create-failure rollback path in the stack state machine, and DeletionTime/LastOperations in stack state.
Traces: cloudformation-tsrb-inactive-stacks-rolled-back-and-review#task#3.

Cause 6 of 10 - ListStackResources on that rolled-back create: AWS reports each resource ResourceStatus 'DELETE_COMPLETE' after the rollback removed it; emulator reports 'CREATE_COMPLETE'.
Fix: the rollback must update per-resource status in stack state.
Traces: cloudformation-tsrb-inactive-stacks-rolled-back-and-review#task#4.

Cause 7 of 10 - ListStackResources after a rolled-back update.
AWS: the reverted resource reads ResourceStatus 'UPDATE_ROLLBACK_COMPLETE' with ResourceStatusReason 'Rollback succeeded for the failed resources.' (fixed text, no variable part), and the AWS::Lambda::Version row's PhysicalResourceId is the published version ARN.
Emulator: 'UPDATE_COMPLETE' with no reason, and the version row carries a '<stack>-<logical>-<hex>' name.
Fix: the rollback path's per-resource status/reason, and the AWS::Lambda::Version provisioner's physical id.
Traces: cloudformation-cfnuf-lambda-crosscheck-then-delete-from-update-failed#task#5.

Cause 8 of 10 - DescribeStackResource after a rolled-back update: same UPDATE_ROLLBACK_COMPLETE + 'Rollback succeeded for the failed resources.' expected, plus StackResourceDetail.Metadata present; emulator reports UPDATE_COMPLETE, no reason, no Metadata.
Fix: DescribeStackResource's serializer plus the rollback status write.
Traces: cloudformation-cfnuf-reference-diagnose-and-rollback#task#11.

Cause 9 of 10 - ListStackResources on a create where one resource could not be created: AWS reports that resource ResourceStatus 'CREATE_FAILED' with a ResourceStatusReason; emulator reports 'CREATE_COMPLETE' with no reason.
Fix: the resource provisioner must record per-resource creation failure.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#6.

Cause 10 of 10 - ListStackResources on a CREATE_FAILED stack returns an EMPTY StackResourceSummaries where AWS returns both the successfully created resource (CREATE_COMPLETE) and the failed one (CREATE_FAILED with a reason). Smithy: ListStackResourcesOutput.StackResourceSummaries is the paginated items member, so an empty list is a claim the stack has no resources.
Why: the resources a failed create left behind are exactly what a caller must inspect to clean up.
Fix: ListStackResources' handler must list resources of stacks in failed states.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#5.

## Evidence
Replay of simparity-wire-aws-v1, 10 divergence clusters, all stub_path='' and owned_by=[].
Cause 1/2: expected StackStatus 'UPDATE_FAILED' + StackStatusReason 'The following resource(s) failed to update: [Fn0Alias].', got 'UPDATE_COMPLETE' with no reason (poll reports 'UPDATE_COMPLETE' is in fail_on).
Cause 3/4: expected 'UPDATE_ROLLBACK_COMPLETE' with LastOperations [ROLLBACK, UPDATE_STACK], got 'UPDATE_COMPLETE' with only the update.
Cause 5: expected 'ROLLBACK_COMPLETE' + DeletionTime present + LastOperations [ROLLBACK, CREATE_STACK], got 'CREATE_COMPLETE', DeletionTime missing, LastOperations [CREATE_STACK].
Cause 6: expected ResourceStatus 'DELETE_COMPLETE' on three rows, got 'CREATE_COMPLETE'.
Cause 7/8: expected 'UPDATE_ROLLBACK_COMPLETE' + ResourceStatusReason 'Rollback succeeded for the failed resources.', got 'UPDATE_COMPLETE' and no reason.
Cause 9: expected ResourceStatus 'CREATE_FAILED' + a ResourceStatusReason on the failed row, got 'CREATE_COMPLETE' and no reason.
Cause 10: expected two StackResourceSummaries members, got ''.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture cloudformation-cfnuf-change-set-preview-then-execute, step task#6 - execute a change set that sets an AWS::Lambda::Alias FunctionVersion to a version that does not exist, then poll DescribeStacks.
3. Causes 2-10: replay the fixture#step bindings in trace_ids (UpdateStack-driven failure, RollbackStack, export-guard cancellation, a create whose resources fail, and the per-resource Describe/List views of each).
4. Compare StackStatus, StackStatusReason, DeletionTime, LastOperations, ResourceStatus and ResourceStatusReason against the recorded AWS responses.
