Issue-Source: trace-sweep-divergence
Issue-Trace: cfnrep-changeset-replacement-preview
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnrep-changeset-replacement-preview cloudformation-changeset-create-execute-workflow cloudformation-stack-create-update-delete-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.ExecutionStatus
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Status
Issue-Key: cloudformation|CreateChangeSet|2xx->4xx|$.CreateChangeSetResponse
Issue-Key: cloudformation|CreateChangeSet|2xx->4xx|status
Issue-Key: cloudformation|CreateChangeSet|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member.ResourceChange.Details
Issue-Key: cloudformation|DescribeChangeSet|2xx->2xx|$.DescribeChangeSetResponse.DescribeChangeSetResult.Changes.member.ResourceChange.Replacement
Issue-Key: cloudformation|CreateChangeSet|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: 6c57fad11ffc4b94eeb9d9eccf7474a9457dce26

# CloudFormation change sets: DescribeChangeSet returns FAILED with no Changes detail and CreateChangeSet mis-handles name reuse (5 causes)

> _Note: this issue was identified against the code base at commit `6c57fad11ffc4b94eeb9d9eccf7474a9457dce26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChangeSet

## Description
The emulator's change-set plane does not compute a change set: creation of a change set that forces resource replacement fails outright, the describe model carries no per-resource change detail, and the uniqueness/dedup rules for a change-set name are wrong. 5 distinct causes, all in cloudformation.

Cause 1 of 5 - DescribeChangeSet reports a created change set as FAILED/UNAVAILABLE with no Changes (fixture cfnrep-changeset-replacement-preview, step task#5).
AWS: for a change set whose template/parameter edit forces replacement of a resource, DescribeChangeSet returns Status=CREATE_COMPLETE, ExecutionStatus=AVAILABLE and a Changes list with one ResourceChange per affected resource (Action=Modify, Replacement=True/False, Scope, Details with Target/Evaluation/ChangeSource/CausingEntity). Smithy: DescribeChangeSet output members Status->ChangeSetStatus (values include CREATE_COMPLETE, FAILED), ExecutionStatus->ExecutionStatus (values include AVAILABLE, UNAVAILABLE), Changes->Changes list.
Emulator: Status='FAILED', ExecutionStatus='UNAVAILABLE' and Changes serialized as the empty string instead of a member list; the fixture's poll for CREATE_COMPLETE aborted because FAILED is terminal after 2 attempts.
Why: a client that creates a change set to preview a replacement can never reach ExecuteChangeSet - every change-set-gated deployment path (console-style preview, CI 'plan then apply') stops at a change set the emulator declares FAILED, and the preview it would have read is empty.
Fix: the change-set creation path in the CloudFormation handler and its state - the change set must be computed and stored with a terminal CREATE_COMPLETE/AVAILABLE status plus the computed Changes list, rather than recorded as FAILED.
Traces: cfnrep-changeset-replacement-preview#task#5 (also affects task#6, task#7 of the same fixture).

Cause 2 of 5 - CreateChangeSet answers 400 where AWS creates the change set (fixture cfnrep-changeset-replacement-preview, step task#10).
AWS: recorded behavior - CreateChangeSet returns 200 with CreateChangeSetResult.Id (changeSet ARN) and CreateChangeSetResult.StackId. Smithy: CreateChangeSet output {Id: ChangeSetId, StackId}.
Emulator: HTTP 400 with no CreateChangeSetResponse body at all.
Why: the client cannot create a change set for the stack, so no preview and no execution is possible; it sees a hard client error for a request AWS accepts.
Fix: CreateChangeSet's validation/uniqueness guard in the CloudFormation handler. Hypothesis: the emulator rejects the request because a change set of that name already existed earlier in the fixture's lifecycle, where AWS accepts the create.
Traces: cfnrep-changeset-replacement-preview#task#10.

Cause 3 of 5 - re-creating a change-set name with different attributes is reported as a plain name collision (fixture cfnrep-changeset-replacement-preview, step task#11).
AWS: when a change-set name is reused for a change set whose attributes differ from the existing one, the 400 names the mismatching attribute - recorded message, which is built from the request: 'ChangeSet <name> cannot be created due to a mismatch with existing attribute Parameters'.
Emulator: 400 whose message is 'ChangeSet [<name>] already exists' - the dedup key is the change-set name only, with no comparison of the submitted attributes.
Why: a client retrying a create with changed Parameters cannot distinguish an idempotent re-submit from an attribute conflict, so it retries forever or silently reuses a stale plan. HTTP status and error code agree here; the diverging part is which rule the emulator applied.
Fix: the change-set uniqueness comparison in CreateChangeSet's handler/state - compare the submitted attributes (Parameters included) against the stored change set, not just the name.
Traces: cfnrep-changeset-replacement-preview#task#11.

Cause 4 of 5 - ResourceChange.Details is empty and Replacement is absent (fixture cloudformation-changeset-create-execute-workflow, step task#9).
AWS: for a Modify action DescribeChangeSet populates ResourceChange.Replacement and ResourceChange.Details, a list of ResourceChangeDetail entries carrying Target{Attribute,Name,RequiresRecreation}, Evaluation and ChangeSource. Smithy: ResourceChange.Details -> ResourceChangeDetails list; ResourceChange.Replacement -> Replacement enum; ResourceChangeDetail members Target/Evaluation/ChangeSource/CausingEntity.
Emulator: Details serialized as the empty string (no member list) and Replacement missing entirely from the ResourceChange.
Why: the whole point of a change set is to tell the caller whether an update replaces a resource and which property caused it; with Replacement absent a client cannot gate a destructive update at all.
Fix: the DescribeChangeSet serializer plus the diff computation that fills ResourceChange in the change-set state.
Traces: cloudformation-changeset-create-execute-workflow#task#9 (also affects cloudformation-stack-create-update-delete-lifecycle#task#10 and #task#17).

Cause 5 of 5 - wrong error code for CreateChangeSet with ChangeSetType=CREATE on an existing stack (fixture cloudformation-changeset-create-execute-workflow, step task#12).
AWS: 400 with Error.Code='ValidationError'; recorded message, built from the request, 'Stack [<stack>] already exists and cannot be created again with the changeSet [<changeset>].'
Emulator: 400 with Error.Code='AlreadyExistsException' and message 'Stack [<stack>] already exists'. The status matches; the error code does not.
Why: clients branch on the error code. Code AlreadyExistsException is modeled for CreateChangeSet, so SDK error-matching treats this as a change-set collision and may retry with a new change-set name, where AWS reports an unrecoverable request-shape error.
Fix: the CreateChangeSet handler's existing-stack guard must raise the ValidationError code for this case instead of AlreadyExistsException.
Traces: cloudformation-changeset-create-execute-workflow#task#12.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

Cause 1 - cfnrep-changeset-replacement-preview#task#5 DescribeChangeSet, status 200 vs 200: Changes type_mismatch (expected a member list of two ResourceChange entries with Replacement True/False and Details, got ''); ExecutionStatus expected 'AVAILABLE', got 'UNAVAILABLE'; Status expected 'CREATE_COMPLETE', got 'FAILED'; poll: 'FAILED' is in fail_on after 2 attempt(s).
Cause 2 - cfnrep-changeset-replacement-preview#task#10 CreateChangeSet: status expected 200, got 400; $.CreateChangeSetResponse missing (expected CreateChangeSetResult with Id and StackId, got None).
Cause 3 - cfnrep-changeset-replacement-preview#task#11 CreateChangeSet, 400 vs 400: Error.Message expected 'ChangeSet <name> cannot be created due to a mismatch with existing attribute Parameters', got 'ChangeSet [<name>] already exists'.
Cause 4 - cloudformation-changeset-create-execute-workflow#task#9 DescribeChangeSet, 200 vs 200: Changes.member.ResourceChange.Details type_mismatch (expected member {Evaluation: Static, ChangeSource: DirectModification, Target: {RequiresRecreation: Never, Name: DisplayName, Attribute: Properties}}, got ''); Changes.member.ResourceChange.Replacement missing (expected 'False', got None).
Cause 5 - cloudformation-changeset-create-execute-workflow#task#12 CreateChangeSet, 400 vs 400: Error.Code expected 'ValidationError', got 'AlreadyExistsException'.

Recorded names/ARNs/ids above are examples from one recording; the requirements are the statuses, the error codes and the presence/values of the modeled members.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture cfnrep-changeset-replacement-preview: steps task#5 (DescribeChangeSet), task#10 and task#11 (CreateChangeSet).
3. Replay fixture cloudformation-changeset-create-execute-workflow: steps task#9 (DescribeChangeSet) and task#12 (CreateChangeSet with ChangeSetType=CREATE against the already-created stack).
4. Compare status, Error.Code and the DescribeChangeSet Status/ExecutionStatus/Changes members against the recorded responses. Also re-replay cloudformation-stack-create-update-delete-lifecycle#task#10 and #task#17 for cause 4.
