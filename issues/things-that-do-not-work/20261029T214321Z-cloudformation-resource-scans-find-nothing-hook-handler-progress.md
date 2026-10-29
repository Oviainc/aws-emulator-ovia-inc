Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.resource-scan-result-ttl-expiry cloudformation.start-resource-scan-in-progress-rejected cloudformation.hook-result-not-found-rejected cloudformation.hook-result-not-found-for-unhooked-resource cloudformation.handler-progress-invalid-operation-status-rejected cloudformation.stack-instance-and-refactor-not-found cloudformation.stack-instance-and-refactor-not-found-rejected cloudformation.organizations-access-toggle-lifecycle cloudformation.delete-stack-retain-resources-invalid-list-rejected cloudformation.delete-stack-duplicate-token-rejected cloudformation.import-existing-resources-adopts-named-parameter
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.resource-scan-result-ttl-expiry|run#13|AssertionError|assert any(
Issue-Key: cloudformation.start-resource-scan-in-progress-rejected|run#11|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: cloudformation.hook-result-not-found-rejected|run#10|AssertionError|assert list_error.get("Message") == "Invalid TargetType: STACK", (
Issue-Key: cloudformation|ListHookResults|ValidationError
Issue-Key: cloudformation.handler-progress-invalid-operation-status-rejected|run#6|AssertionError|assert error.get("Message") == expected_message, f"expected message {expected_message!r}, got {error!r}"
Issue-Key: cloudformation.stack-instance-and-refactor-not-found|run#13|AssertionError|assert code2 == "StackRefactorNotFoundException", (
Issue-Key: cloudformation.stack-instance-and-refactor-not-found-rejected|run#21|AssertionError|assert code3 == "StackRefactorNotFoundException", (
Issue-Key: cloudformation.delete-stack-retain-resources-invalid-list-rejected|run#14|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: cloudformation.delete-stack-duplicate-token-rejected|_client_error#17|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: cloudformation.import-existing-resources-adopts-named-parameter|run#19|AssertionError|assert changes == [("Import", "P", name)], (
Issue-Related: 20261025T015306Z-cloudformation-is-not-region-scoped-stack-names-and.md
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: resource scans find nothing, hook/handler-progress and scan refusal messages diverge, DescribeStackRefactor and DeleteStack(RetainResources) answer NotImplemented, DeleteStack is not blocked during an update, and ImportExistingResources never classifies an Import (9 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListResourceScanResources

## Description
Nine net-new behavioral root causes of service `cloudformation`, grounded in eleven failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 9: a completed resource scan reports no resources ===
AWS: recorded behavior only — after StartResourceScan reaches COMPLETE, the scan's result listing contains the account/region's existing resources, so a topic created just before the scan appears among the scanned AWS::SNS::Topic entries by its ARN.
Emulator: the scan completes and the AWS::SNS::Topic result set is empty ([]).
Why: resource scanning is the entry point of the whole IaC-generator flow (scan → generated template → managed stack). With an empty scan, every downstream step is vacuous and a client concludes the account holds no resources.
Fix: resource enumeration in the cloudformation service's resource-scan engine (the scan must walk the emulator's provisioned-resource state), surfaced through ListResourceScanResources/ListResourceScanRelatedResources.
Traces: cloudformation.resource-scan-result-ttl-expiry.

=== Cause 2 of 9: ResourceScanInProgressException's message is the emulator's own sentence ===
AWS: Smithy models StartResourceScan error `ResourceScanInProgressException` (errorKind client, httpError 400, awsQueryError ResourceScanInProgress/400). Message is recorded behavior only and is the fixed text `There is an active ResourceScan in progress. Please wait for it to finish before starting a new ResourceScan. Describe ResourceScans to see the status of the in-progress ResourceScan.` — no interpolation.
Emulator: same status/code, message `A resource scan is currently in progress. Only one can be run at a time for an account in a Region.`
Why: the message is the remediation instruction (it names DescribeResourceScans as the next call); clients and operators comparing against AWS read text AWS never emits.
Fix: the StartResourceScan single-flight refusal in the cloudformation service's resource-scan ops.
Traces: cloudformation.start-resource-scan-in-progress-rejected.

=== Cause 3 of 9: ListHookResults' unsupported-TargetType refusal is reworded ===
AWS: Smithy models ListHookResults.TargetType (ListHookResultsTargetType enum = [CHANGE_SET, STACK, RESOURCE, CLOUD_CONTROL]), so STACK and RESOURCE are valid wire values and the refusal is a runtime one: HTTP 400 `ValidationError` with Error.Message `Invalid TargetType: <value>` — fixed text `Invalid TargetType: ` plus the rejected enum value from the request.
Emulator: `TargetType <value> is not a supported filter option; the supported filter options are CHANGE_SET and CLOUD_CONTROL` — a different template that also enumerates the supported set.
Why: Message is the only discriminator (status and code match); taxonomy suites and CLI comparisons diverge for both STACK and RESOURCE.
Fix: the ListHookResults handler's TargetType guard in the cloudformation service's hook ops.
Traces: cloudformation.hook-result-not-found-rejected (STACK), cloudformation.hook-result-not-found-for-unhooked-resource (RESOURCE).

=== Cause 4 of 9: RecordHandlerProgress' enum-constraint message lists the wrong value set ===
AWS: Smithy models RecordHandlerProgress.OperationStatus as the `OperationStatus` enum, whose modelled values are [PENDING, IN_PROGRESS, SUCCESS, FAILED]. The refusal message is recorded behavior only: AWS answers HTTP 400 `ValidationError` with `1 validation error detected: Value '<value>' at 'operationStatus' failed to satisfy constraint: Member must satisfy enum value set: [CANCEL_IN_PROGRESS, SUCCESS, IN_PROGRESS, PENDING, FAILED, CANCEL_COMPLETE]` — the accepted set AWS reports is a superset of the modelled one and is rendered in that order.
Emulator: same status/code and the same sentence frame, but the set printed is `[PENDING, IN_PROGRESS, SUCCESS, FAILED]` — the model's values in the model's order.
Why: the value set in the message is the list a caller fixes its request from; a client told CANCEL_IN_PROGRESS/CANCEL_COMPLETE are not valid will never use the cancellation statuses AWS accepts.
Fix: the OperationStatus enum-constraint refusal for RecordHandlerProgress in the cloudformation service's input validation — render AWS's accepted set and order.
Traces: cloudformation.handler-progress-invalid-operation-status-rejected.

=== Cause 5 of 9: DescribeStackRefactor answers NotImplemented instead of its modelled not-found error ===
AWS: Smithy models DescribeStackRefactor with required input StackRefactorId and exactly one error, `StackRefactorNotFoundException` (errorKind client, httpError 404, awsQueryError StackRefactorNotFoundException/404). A StackRefactorId that names no refactor is refused with that error.
Emulator: answers Error.Code `NotImplemented` ('operation … is scaffolded but not implemented in this emulator'), a 5xx-class server refusal.
Why: a client cannot distinguish 'no such refactor' from 'the service is broken'; 404-vs-5xx also sends SDK retry layers down the wrong path, retrying a permanent condition. The already-filed stub issue for `CreateStackRefactor` (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/create_stack_refactor.rs) names a DIFFERENT operation and does not cover DescribeStackRefactor's refusal.
Fix: the DescribeStackRefactor handler in the cloudformation service's stack-refactor ops — resolve the id against stored refactors and raise the modelled StackRefactorNotFoundException.
Traces: cloudformation.stack-instance-and-refactor-not-found, cloudformation.stack-instance-and-refactor-not-found-rejected.

=== Cause 6 of 9: DescribeOrganizationsAccess answers NotImplemented where AWS refuses with ValidationError ===
AWS: recorded behavior only — a caller that is neither the organization management account nor a delegated administrator is refused with a client-class error (the check expects ValidationError / InvalidOperationException), not a server-class one.
Emulator: answers `{'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation DescribeOrganizationsAccess is scaffolded but not implemented in this emulator'}`.
Why: Type=Receiver marks the fault as the service's, so clients retry; AWS's refusal is a Sender-class permanent condition the caller must act on. The scaffold-stub issue already filed for DescribeOrganizationsAccess covers implementing the operation; this check is reported here because the replay report marks it neither ALREADY FILED nor OWNED, and it is listed so a fixer re-replays it after that stub lands.
Fix: the DescribeOrganizationsAccess handler (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_organizations_access.rs) — the CallAs/account-role gate must produce the client-class refusal.
Traces: cloudformation.organizations-access-toggle-lifecycle.

=== Cause 7 of 9: DeleteStack with RetainResources is an unimplemented branch ===
AWS: Smithy models DeleteStack.RetainResources (list of LogicalResourceId) as a first-class input member with only TokenAlreadyExistsException declared, so a RetainResources list naming resources in a state that may not be retained is refused with the service-common `ValidationError`, message `The specified resources to retain must be in a valid state. Do not specify resources that are in the DELETE_COMPLETE state.` (fixed text, no interpolation).
Emulator: answers Error.Code `NotImplemented` with 'operation DeleteStack with RetainResources is scaffolded but not implemented in this emulator' — the member is accepted by the model but the handler branch does not exist.
Why: RetainResources is how an operator deletes a stack while keeping specific resources; against the emulator the whole feature is unavailable and the refusal is misclassified as a service fault.
Fix: the DeleteStack handler's RetainResources branch in the cloudformation service's stack ops — validate the named logical ids against their resource states and retain them during the delete.
Traces: cloudformation.delete-stack-retain-resources-invalid-list-rejected.

=== Cause 8 of 9: DeleteStack is accepted while an update is in flight ===
AWS: recorded behavior only — DeleteStack issued with a fresh ClientRequestToken while the stack is mid-UPDATE is refused (the stack is not in a state that permits deletion); Smithy declares TokenAlreadyExistsException for the token-replay case, so this refusal is the service-common ValidationError.
Emulator: the call succeeds.
Why: a client can delete a stack out from under a running update, so the two operations race and the resulting stack/resource state is unreproducible; the guard that makes CloudFormation's single-writer model safe is absent.
Fix: the DeleteStack handler's stack-status guard in the cloudformation service's stack ops.
Traces: cloudformation.delete-stack-duplicate-token-rejected.

=== Cause 9 of 9: CreateChangeSet(ImportExistingResources=True) classifies an already-existing named resource as Add ===
AWS: Smithy models CreateChangeSet.ImportExistingResources (boolean) and ResourceChange.Action as a ChangeAction enum that includes `Import`; recorded behavior is that with ImportExistingResources=True a resource that already exists under its explicitly named physical id is classified Action='Import' and the change carries that PhysicalResourceId.
Emulator: the change comes back as `('Add', 'P', None)` — Action='Add' with no PhysicalResourceId, i.e. the flag is ignored and the existing resource would be created again.
Why: executing that change set on AWS would adopt the resource; executing the emulator's plan would try to create a resource that already exists and fail (or clobber it). Change-set review — the entire point of the operation — shows the wrong action for every adopted resource.
Fix: the ImportExistingResources branch of the CreateChangeSet change-computation in the cloudformation service's change-set ops — look the named physical resource up and emit Action='Import' with its PhysicalResourceId.
Traces: cloudformation.import-existing-resources-adopts-named-parameter.

## Evidence
Cause 1 — cloudformation.resource-scan-result-ttl-expiry [mismatch]:
  AssertionError: expected arn:aws:sns:us-west-2:000000000000:sim2real-behav-scan-topic-3fcd149951d5 among scanned AWS::SNS::Topic resources, got []

Cause 2 — cloudformation.start-resource-scan-in-progress-rejected [mismatch]:
  AssertionError: expected message 'There is an active ResourceScan in progress. Please wait for it to finish before starting a new ResourceScan. Describe ResourceScans to see the status of the in-progress ResourceScan.', got 'A resource scan is currently in progress. Only one can be run at a time for an account in a Region.'
  Smithy: StartResourceScan errors = ResourceScanInProgressException (client, 400, awsQueryError ResourceScanInProgress/400), ResourceScanLimitExceededException (client, 400).

Cause 3 — cloudformation.hook-result-not-found-rejected [mismatch]:
  AssertionError: expected message 'Invalid TargetType: STACK', got {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'TargetType STACK is not a supported filter option; the supported filter options are CHANGE_SET and CLOUD_CONTROL'}
  cloudformation.hook-result-not-found-for-unhooked-resource [mismatch]: same with RESOURCE.
  Smithy: ListHookResultsTargetType enum values = [CHANGE_SET, STACK, RESOURCE, CLOUD_CONTROL]; ListHookResults errors = HookResultNotFoundException (client, 404, HookResultNotFound/404).

Cause 4 — cloudformation.handler-progress-invalid-operation-status-rejected [mismatch]:
  AssertionError: expected message "1 validation error detected: Value 'BOGUS_STATUS' at 'operationStatus' failed to satisfy constraint: Member must satisfy enum value set: [CANCEL_IN_PROGRESS, SUCCESS, IN_PROGRESS, PENDING, FAILED, CANCEL_COMPLETE]", got {'Type': 'Sender', 'Code': 'ValidationError', 'Message': "1 validation error detected: Value 'BOGUS_STATUS' at 'operationStatus' failed to satisfy constraint: Member must satisfy enum value set: [PENDING, IN_PROGRESS, SUCCESS, FAILED]"}
  Smithy: OperationStatus enum values = [PENDING, IN_PROGRESS, SUCCESS, FAILED] (the model's set is narrower than the set AWS reports in the message); RecordHandlerProgress errors = InvalidStateTransitionException (400/InvalidStateTransition), OperationStatusCheckFailedException (400/ConditionalCheckFailed).

Cause 5 — cloudformation.stack-instance-and-refactor-not-found [mismatch]:
  AssertionError: DescribeStackRefactor(StackRefactorId=<unknown>): expected StackRefactorNotFoundException, got 'NotImplemented'
  cloudformation.stack-instance-and-refactor-not-found-rejected [mismatch]:
  AssertionError: DescribeStackRefactor with a never-created StackRefactorId: expected StackRefactorNotFoundException, got 'NotImplemented'
  Smithy: DescribeStackRefactor input {StackRefactorId required}, output {Description, StackRefactorId, StackIds, ExecutionStatus, ExecutionStatusReason, Status, StatusReason}, errors = [StackRefactorNotFoundException, client, httpError 404, awsQueryError StackRefactorNotFoundException/404].

Cause 6 — cloudformation.organizations-access-toggle-lifecycle [mismatch]:
  AssertionError: expected a non-management/non-delegated-admin account to be refused with ValidationError on DescribeOrganizationsAccess, got 'NotImplemented'

Cause 7 — cloudformation.delete-stack-retain-resources-invalid-list-rejected [mismatch]:
  AssertionError: expected ('ValidationError', 'The specified resources to retain must be in a valid state. Do not specify resources that are in the DELETE_COMPLETE state.'), got ('NotImplemented', 'operation DeleteStack with RetainResources is scaffolded but not implemented in this emulator')
  Smithy: DeleteStack input {StackName required, RetainResources (list), RoleARN, ClientRequestToken, DeletionMode, DeploymentConfig}, errors = [TokenAlreadyExistsException (client, 400)].

Cause 8 — cloudformation.delete-stack-duplicate-token-rejected [mismatch]:
  AssertionError: expected DeleteStack with a fresh token during an update to be rejected, but the call succeeded

Cause 9 — cloudformation.import-existing-resources-adopts-named-parameter [mismatch]:
  AssertionError: an existing named parameter must be classified Import with its physical id; got [('Add', 'P', None)]
  Smithy: CreateChangeSet.ImportExistingResources (boolean); ResourceChange.Action is ChangeAction, documented to include Import; ResourceChange.PhysicalResourceId carries the resource name.

## Steps to Reproduce
Cause 1: create an AWS::SNS::Topic, StartResourceScan, poll to COMPLETE, then list the scan's AWS::SNS::Topic resources — observe [].
Cause 2: StartResourceScan while a scan is in progress; compare Error.Message against AWS's fixed sentence.
Cause 3: ListHookResults(TargetType='STACK', TargetId=<a stack arn>) and again with TargetType='RESOURCE'; observe the reworded ValidationError instead of 'Invalid TargetType: STACK' / 'Invalid TargetType: RESOURCE'.
Cause 4: RecordHandlerProgress(BearerToken=<any>, OperationStatus='BOGUS_STATUS'); compare the enum value set printed in the message.
Cause 5: DescribeStackRefactor(StackRefactorId=<a uuid never issued>); observe NotImplemented instead of 404 StackRefactorNotFoundException.
Cause 6: DescribeOrganizationsAccess(CallAs='DELEGATED_ADMIN') from an account that is neither management nor delegated admin; observe Type=Receiver / Code=NotImplemented.
Cause 7: DeleteStack(StackName=<stack>, RetainResources=['SomeLogicalId']); observe NotImplemented instead of the ValidationError above.
Cause 8: start an UpdateStack that stays IN_PROGRESS (e.g. behind an unsignalled WaitCondition), then DeleteStack(StackName=<stack>, ClientRequestToken=<fresh>); observe success instead of a refusal.
Cause 9: create a resource with an explicit physical name out of band, then CreateChangeSet(ImportExistingResources=True) for a template declaring that same named resource; observe Action='Add' with PhysicalResourceId None instead of Action='Import' with the physical id.
Re-replay all eleven check ids listed in trace_ids.
