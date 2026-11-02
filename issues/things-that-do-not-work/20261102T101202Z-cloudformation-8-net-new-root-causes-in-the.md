Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.change-set-client-token-retry-same-id-other-token-rejected cloudformation.change-set-on-stack-failure-delete-on-update-type-rejected cloudformation.change-set-on-stack-failure-with-disable-rollback-rejected cloudformation.change-set-resource-types-excluding-a-template-type-rejected cloudformation.change-set-stack-tag-change-scopes-tags-not-properties cloudformation.change-set-include-property-values-add-modify-remove-contexts cloudformation.change-set-execute-client-request-token-marks-only-its-events cloudformation.cdk-deploy-method-direct-creates-and-updates-without-a-change-set
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.change-set-client-token-retry-same-id-other-token-rejected|_error#13|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: cloudformation.change-set-on-stack-failure-delete-on-update-type-rejected|_error#7|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: cloudformation.change-set-on-stack-failure-with-disable-rollback-rejected|run#11|AssertionError|assert got == (
Issue-Key: cloudformation.change-set-resource-types-excluding-a-template-type-rejected|_error#16|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {result!r}")
Issue-Key: cloudformation.change-set-stack-tag-change-scopes-tags-not-properties|run#11|AssertionError|assert cs["Status"] == "CREATE_COMPLETE", f"change set: {cs.get('StatusReason')!r}"
Issue-Key: cloudformation.change-set-include-property-values-add-modify-remove-contexts|run#18|KeyError|assert json.loads(mod["BeforeContext"]) == props(keep, "k1"), mod.get("BeforeContext")
Issue-Key: cloudformation.change-set-execute-client-request-token-marks-only-its-events|run#20|AssertionError|assert [(lid, st) for lid, st, _ in new] == [
Issue-Key: cloudformation.cdk-deploy-method-direct-creates-and-updates-without-a-change-set|_err#18|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation: 8 net-new root causes in the change-set surface — unenforced guards, missing property contexts and wrong refusal messages

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Eight distinct net-new root causes in CreateChangeSet/DescribeChangeSet/ExecuteChangeSet. Open issues 20261029T214321Z-cloudformation-the-change-set-status-surface-diverges-delete.md and 20261025T015306Z-cloudformation-refusal-taxonomy-diverges-wrong-not-found-validation.md cover neighbouring change-set symptoms but carry none of the replay keys below.

Cause 1 of 8: a repeat CreateChangeSet for the same ChangeSetName with a different ClientToken is accepted.
AWS: Smithy: CreateChangeSet takes ClientToken (length 1..128) and models AlreadyExistsException (400, awsQueryError AlreadyExistsException); a retry of the same change-set name under a different token is a new creation of an existing name and is refused.
Emulator: the second call returns 200 with a fresh Id/StackId.
Why: a client's retry silently replaces the change set it meant to be idempotent about, and two change sets share one name.
Fix: idempotency/uniqueness check in the CreateChangeSet handler.
Traces: cloudformation.change-set-client-token-retry-same-id-other-token-rejected.

Cause 2 of 8: OnStackFailure on a ChangeSetType=UPDATE change set is accepted.
AWS: Smithy: CreateChangeSet has both ChangeSetType (enum) and OnStackFailure (enum); OnStackFailure is only valid for ChangeSetType=CREATE and the UPDATE combination is refused with ValidationError.
Emulator: the call returns 200 with an Id/StackId.
Why: the caller believes a failure disposition is in force for an update that will never honour it.
Fix: input validation in the CreateChangeSet handler.
Traces: cloudformation.change-set-on-stack-failure-delete-on-update-type-rejected.

Cause 3 of 8: ExecuteChangeSet(DisableRollback) over an OnStackFailure change set is refused with the wrong message.
AWS: the refusal is ValidationError with the fixed text 'Either DisableRollback or OnStackFailure can be specified, but not both. Validation error as OnStackFailure parameter was specified during CreateChangeSet.' (no variable parts).
Emulator: ValidationError 'DisableRollback cannot be specified when the change set was created with OnStackFailure'.
Why: clients that branch on the message text do not recognise the refusal.
Fix: the ExecuteChangeSet handler's validation message.
Traces: cloudformation.change-set-on-stack-failure-with-disable-rollback-rejected.

Cause 4 of 8: the ResourceTypes allow-list is not enforced on CreateChangeSet.
AWS: Smithy: CreateChangeSet.ResourceTypes is a list of permitted resource types; a template containing a type outside the list is refused with InsufficientCapabilitiesException/ValidationError before the change set is created.
Emulator: the call returns 200 and the change set is created.
Why: a least-privilege guard a caller relies on is inert.
Fix: ResourceTypes enforcement in the CreateChangeSet handler's validation.
Traces: cloudformation.change-set-resource-types-excluding-a-template-type-rejected.

Cause 5 of 8: a tags-only change set is reported as containing no changes.
AWS: recorded behavior only — changing only the stack's Tags produces a successful change set whose ResourceChange entries carry Scope ['Tags'] (Smithy: ResourceChange.Scope documents Metadata/Properties/Tags as the triggering attribute).
Emulator: the change set ends FAILED with StatusReason "The submitted information didn't contain changes. Submit different information to create a change set."
Why: tag-only deployments are impossible through change sets.
Fix: the change-set differ (tag scope) in the CreateChangeSet handler.
Traces: cloudformation.change-set-stack-tag-change-scopes-tags-not-properties.

Cause 6 of 8: DescribeChangeSet(IncludePropertyValues=true) omits BeforeContext/AfterContext.
AWS: Smithy: DescribeChangeSet input has IncludePropertyValues; ResourceChange has members BeforeContext ('An encoded JSON string that contains the context of the resource before the change is executed') and AfterContext; Add changes carry AfterContext, Remove changes BeforeContext, Modify both.
Emulator: the response has no BeforeContext key at all (KeyError: 'BeforeContext').
Why: a client cannot see the property values a change will apply, which is the entire purpose of the flag.
Fix: the DescribeChangeSet serializer/change-set state.
Traces: cloudformation.change-set-include-property-values-add-modify-remove-contexts.

Cause 7 of 8: the event stream of an executed change set is malformed.
AWS: recorded behavior only — executing a change set emits, per resource, CREATE_IN_PROGRESS (no reason), CREATE_IN_PROGRESS with reason 'Resource creation Initiated', then CREATE_COMPLETE, and the stack ends UPDATE_COMPLETE_CLEANUP_IN_PROGRESS then UPDATE_COMPLETE; every event carries the execute call's ClientRequestToken.
Emulator: each resource emits CREATE_IN_PROGRESS twice with no 'Resource creation Initiated' reason, and the stack goes straight from UPDATE_IN_PROGRESS to UPDATE_COMPLETE with no UPDATE_COMPLETE_CLEANUP_IN_PROGRESS event.
Why: tooling that keys on the 'Resource creation Initiated' marker or the cleanup phase mis-reads progress.
Fix: event emission in the ExecuteChangeSet/update path.
Traces: cloudformation.change-set-execute-client-request-token-marks-only-its-events.

Cause 8 of 8: a direct (non-change-set) deploy leaves a change set behind.
AWS: recorded behavior only — `cdk deploy --method=direct` calls CreateStack/UpdateStack with no change set, so DescribeChangeSet for the name is refused with ChangeSetNotFound (Smithy: ChangeSetNotFoundException, httpError 404, awsQueryError ChangeSetNotFound) and the stack's ChangeSetId stays absent.
Emulator: DescribeChangeSet succeeds and returns a change set with ExecutionStatus EXECUTE_COMPLETE/Status CREATE_COMPLETE for a deploy that used no change set.
Why: a client cannot distinguish a direct deploy from a change-set deploy, and change-set-scoped cleanup never sees these objects.
Fix: the direct CreateStack/UpdateStack path must not synthesise a change-set record.
Traces: cloudformation.cdk-deploy-method-direct-creates-and-updates-without-a-change-set.

## Evidence
Cause 1: 'expected a refusal, the call succeeded: {Id: arn:...:changeSet/<name>/<uuid>, StackId: ..., HTTPStatusCode: 200}'.
Cause 2: 'expected a refusal, the call succeeded: {Id: ..., StackId: ..., HTTPStatusCode: 200}'.
Cause 3: expected ('ValidationError', 'Either DisableRollback or OnStackFailure can be specified, but not both. Validation error as OnStackFailure parameter was specified during CreateChangeSet.'), got ('ValidationError', 'DisableRollback cannot be specified when the change set was created with OnStackFailure').
Cause 4: 'expected a refusal, the call succeeded: {Id: ..., HTTPStatusCode: 200}'.
Cause 5: change set status FAILED, StatusReason "The submitted information didn't contain changes. Submit different information to create a change set."
Cause 6: KeyError: 'BeforeContext'.
Cause 7: recorded sequence [(stack,'UPDATE_IN_PROGRESS',tok),('One','CREATE_IN_PROGRESS',tok),('Two','CREATE_IN_PROGRESS',tok),('One','CREATE_IN_PROGRESS',tok),('Two','CREATE_IN_PROGRESS',tok),('One','CREATE_COMPLETE',tok),('Two','CREATE_COMPLETE',tok),(stack,'UPDATE_COMPLETE',tok)].
Cause 8: DescribeChangeSet returned 200 with ExecutionStatus 'EXECUTE_COMPLETE', Status 'CREATE_COMPLETE', Description 'CDK Changeset for execution <uuid>'.
Ids, names and timestamps above are per-run values filling the template.

## Steps to Reproduce
Replay the checks in trace_ids: 1) CreateChangeSet twice with the same ChangeSetName and different ClientToken; 2) CreateChangeSet with ChangeSetType=UPDATE and OnStackFailure=DELETE; 3) CreateChangeSet(OnStackFailure=DO_NOTHING) then ExecuteChangeSet(DisableRollback=True); 4) CreateChangeSet with ResourceTypes excluding a type present in the template; 5) CreateChangeSet that changes only Tags; 6) DescribeChangeSet(IncludePropertyValues=True) on an Add/Modify/Remove change set; 7) ExecuteChangeSet with a ClientRequestToken and read DescribeStackEvents; 8) `cdk deploy --method=direct` then DescribeChangeSet for the CDK change-set name.
