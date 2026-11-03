Issue-Source: trace-sweep-divergence
Issue-Trace: change-set-on-stack-failure-delete-update-type-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: change-set-on-stack-failure-delete-update-type-rejected cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket cloudformation-cfnuf-reference-diagnose-and-rollback cloudformation-kbdeps-replace-kb-export-while-imported cloudformation-kbdeps-delete-kb-stack-before-unimporting cloudformation-kbdeps-export-index-importers
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ListChangeSets|2xx->2xx|$.ListChangeSetsResponse.ListChangeSetsResult.Summaries
Issue-Key: cloudformation|DescribeChangeSet|4xx->2xx|$.ErrorResponse
Issue-Key: cloudformation|DescribeChangeSet|4xx->2xx|status
Issue-Key: cloudformation|ListStacks|2xx->2xx|$
Issue-Key: cloudformation|ListStacks|2xx->2xx|$.ListStacksResponse.ListStacksResult.StackSummaries.member
Issue-Key: cloudformation|DescribeEvents|2xx->2xx|$.DescribeEventsResponse.DescribeEventsResult.OperationEvents
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*]
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*].ResourceStatus
Issue-Key: cloudformation|ListImports|2xx->4xx|$.ListImportsResponse
Issue-Key: cloudformation|ListImports|2xx->4xx|status
Issue-Related: 20261103T171935Z-cloudformation-resource-failures-never-surface-stacks-and-resources.md
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# CloudFormation change-set lifecycle, export guard and event history surfaces diverge

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChangeSets

## Description
Change sets that AWS has disposed of are still listed and describable, stack listings and event histories are incomplete, and the cross-stack export surface (guard message, exported Outputs, importer index) diverges. This bundle carries 8 distinct causes. Related open issues 20261029T214321Z-cloudformation-the-change-set-status-surface-diverges-delete.md and 20261102T101202Z-cloudformation-8-net-new-root-causes-in-use.md cover neighbouring symptoms and own none of these replay keys (owned_by is empty for all eight), so each must be closed here.

Cause 1 of 8 - ListChangeSets after a change set is disposed of.
AWS: Summaries is empty once the change set is gone.
Emulator: still lists it, with Status 'CREATE_PENDING' and ExecutionStatus 'UNAVAILABLE'.
Why: a caller enumerating change sets sees a stale pending one and waits on or tries to execute something AWS no longer holds.
Fix: the change-set lifecycle in CloudFormation state must remove disposed change sets; ListChangeSets' handler must not list them.
Traces: change-set-on-stack-failure-delete-update-type-rejected#task#4.

Cause 2 of 8 - DescribeChangeSet for that disposed change set.
AWS: 404 with Error.Type 'Sender', Error.Code 'ChangeSetNotFound' and Message 'ChangeSet [<changeSetName>] does not exist' - fixed text 'ChangeSet [' / '] does not exist' with the requested name. Smithy: DescribeChangeSet declares ChangeSetNotFoundException with awsQueryError code 'ChangeSetNotFound' and httpError 404.
Emulator: 200, describing it.
Why: the modeled not-found error never fires, so SDK callers cannot detect a vanished change set.
Fix: DescribeChangeSet's lookup must raise ChangeSetNotFoundException.
Traces: change-set-on-stack-failure-delete-update-type-rejected#task#5.

Cause 3 of 8 - ListStacks with a status filter returns an incomplete list.
AWS: StackSummaries carries every stack matching the filter - here both failed stacks - and a summary row carries StackId, StackName, StackStatus, StackStatusReason, TemplateDescription, CreationTime and DriftInformation.
Emulator: returns only one of the matching stacks, and adds a LastOperations member AWS's StackSummary does not carry.
Why: a caller enumerating stacks in a given state silently misses stacks; the extra member is a field AWS never sends.
Fix: ListStacks' filter/enumeration in the handler and its StackSummary serializer.
Traces: cloudformation-cfns3cnt-create-failed-stack-keeps-live-bucket#task#3.

Cause 4 of 8 - DescribeEvents OperationEvents for a failed update.
AWS: one event per failed resource with EventType 'PROVISIONING_ERROR', ResourceStatus 'UPDATE_FAILED', the resource's ResourceProperties, its LogicalResourceId/PhysicalResourceId and a ResourceStatusReason naming the handler error.
Emulator: returns a different set that omits those events.
Why: the operation-event stream is the diagnostic surface for a failed deployment; without the provisioning errors there is nothing to diagnose from.
Fix: the operation-event recording in the resource provisioning path, read by DescribeEvents.
Traces: cloudformation-cfnuf-reference-diagnose-and-rollback#task#7.

Cause 5 of 8 - DescribeStackEvents for an update canceled by the in-use-export guard.
AWS: the history contains the UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE stack events plus the resource event whose ResourceStatusReason is the 'Update canceled. Cannot update export ...' text.
Emulator: those events are absent.
Why: a caller polling stack events never sees the cancellation or the rollback.
Fix: the stack-event recording for guard cancellation and rollback transitions.
Traces: cloudformation-kbdeps-replace-kb-export-while-imported#task#13.

Cause 6 of 8 - DescribeStacks StackStatusReason for a delete blocked by an in-use export.
AWS: 'Delete canceled. Cannot delete export <exportName> as it is in use by <importingStackName>.' - fixed text with the export actually imported and the importing stack's name, and a terminating period.
Emulator: names a DIFFERENT export of the same stack and omits the trailing period.
Why: the message is how a caller learns which export to unwind; naming the wrong one sends it to the wrong dependency.
Fix: the export-in-use guard's reason construction - it must select the export that is actually imported and match AWS's punctuation.
Traces: cloudformation-kbdeps-delete-kb-stack-before-unimporting#task#8.

Cause 7 of 8 - DescribeStacks Outputs for a stack with exported outputs.
AWS: Stacks[].Outputs lists each output with OutputKey, OutputValue and ExportName for exported ones.
Emulator: Outputs is omitted entirely.
Why: exports are discovered through the owning stack's Outputs; with none, no consumer can resolve an Fn::ImportValue source.
Fix: DescribeStacks' Outputs serializer and the export name recorded in stack state.
Traces: cloudformation-kbdeps-delete-kb-stack-before-unimporting#task#17.

Cause 8 of 8 - ListImports for an export that is imported.
AWS: 200 with ListImportsResult.Imports listing the importing stack names.
Emulator: 400.
Why: the importer index is the only way to find what blocks an export change; an error there makes the dependency undiscoverable.
Fix: ListImports' handler must resolve the export and return its importers.
Traces: cloudformation-kbdeps-export-index-importers#task#9.

## Evidence
Replay of simparity-wire-aws-v1, 8 divergence clusters, all stub_path='' and owned_by=[].
Cause 1: expected ListChangeSetsResult.Summaries '', got a member with Status 'CREATE_PENDING' / ExecutionStatus 'UNAVAILABLE'.
Cause 2: expected 404 Code 'ChangeSetNotFound' Message 'ChangeSet [<name>] does not exist', got 200.
Cause 3: expected two StackSummaries members (both CREATE_FAILED stacks), got one, plus an unexpected LastOperations member.
Cause 4: expected OperationEvents with EventType 'PROVISIONING_ERROR' / ResourceStatus 'UPDATE_FAILED' / ResourceProperties / ResourceStatusReason per failed resource, got a different set (type_mismatch).
Cause 5: expected StackEvents including UPDATE_ROLLBACK_COMPLETE and UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS plus the 'Update canceled. Cannot update export ...' reason, got those members absent.
Cause 6: expected StackStatusReason 'Delete canceled. Cannot delete export <exportA> as it is in use by <importer>.', got 'Delete canceled. Cannot delete export <exportB> as it is in use by <importer>' (different export, no trailing period).
Cause 7: expected Stacks[].Outputs with OutputKey/OutputValue/ExportName entries, got Outputs absent.
Cause 8: expected 200 with ListImportsResult.Imports, got 400.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Cause 1: fixture change-set-on-stack-failure-delete-update-type-rejected, step task#4 - after the change set is disposed of, call ListChangeSets.
3. Causes 2-8: replay the fixture#step bindings in trace_ids (DescribeChangeSet on the disposed change set; ListStacks with a status filter over two failed stacks; DescribeEvents after a failed update; DescribeStackEvents after a guard-canceled update; DescribeStacks on a delete blocked by an in-use export and on the exporting stack; ListImports for the imported export).
4. Compare Summaries, Error.Code/Message, StackSummaries, OperationEvents, StackEvents, StackStatusReason, Outputs and Imports against the recorded AWS responses.
