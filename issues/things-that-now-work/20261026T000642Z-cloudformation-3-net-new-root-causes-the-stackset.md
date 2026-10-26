Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.detect-stackset-drift-lifecycle cloudformation.list-stack-instance-resource-drifts cloudformation.stack-set-drift-detection-lifecycle cloudformation.export-deletion-blocked-while-imported-rejected cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.detect-stackset-drift-lifecycle|_bootstrap_self_managed_stackset#3|Timeout|time.sleep(5)
Issue-Key: cloudformation.list-stack-instance-resource-drifts|_bootstrap_self_managed_stackset#3|Timeout|time.sleep(5)
Issue-Key: cloudformation.stack-set-drift-detection-lifecycle|_create_stack_set_with_retry#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation.export-deletion-blocked-while-imported-rejected|_wait_for_import_registered#15|Timeout|time.sleep(4)
Issue-Key: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip|_wait_terminal#21|Timeout|time.sleep(5)
Issue-Commit: c7f894d9cab6281b7f6a51d1fbce512135c74b00

# cloudformation: 3 net-new root causes — the StackSet drift surface is unimplemented, ListImports never reflects a live Fn::ImportValue, and an update emits no per-resource stack events, so bounded pollers never get an answer

> _Note: this issue was identified against the code base at commit `c7f894d9cab6281b7f6a51d1fbce512135c74b00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectStackSetDrift

## Description
This issue bundles 3 net-new root causes in the cloudformation emulator. All three share one visible symptom: a client that polls an asynchronous CloudFormation surface for a state real AWS reaches never observes it, so the call sequence spins until its own deadline instead of receiving either a result or a modeled refusal. Each cause below must be closed before this issue is submitted.

=== Cause 1 of 3: the StackSet drift-detection surface answers 501 NotImplemented end to end ===
Failing checks: cloudformation.detect-stackset-drift-lifecycle, cloudformation.list-stack-instance-resource-drifts, cloudformation.stack-set-drift-detection-lifecycle

AWS: a self-managed StackSet drift flow is three modeled operations and one aggregate field.
  * Smithy: CreateStackSet (input StackSetName/TemplateBody/PermissionModel/AdministrationRoleARN/ExecutionRoleName) answers 200 with StackSetId; its only modeled errors are NameAlreadyExistsException (409), CreatedButModifiedException (409) and LimitExceededException (409) — a well-formed SELF_MANAGED create of a fresh name is a success, never a server error.
  * Smithy: DetectStackSetDrift.StackSetName (StackSetNameOrId, pattern ^[a-zA-Z][-a-zA-Z0-9]*(?::[0-9a-f-]{36})?$, required) -> 200 with output member OperationId (ClientRequestToken, @length 1..128, pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$); modeled errors are only InvalidOperationException (400), OperationInProgressException (409) and StackSetNotFoundException (404). Its own model documentation states: "DetectStackSetDrift returns the OperationId of the StackSet drift detection operation. Use this operation id with DescribeStackSetOperation to monitor the progress of the drift detection operation."
  * Smithy: DescribeStackSetOperation(StackSetName, OperationId) -> 200 with StackSetOperation, whose Status reaches a terminal SUCCEEDED for a completed detection; modeled errors are only OperationNotFoundException (404) and StackSetNotFoundException (404).
  * Smithy: ListStackInstanceResourceDrifts(StackSetName, StackInstanceAccount, StackInstanceRegion, OperationId all required) -> 200 with Summaries; modeled errors are only OperationNotFoundException (404), StackInstanceNotFoundException (404) and StackSetNotFoundException (404). A bogus/unrelated OperationId is refused with OperationNotFoundException, not answered with the previous operation's summaries.
  * Smithy: DescribeStackSet -> StackSet.StackSetDriftDetectionDetails carries DriftStatus (DRIFTED once any instance resource diverges) and DriftedStackInstancesCount (>= 1 in that case).
So on AWS: create the StackSet, create an instance, change a resource out of band (here SQS SetQueueAttributes VisibilityTimeout 30 -> 111/222), DetectStackSetDrift, poll DescribeStackSetOperation to SUCCEEDED, then read DriftStatus == DRIFTED and the per-resource drift summaries.

Emulator: none of this surface is written. Every handler is still the scaffold that returns HTTP 501 with error code NotImplemented and the message "operation <OperationName> is scaffolded but not implemented in this emulator":
  * services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack_set.rs
  * services/service-com-amazonaws-cloudformation-cloudformation/src/ops/detect_stack_set_drift.rs
  * services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stack_set_operation.rs
  * services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_instance_resource_drifts.rs
Because CreateStackSet is the first call and real AWS makes a client retry it while the freshly created administration/execution roles propagate, all three checks wrap it in a bounded retry loop (90s / 120s) that swallows the refusal and retries. The emulator therefore produces no terminal outcome at all: the replay is killed by the harness ceiling ("Error: Check timed out after 60.0s") rather than surfacing the 501. The same is true one step further in: with CreateStackSet fixed, DetectStackSetDrift/DescribeStackSetOperation/ListStackInstanceResourceDrifts would still each answer 501.
Named for context, NOT a duplicate: the open issue 20261025T002357Z-cloudformation-createstackset-stub.md covers CreateStackSet alone, and 20261025T221744Z-cloudformation-liststacksetoperations-stub.md covers ListStackSetOperations alone. No open issue names DetectStackSetDrift, DescribeStackSetOperation, ListStackInstanceResourceDrifts or the StackSetDriftDetectionDetails aggregation, and no open issue records that the refusal is invisible to a retrying client.

Why: a client that drives StackSets — the AWS CLI, CDK pipelines, or any drift-governance tool — gets neither a StackSet nor a refusal it can classify. A 501 NotImplemented is not a modeled CloudFormation error, so botocore surfaces it as an opaque ClientError that retry logic written for AWS's propagation delay treats as retryable; the caller hangs for its whole deadline and then reports a timeout, hiding the real cause. Drift governance silently never reports DRIFTED.

Fix: implement the StackSet state and its drift operations in the cloudformation service — create_stack_set.rs (StackSet record, StackSetId, duplicate-name refusal), the stack-instance lifecycle it feeds, detect_stack_set_drift.rs (allocate and return an OperationId; refuse an unknown StackSet with StackSetNotFoundException and a concurrent detection with OperationInProgressException), describe_stack_set_operation.rs (drive the operation to a terminal SUCCEEDED and refuse an unknown OperationId with OperationNotFoundException), list_stack_instance_resource_drifts.rs (per-resource summaries keyed by the drift OperationId, OperationNotFoundException for an unrelated id), and the DescribeStackSet serializer so StackSetDriftDetectionDetails carries DriftStatus and DriftedStackInstancesCount computed from the instances' actual resource state.

Traces: cloudformation.detect-stackset-drift-lifecycle, cloudformation.list-stack-instance-resource-drifts, cloudformation.stack-set-drift-detection-lifecycle. In the replay report each is verdict [mismatch] with detail "Error: Check timed out after 60.0s"; the CreateStackSet retry loops are at report lines 14330-14346, 19011-19027 and 23107-23121, and the stub handlers are the four files cited above.

=== Cause 2 of 3: ListImports answers 501, so the cross-stack import index is never observable and the export-in-use guard cannot be exercised ===
Failing check: cloudformation.export-deletion-blocked-while-imported-rejected

AWS: Smithy: ListImports(ExportName required) -> 200 with Imports (a list of stack names), @paginated(inputToken=NextToken, outputToken=NextToken, items=Imports), and it models NO errors — for an export name that exists it is always a 200 whose Imports lists the stacks currently consuming it. After a stack whose template contains {"Fn::ImportValue": <ExportName>} reaches CREATE_COMPLETE, that stack's name appears in Imports for the exporting stack's export; after the importer is deleted it drops out again. That index is what makes the export-in-use guard observable: while it names an importer, deleting the exporting stack must not actually remove it (AWS either refuses the DeleteStack call with a ValidationError whose Message names the export and the importing stack, or accepts the call and leaves the stack where it was / settles it into DELETE_FAILED with a StackStatusReason naming the export and the importing stack); once it no longer names an importer, the byte-identical DeleteStack succeeds and the export disappears from ListExports.

Emulator: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_imports.rs is still the scaffold and answers HTTP 501 with error code NotImplemented and message "operation ListImports is scaffolded but not implemented in this emulator" for every call. The check polls ListImports until the importer shows up (300s budget) before it is willing to attempt the delete, catching and ignoring the failure as a not-yet-consistent index, so the replay never reaches the DeleteStack leg and is killed at the harness ceiling: "Error: Check timed out after 60.0s".
Named for context, NOT a duplicate: 20261025T133035Z-cloudformation-listimports-stub.md covers the ListImports stub itself. What is net-new here is the behavior that depends on it — that the import index is never populated from a deployed Fn::ImportValue and that the export-in-use guard on DeleteStack and the subsequent recovery (delete importer -> export drops out of the index -> exporter deletes cleanly -> export gone from ListExports) is therefore untested and unimplemented.

Why: any caller that reasons about cross-stack references before tearing down infrastructure — "who still imports this export?" — gets an unclassifiable 501 instead of a list, and an emulator that does not hold the import index can let a DeleteStack of an exporting stack succeed and silently break every importing stack's reference, which real AWS refuses.

Fix: implement list_imports.rs over the cloudformation export/import index, and populate that index when a stack template's Fn::ImportValue is resolved at deploy time and clear it when the importing stack is deleted; then enforce the export-in-use guard in the DeleteStack handler/state so the refusal message names both the export name and the importing stack name (fixed text as AWS words it for that class, with the export name and the importing stack name taken from the request and the emulator's own state — never a literal from one recording).

Traces: cloudformation.export-deletion-blocked-while-imported-rejected ([mismatch], "Error: Check timed out after 60.0s"; the ListImports poll is at report lines 16649-16659). Its sibling cloudformation.export-in-use-blocks-stack-deletion is already filed against the same stub and is deliberately excluded here.

=== Cause 3 of 3: an UpdateStack emits no per-resource stack events, so no client can observe which resource was updated before an update failed ===
Failing check: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip

AWS: Smithy: DescribeStackEvents(StackName required) -> 200 with StackEvents, @paginated(items=StackEvents), models no errors. Each StackEvent carries LogicalResourceId, ResourceStatus and ResourceStatusReason, and AWS records one event per resource per transition, newest first — so during an update that changes AWS::SQS::Queue "Q" and then fails on an unsignalled WaitCondition, the queue's own UPDATE_COMPLETE event is present in the stack's history before the failure, and the failing resource's UPDATE_FAILED event carries the reason. That per-resource timeline is the only way a client can tell that the queue was already updated when the rollback was denied, which is the precondition for the UPDATE_ROLLBACK_FAILED / ContinueUpdateRollback(ResourcesToSkip) recovery contract this check then exercises.

Emulator: the replay never gets past the bounded wait for the queue's own UPDATE_COMPLETE event (300s budget) and is killed at the harness ceiling: "Error: Check timed out after 60.0s". Hypothesis: DescribeStackEvents returns only stack-level events for the update, with no event whose LogicalResourceId is the queue's logical id, which is consistent with the per-resource-event gap already recorded for the CREATE path. The recording proves only that the event never appears within the poll window.
Named for context, NOT a duplicate: 20261025T053459Z-cloudformation-emits-no-per-resource-stack-events-and.md, 20261025T050651Z-cloudformation-records-no-per-resource-stack-events-a.md and 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md all describe the CREATE/rollback path. Net-new here is the UPDATE path: no per-resource UPDATE_COMPLETE/UPDATE_FAILED events are recorded for an UpdateStack, and the UPDATE_ROLLBACK_FAILED -> ContinueUpdateRollback(ResourcesToSkip=[...]) -> UPDATE_ROLLBACK_COMPLETE recovery that depends on them is unreachable.

Why: a client cannot distinguish "the update never touched the resource" from "the update changed it and then failed", so recovery tooling cannot decide which resources to skip; a CloudFormation console or CLI view of the update is blank where AWS shows a per-resource timeline.

Fix: the cloudformation stack-update engine and its event state — record one stack event per resource per transition for updates (UPDATE_IN_PROGRESS / UPDATE_COMPLETE / UPDATE_FAILED with its ResourceStatusReason) alongside the existing stack-level events, and have DescribeStackEvents serialize them newest-first; then honor ContinueUpdateRollback's ResourcesToSkip so a skipped resource is left exactly as the failed update made it rather than reverted.

Traces: cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip ([mismatch], "Error: Check timed out after 60.0s"; the per-resource-event wait is at report lines 10969-10981).

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, 117/411 passed. Of 225 distinct failing checks, 219 are marked ALREADY FILED or OWNED by an open issue and were skipped. The five checks below are the surviving, unowned failures clustered here.

Cause 1 — StackSet drift surface:
  Expected (AWS): CreateStackSet -> 200 {StackSetId}; DetectStackSetDrift -> 200 {OperationId}; DescribeStackSetOperation -> 200 with StackSetOperation.Status reaching SUCCEEDED; DescribeStackSet -> StackSet.StackSetDriftDetectionDetails.DriftStatus == "DRIFTED" with DriftedStackInstancesCount >= 1 after an out-of-band SQS SetQueueAttributes on an instance's queue; ListStackInstanceResourceDrifts -> 200 with per-resource Summaries, and OperationNotFoundException (404) for an unrelated OperationId.
  Actual (emulator): every one of create_stack_set.rs, detect_stack_set_drift.rs, describe_stack_set_operation.rs and list_stack_instance_resource_drifts.rs is still the scaffold and answers HTTP 501, error code NotImplemented, message "operation <OperationName> is scaffolded but not implemented in this emulator". Because the first call sits inside an AWS-shaped propagation retry loop, the replay surfaces no status at all: verdict [mismatch], detail "Error: Check timed out after 60.0s" for all three checks.

Cause 2 — ListImports / export-in-use:
  Expected (AWS): after the importing stack reaches CREATE_COMPLETE, ListImports(ExportName=<export>) -> 200 with Imports containing the importing stack's name (ListImports models no errors); DeleteStack on the exporting stack while that holds must not delete it (synchronous ValidationError naming the export and the importing stack, or the stack stays put / settles DELETE_FAILED with a StackStatusReason naming them); after the importer is deleted the index drops it and the identical DeleteStack succeeds and the export disappears from ListExports.
  Actual (emulator): ListImports answers HTTP 501, error code NotImplemented, message "operation ListImports is scaffolded but not implemented in this emulator" (services/.../src/ops/list_imports.rs is the scaffold). The import index is never populated from a deployed Fn::ImportValue, so the delete leg is never reached: verdict [mismatch], detail "Error: Check timed out after 60.0s".

Cause 3 — no per-resource events on update:
  Expected (AWS): DescribeStackEvents for the updating stack contains an event with LogicalResourceId == the queue's logical id and ResourceStatus == "UPDATE_COMPLETE" before the WaitCondition failure, then UPDATE_FAILED for the queue with a ResourceStatusReason, letting the stack settle into UPDATE_ROLLBACK_FAILED and be recovered with ContinueUpdateRollback(ResourcesToSkip=["Q"]) to UPDATE_ROLLBACK_COMPLETE with the skipped queue left at the updated value.
  Actual (emulator): no such per-resource event is ever returned within the 300s poll; verdict [mismatch], detail "Error: Check timed out after 60.0s". Hypothesis (neither the model nor the recording states it): DescribeStackEvents records only stack-level events for an update, matching the already-filed CREATE-path gap.

Note on values: the stack, StackSet, role, queue and export names in these checks are generated per run (ctx.unique_name), as are StackSetIds, OperationIds and timestamps. They fill the templates above and are not part of the requirement; the requirements are the statuses, the error codes, and the fixed text of each message with its variable parts taken from the request and the emulator's state.

## Steps to Reproduce
Prerequisite: git_clone(), then run_behavioral_suite() against the built emulator. All five checks are in service token `cloudformation` (bootstrapped).

Cause 1 (3 checks):
1. Replay cloudformation.detect-stackset-drift-lifecycle, cloudformation.list-stack-instance-resource-drifts and cloudformation.stack-set-drift-detection-lifecycle.
2. Each creates an administration role and an execution role in IAM, then calls CreateStackSet(StackSetName=<generated>, TemplateBody=<one AWS::SQS::Queue with VisibilityTimeout 30>, PermissionModel="SELF_MANAGED", AdministrationRoleARN=<admin role arn>, ExecutionRoleName=<exec role name>) inside a bounded retry loop.
3. Observe the emulator answers HTTP 501 NotImplemented for CreateStackSet on every attempt, so the loop never exits and the check is killed at the harness ceiling instead of reporting a status.
4. To see the rest of the surface directly, call DetectStackSetDrift, DescribeStackSetOperation and ListStackInstanceResourceDrifts with any arguments: each answers HTTP 501 NotImplemented from its scaffold handler.

Cause 2 (1 check):
1. Replay cloudformation.export-deletion-blocked-while-imported-rejected.
2. CreateStack an exporter whose Outputs export a value under a generated export name; wait for CREATE_COMPLETE.
3. CreateStack an importer whose AWS::SNS::Topic DisplayName is {"Fn::ImportValue": <that export name>}; wait for CREATE_COMPLETE.
4. Call ListImports(ExportName=<that export name>). Observe HTTP 501 NotImplemented instead of a 200 whose Imports names the importer stack — the check's import-registration poll never succeeds and it times out before it can attempt DeleteStack on the exporter.

Cause 3 (1 check):
1. Replay cloudformation.deep-cloudformation-1-cfn-enforce-continue-update-rollback-requires-resource-skip.
2. CreateStack (with a service RoleARN) a stack holding one AWS::SQS::Queue "Q" with VisibilityTimeout 30; wait for CREATE_COMPLETE.
3. UpdateStack to a template that sets VisibilityTimeout 60 on Q and adds an AWS::CloudFormation::WaitCondition (DependsOn Q, Timeout 90, never signalled).
4. Page DescribeStackEvents(StackName=<stack id>) and look for an event with LogicalResourceId "Q" and ResourceStatus "UPDATE_COMPLETE". Observe none is ever recorded, so the check's 300s wait for it never completes and the replay is killed at the harness ceiling.
