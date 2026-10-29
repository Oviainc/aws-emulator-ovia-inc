Issue-Source: trace-sweep-divergence
Issue-Trace: cfnerr-stackset-operation-id-reuse
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnerr-stackset-operation-id-reuse cloudformation-stackset-drift-and-import cfnerr-stackset-not-empty cfnerr-stackset-name-already-exists cloudformation-stackset-core-lifecycle cloudformation-stackset-multi-account-region-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|UpdateStackSet|2xx->4xx|$.UpdateStackSetResponse
Issue-Key: cloudformation|UpdateStackSet|2xx->4xx|status
Issue-Key: cloudformation|DetectStackSetDrift|2xx->4xx|$.DetectStackSetDriftResponse
Issue-Key: cloudformation|DetectStackSetDrift|2xx->4xx|status
Issue-Key: cloudformation|ListStackInstances|2xx->2xx|$
Issue-Key: cloudformation|ListStackInstances|2xx->2xx|$.ListStackInstancesResponse.ListStackInstancesResult.Summaries.member.OrganizationalUnitId
Issue-Key: cloudformation|ListStackInstances|2xx->2xx|$.ListStackInstancesResponse.ListStackInstancesResult.Summaries.member.StackInstanceStatus.DetailedStatus
Issue-Key: cloudformation|ListStackInstances|2xx->2xx|$.ListStackInstancesResponse.ListStackInstancesResult.Summaries.member.Status
Issue-Key: cloudformation|ListStackInstances|2xx->2xx|$.ListStackInstancesResponse.ListStackInstancesResult.Summaries.member.StatusReason
Issue-Key: cloudformation|CreateStackSet|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|DescribeStackSet|2xx->2xx|$.DescribeStackSetResponse.DescribeStackSetResult.StackSet.AdministrationRoleARN
Issue-Key: cloudformation|DescribeStackSet|2xx->2xx|$.DescribeStackSetResponse.DescribeStackSetResult.StackSet.Capabilities
Issue-Key: cloudformation|DescribeStackSet|2xx->2xx|$.DescribeStackSetResponse.DescribeStackSetResult.StackSet.OrganizationalUnitIds
Issue-Key: cloudformation|DescribeStackSet|2xx->2xx|$.DescribeStackSetResponse.DescribeStackSetResult.StackSet.Parameters
Issue-Key: cloudformation|DescribeStackSet|2xx->2xx|$.DescribeStackSetResponse.DescribeStackSetResult.StackSet.Regions
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->2xx|$.DescribeStackSetOperationResponse.DescribeStackSetOperationResult.StackSetOperation.AdministrationRoleARN
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->2xx|$.DescribeStackSetOperationResponse.DescribeStackSetOperationResult.StackSetOperation.OperationPreferences
Issue-Key: cloudformation|DescribeStackSetOperation|2xx->2xx|$.DescribeStackSetOperationResponse.DescribeStackSetOperationResult.StackSetOperation.StatusDetails.TotalAccountsCount
Issue-Key: cloudformation|UpdateStackSet|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudformation|ListStackSetOperations|2xx->2xx|$.ListStackSetOperationsResponse.ListStackSetOperationsResult.Summaries.member.OperationPreferences
Issue-Key: cloudformation|ListStackSetOperations|2xx->2xx|$.ListStackSetOperationsResponse.ListStackSetOperationsResult.Summaries.member.StatusDetails.TotalAccountsCount
Issue-Key: cloudformation|StopStackSetOperation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudformation StackSet operations refuse wrongly and StackSet summaries omit modelled members

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStackSet

## Description
The StackSet surface diverges in two ways at once: two operations refuse requests AWS accepts (UpdateStackSet with a fresh OperationId, DetectStackSetDrift), and every StackSet read model drops members a client needs — plus four refusals are written in the emulator's words. 9 net-new root causes, one service.

The replay reported owned_by=[] and no stub for all nine; UpdateStackSet answered 409/400 (it is serving), so the open stub issue "Implement cloudformation UpdateStackSet: unimplemented scaffold stub" does not cover cause 1. The open issues "cloudformation__events__sqs: StackSet operations never settle ..." and "cloudformation__events__sqs: a SELF_MANAGED StackSet operation never reaches a terminal status ..." are behavioral neighbours and carry none of the fixture#step bindings below.

Cause 1 of 9: UpdateStackSet refuses a second operation on the same StackSet with 409.
AWS: Smithy: UpdateStackSet output OperationId; its refusals are modelled errors (OperationIdAlreadyExistsException, OperationInProgressException, ...). An UpdateStackSet issued after the previous operation on that StackSet has reached a terminal status, carrying a NEW OperationId, returns 200 with UpdateStackSetResult.OperationId.
Emulator: status 409 and no UpdateStackSetResponse at all.
Why: a StackSet can be updated exactly once — every subsequent update of the same StackSet is rejected, so no StackSet lifecycle beyond the first update works.
Fix: the UpdateStackSet handler's in-progress/operation-id conflict gate and the StackSet operation state it consults.
Traces: cfnerr-stackset-operation-id-reuse#task#3.

Cause 2 of 9: DetectStackSetDrift refuses with 400.
AWS: Smithy: DetectStackSetDrift output OperationId; for an existing StackSet with instances it returns 200 with DetectStackSetDriftResult.OperationId.
Emulator: status 400 with no DetectStackSetDriftResponse.
Why: StackSet drift detection cannot be started at all, so the drift/import workflow has no entry point.
Fix: the DetectStackSetDrift handler.
Traces: cloudformation-stackset-drift-and-import#task#2.

Cause 3 of 9: a stack instance whose administration role does not exist still succeeds.
AWS: Smithy: StackInstanceSummary members Status (StackInstanceStatus: OUTDATED when the associated stack failed during a Create/UpdateStackSet operation), StatusReason (Reason), StackInstanceStatus.DetailedStatus and OrganizationalUnitId. When the StackSet's administration role is absent the recorded instance ends Status OUTDATED with DetailedStatus FAILED and StatusReason "Account <account id> should have '<administration role name>' role with trust relationship to CloudFormation service." — fixed text apart from the account id and the role name taken from the request/state.
Emulator: the instance ends Status CURRENT with DetailedStatus SUCCEEDED, carries a StackId, and omits StatusReason and OrganizationalUnitId; the recorded expectation is never reached within the 60 s poll.
Why: a misconfigured StackSet looks healthy — the one failure mode StackSets exist to report is reported as success, and the diagnostic StatusReason is absent.
Fix: the stack-instance provisioning path (administration-role resolution) and the ListStackInstances summary serializer.
Traces: cfnerr-stackset-not-empty#task#3.

Cause 4 of 9: CreateStackSet's already-exists message is the emulator's.
AWS: the duplicate-name refusal is 409 with Message 'StackSet already exists.' — fixed text with no variable part.
Emulator: 409 with 'StackSet <name> already exists' (interpolating the requested name and dropping the period).
Why: the message is a requirement of this error class; a client matching AWS's text does not match.
Fix: the CreateStackSet validation's error message.
Traces: cfnerr-stackset-name-already-exists#task#2.

Cause 5 of 9: DescribeStackSet omits five modelled members.
AWS: Smithy: StackSet members Regions (RegionList), Capabilities (Capabilities), AdministrationRoleARN (RoleARN), Parameters (Parameters) and OrganizationalUnitIds (OrganizationalUnitIdList). All five are present in the recorded 200 body — the list members as empty lists when the StackSet has none, and AdministrationRoleARN as the StackSet's administration role ARN (the default AWSCloudFormationStackSetAdministrationRole role ARN in the caller's account when none was supplied).
Emulator: all five members absent.
Why: a client cannot read back which regions a StackSet deploys to, which capabilities it was granted or which role administers it, so a describe-then-update round trip loses configuration.
Fix: the DescribeStackSet response serializer and the StackSet state that must retain these fields.
Traces: cfnerr-stackset-name-already-exists#task#3, cloudformation-stackset-core-lifecycle#task#2, cloudformation-stackset-multi-account-region-lifecycle#task#2.

Cause 6 of 9: DescribeStackSetOperation omits AdministrationRoleARN, OperationPreferences and the account counters.
AWS: Smithy: StackSetOperation members AdministrationRoleARN, OperationPreferences (StackSetOperationPreferences, carrying RegionOrder) and StatusDetails (StackSetOperationStatusDetails, carrying FailedStackInstancesCount/TotalAccountsCount). The recorded body carries all three, with StatusDetails.TotalAccountsCount present (0 when the operation touched no accounts) and OperationPreferences carrying its RegionOrder member.
Emulator: AdministrationRoleARN, OperationPreferences and StatusDetails.TotalAccountsCount absent.
Why: a client polling an operation cannot read how many accounts it covers or which preferences it ran with.
Fix: the DescribeStackSetOperation response serializer and the operation record in StackSet state.
Traces: cfnerr-stackset-operation-id-reuse#task#4, cloudformation-stackset-core-lifecycle#task#6, cloudformation-stackset-core-lifecycle#task#7, cloudformation-stackset-core-lifecycle#task#9.

Cause 7 of 9: ListStackSetOperations summaries omit the same members.
AWS: as cause 6 — the recorded summaries carry StatusDetails.TotalAccountsCount and OperationPreferences (with RegionOrder).
Emulator: both absent.
Why/Fix: as cause 6, in the ListStackSetOperations summary serializer (a separate serializer, so fixing DescribeStackSetOperation alone does not fix it).
Traces: cloudformation-stackset-core-lifecycle#task#8.

Cause 8 of 9: UpdateStackSet's duplicate-operation-id message is the emulator's.
AWS: the OperationIdAlreadyExists refusal is 409 with 'Operation with Id <OperationId from the request> already exists' — fixed text apart from the supplied operation id.
Emulator: 409 with 'Operation id <id> already exists'.
Why: the idempotency-collision message a client surfaces/matches differs from AWS's.
Fix: the UpdateStackSet operation-id validation's error message.
Traces: cfnerr-stackset-operation-id-reuse#task#5.

Cause 9 of 9: StopStackSetOperation's wrong-status message is the emulator's.
AWS: stopping an operation that is already terminal is refused 400 with 'Cannot Stop since Operation is in <current StackSetOperationStatus>' — fixed text apart from the status taken from state.
Emulator: 400 with 'StackSet operation <operation id> is in <status> state and cannot be stopped'.
Why: the message is a requirement of this error class; the emulator also leaks the operation id into text AWS does not include.
Fix: the StopStackSetOperation status gate's error message.
Traces: cloudformation-stackset-core-lifecycle#task#11.

## Evidence
Replay of simparity-wire-aws-v1, 9 mismatch clusters:
- cfnerr-stackset-operation-id-reuse#task#3 UpdateStackSet: status expected 200, got 409; $.UpdateStackSetResponse missing (expected UpdateStackSetResult.OperationId).
- cloudformation-stackset-drift-and-import#task#2 DetectStackSetDrift: status expected 200, got 400; $.DetectStackSetDriftResponse missing (expected DetectStackSetDriftResult.OperationId).
- cfnerr-stackset-not-empty#task#3 ListStackInstances 200: Summaries.member.Status value_mismatch (expected 'OUTDATED', got 'CURRENT'); .StackInstanceStatus.DetailedStatus value_mismatch (expected 'FAILED', got 'SUCCEEDED'); .StatusReason missing (expected "Account 000000000000 should have '<admin role>' role with trust relationship to CloudFormation service."); .OrganizationalUnitId missing; poll capped at 60 s.
- cfnerr-stackset-name-already-exists#task#2 CreateStackSet 409: Message expected 'StackSet already exists.', got 'StackSet <name> already exists'.
- cfnerr-stackset-name-already-exists#task#3 DescribeStackSet 200: StackSet.Regions, .Capabilities, .Parameters, .OrganizationalUnitIds missing; .AdministrationRoleARN missing (expected 'arn:aws:iam::000000000000:role/AWSCloudFormationStackSetAdministrationRole') — 3 affected steps.
- cfnerr-stackset-operation-id-reuse#task#4 DescribeStackSetOperation 200: StackSetOperation.AdministrationRoleARN missing, .OperationPreferences missing (expected {RegionOrder: ''}), .StatusDetails.TotalAccountsCount missing (expected '0') — 4 affected steps.
- cloudformation-stackset-core-lifecycle#task#8 ListStackSetOperations 200: Summaries.member.StatusDetails.TotalAccountsCount missing, .OperationPreferences missing.
- cfnerr-stackset-operation-id-reuse#task#5 UpdateStackSet 409: Message expected 'Operation with Id <id> already exists', got 'Operation id <id> already exists'.
- cloudformation-stackset-core-lifecycle#task#11 StopStackSetOperation 400: Message expected 'Cannot Stop since Operation is in SUCCEEDED', got 'StackSet operation <id> is in SUCCEEDED state and cannot be stopped'.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. CreateStackSet, run one UpdateStackSet to completion, then issue a second UpdateStackSet with a new OperationId — the emulator answers 409 with no body instead of 200 + OperationId.
3. Call DetectStackSetDrift on an existing StackSet with instances — 400 instead of 200 + OperationId.
4. Create a StackSet whose administration role does not exist, create an instance, then poll ListStackInstances — the instance reports CURRENT/SUCCEEDED with no StatusReason instead of OUTDATED/FAILED with the role-trust StatusReason.
5. CreateStackSet with a duplicate name; DescribeStackSet; DescribeStackSetOperation and ListStackSetOperations for a completed operation; UpdateStackSet reusing a prior OperationId; StopStackSetOperation on a SUCCEEDED operation — compare members and messages against the recorded bodies.
