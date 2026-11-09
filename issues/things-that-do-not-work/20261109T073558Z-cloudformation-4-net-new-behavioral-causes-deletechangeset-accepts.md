Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.changeset-delete-invalid-status-rejected cloudformation.stack-rollback-after-create-failed-lifecycle cloudformation.type-registry-not-found-errors cloudformation.delete-stack-duplicate-token-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.changeset-delete-invalid-status-rejected|_client_error#34|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: cloudformation.stack-rollback-after-create-failed-lifecycle|run#7|AssertionError|assert failed_status == "CREATE_FAILED", (
Issue-Key: cloudformation|PublishType|CFNRegistryException
Issue-Key: cloudformation.delete-stack-duplicate-token-rejected|_routed_request#23|EgressRefused|target, route = ctx.endpoint_route(url)
Issue-Commit: 6537d79e9f76a83fe65f7d2499927078c6ec62b1

# cloudformation: 4 net-new behavioral causes — DeleteChangeSet accepts an executed change set, DisableRollback CREATE_FAILED is reported CREATE_COMPLETE, PublishType answers the publisher gate instead of "Type not found", and WaitConditionHandle Ref announces an unreachable host

> _Note: this issue was identified against the code base at commit `6537d79e9f76a83fe65f7d2499927078c6ec62b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChangeSet

## Description
Four independent root causes in the CloudFormation emulator, each grounded in its own failing behavioral check. A fixer must close all four before submitting.

=== Cause 1 of 4: DeleteChangeSet does not gate on ExecutionStatus ===
AWS: Smithy (com-amazonaws-cloudformation-cloudformation): DeleteChangeSet declares exactly one modelled error, InvalidChangeSetStatusException, errorKind client, httpError 400, awsQueryError code "InvalidChangeSetStatus" (the wire code has no "Exception" suffix). A change set whose execution has started is no longer deletable: DeleteChangeSet is refused with HTTP 400, Code "InvalidChangeSetStatus" and Message "Cannot delete ChangeSet in execution status <ExecutionStatus>" — fixed text "Cannot delete ChangeSet in execution status " followed by the change set's current ExecutionStatus from the emulator's own state (EXECUTE_IN_PROGRESS while the stack is applying it, EXECUTE_COMPLETE once it has finished). The refusal must not take effect: a following DescribeChangeSet still resolves the change set. A never-executed change set on the same stack stays deletable (recorded behavior: the check's positive control deletes cleanly and then DescribeChangeSet answers ChangeSetNotFound "ChangeSet [<arn>] does not exist").
Emulator: the refusal is only partly implemented. DeleteChangeSet on the change set once its ExecutionStatus is EXECUTE_COMPLETE SUCCEEDS (2xx, empty DeleteChangeSetOutput) instead of answering 400 / InvalidChangeSetStatus / "Cannot delete ChangeSet in execution status EXECUTE_COMPLETE". The replay reports: AssertionError: expected DeleteChangeSet of an executed change set to be rejected, but the call succeeded.
Why: a client that deletes change sets defensively silently destroys the audit record of what was actually applied, and never sees the InvalidChangeSetStatus it is written to catch. Retry/cleanup logic that treats a successful DeleteChangeSet as "this change set was never applied" draws the opposite conclusion from the truth.
Fix: the DeleteChangeSet handler's pre-delete status gate — it must reject for every ExecutionStatus other than UNAVAILABLE/AVAILABLE, not only for the in-progress one, and must leave the change set in state when it rejects.
Traces: cloudformation.changeset-delete-invalid-status-rejected

=== Cause 2 of 4: CreateStack with DisableRollback=true reports CREATE_COMPLETE for a resource it cannot create ===
AWS: Smithy: CreateStack carries the DisableRollback boolean member. With DisableRollback=true a resource failure leaves the stack in CREATE_FAILED (no rollback), which is the state RollbackStack then takes to ROLLBACK_COMPLETE (Smithy: RollbackStack, output StackId/OperationId). The provoking resource is an AWS::SSM::Parameter whose Type is SecureString, which CloudFormation cannot create — recorded behavior: the resource fails, the stack settles in CREATE_FAILED, and DescribeStackResource for the failed logical id afterwards answers ValidationError "Resource <LogicalResourceId> does not exist for stack <StackName>" (fixed text with the logical id and stack name taken from the request).
Emulator: the stack reaches CREATE_COMPLETE. The replay reports: AssertionError: expected CREATE_FAILED (DisableRollback=True), got CREATE_COMPLETE. The unsupported SecureString parameter is treated as created, so the whole CREATE_FAILED -> RollbackStack -> ROLLBACK_COMPLETE lifecycle is unreachable.
Why: a client that deploys with --disable-rollback to inspect a failure gets a green stack and a resource that does not exist; any code branching on CREATE_FAILED (including the documented RollbackStack recovery path) is dead.
Fix: the AWS::SSM::Parameter resource provisioner (it must refuse the SecureString type) and the stack provisioning state machine's DisableRollback path, which must settle the stack in CREATE_FAILED rather than CREATE_COMPLETE when a resource fails.
Traces: cloudformation.stack-rollback-after-create-failed-lifecycle

=== Cause 3 of 4: PublishType for a type that is not in the account answers the publisher gate instead of "Type not found in your account." ===
AWS: Smithy: PublishType declares CFNRegistryException (client, httpError 400, awsQueryError code CFNRegistryException) and TypeNotFoundException (client, httpError 404). For a private-type ARN that names no type registered in the caller's account, the refusal is Code "CFNRegistryException" with Message exactly "Type not found in your account." — a fixed string with no variable part. The not-found condition is evaluated before the publisher-registration condition.
Emulator: PublishType(Arn=arn:aws:cloudformation:<region>:<account>:type/resource/<Type-Name>/00000001) answers Code "CFNRegistryException" with Message "The account <account-id> is not verified as a publisher. Please register as publisher first" — the right code with the wrong message, because the publisher gate is checked first and the type lookup never runs. The replay reports: AssertionError: expected ('CFNRegistryException', 'Type not found in your account.'), got ('CFNRegistryException', 'The account 000000000000 is not verified as a publisher. Please register as publisher first') (the account id there is this run's value, not part of the requirement).
Why: the two conditions require opposite remedies. A client told it is not a verified publisher goes off to register a publisher id, when the real problem is that the type it named is not in the account. Error-message matching in registry tooling takes the wrong branch.
Fix: the PublishType handler's validation ordering — resolve the Arn/TypeName against the account's private type registry and refuse with "Type not found in your account." before consulting publisher registration.
Traces: cloudformation.type-registry-not-found-errors

=== Cause 4 of 4: AWS::CloudFormation::WaitConditionHandle Ref announces a host the emulator is not reached on ===
AWS: recorded behavior only (the Smithy model does not describe template resource attributes). Ref on an AWS::CloudFormation::WaitConditionHandle resolves to a presigned HTTPS URL on an AWS-served, resolvable host, to which a signaller PUTs its signal document. Whatever host the emulator serves, the URL it hands back must be an address the caller can actually reach — i.e. the endpoint the emulator is reached on.
Emulator: the value handed back is not an addressable URL at all: the host component is the synthesized handle name. The replay's egress layer refuses it — simparity.egress.EgressRefused: egress refused: 'sim2real-behav-delrej-<id>-handle-<id>' is not the endpoint under test (127.0.0.1); an emulator run may reach only the emulator. If the host came from a URL the emulator returned, the emulator announces an address other than the endpoint it is reached on. (The names and the 127.0.0.1 are this run's values; the defect is that the announced host is not the serving endpoint.) Because the signal cannot be delivered, the check's whole scenario — holding a stack in UPDATE_IN_PROGRESS to probe DeleteStack's concurrency refusals (ValidationError with a fresh ClientRequestToken, TokenAlreadyExistsException/ValidationError with the update's own token; Smithy: CreateStack and RollbackStack both declare TokenAlreadyExistsException, client, 400) — is never exercised.
Why: every wait-condition signalling path is unusable, and with it every stack whose completion depends on an external signal. A client following the documented flow (read Ref, PUT to it) gets a DNS failure, not a signal.
Fix: the AWS::CloudFormation::WaitConditionHandle resource's Ref resolution — it must mint a URL whose host is the endpoint the emulator serves, and the matching signal-receiving route must accept the PUT. Open issue 20261029T214321Z-cloudformation-aws-cloudformation-waitconditionhandle-ref-returns-the-logical.md and 20261101T212352Z-cloudformation-3-net-new-behavioral-root-causes-waitconditionhandle.md describe the same Ref shape; the suite did not bind this check to either, so it is recorded here and should be closed together with them.
Traces: cloudformation.delete-stack-duplicate-token-rejected

## Evidence
cloudformation.changeset-delete-invalid-status-rejected [mismatch]
  expected: HTTP 400, Error.Code 'InvalidChangeSetStatus', Error.Message 'Cannot delete ChangeSet in execution status EXECUTE_COMPLETE'; the change set still resolvable afterwards.
  actual:   DeleteChangeSet returned success. AssertionError: expected DeleteChangeSet of an executed change set to be rejected, but the call succeeded.
  (the EXECUTE_IN_PROGRESS leg of the same check passed, so the gate exists but stops short of EXECUTE_COMPLETE.)

cloudformation.stack-rollback-after-create-failed-lifecycle [mismatch]
  expected: CreateStack(DisableRollback=True) with an AWS::SSM::Parameter of Type SecureString settles the stack in CREATE_FAILED.
  actual:   AssertionError: expected CREATE_FAILED (DisableRollback=True), got CREATE_COMPLETE.

cloudformation.type-registry-not-found-errors [mismatch]
  expected: ('CFNRegistryException', 'Type not found in your account.')
  actual:   ('CFNRegistryException', 'The account 000000000000 is not verified as a publisher. Please register as publisher first')
  (same check's DescribeType(AWS::S3::Bucket) positive control passed, so the type-management family is wired; only this refusal's ordering is wrong.)

cloudformation.delete-stack-duplicate-token-rejected [error]
  expected: Ref on AWS::CloudFormation::WaitConditionHandle yields a presigned URL on the serving endpoint; a PUT to it signals the wait condition.
  actual:   simparity.egress.EgressRefused: egress refused: 'sim2real-behav-delrej-757d7ffa1641-handle-14e2c3d5ec97' is not the endpoint under test (127.0.0.1); ... the emulator announces an address other than the endpoint it is reached on.

## Steps to Reproduce
Replay these behavioral checks against the emulator:

1. cloudformation.changeset-delete-invalid-status-rejected — CreateStack; CreateChangeSet(ChangeSetType=UPDATE) adding a CreationPolicy-signalled AWS::CloudFormation::WaitCondition; ExecuteChangeSet; SignalResource the waiter; wait for the stack to reach UPDATE_COMPLETE and the change set's ExecutionStatus to reach EXECUTE_COMPLETE; call DeleteChangeSet(ChangeSetName=<change set ARN>, StackName=<stack>) and observe it succeed instead of refusing with InvalidChangeSetStatus.

2. cloudformation.stack-rollback-after-create-failed-lifecycle — CreateStack(DisableRollback=True) with Resources: {BadParam: {Type: AWS::SSM::Parameter, Properties: {Type: SecureString, Value: ...}}}; poll DescribeStacks and observe CREATE_COMPLETE instead of CREATE_FAILED.

3. cloudformation.type-registry-not-found-errors — call PublishType(Arn='arn:aws:cloudformation:<region>:<account>:type/resource/<Unregistered-Type-Name>/00000001') and read Error.Code/Error.Message.

4. cloudformation.delete-stack-duplicate-token-rejected — UpdateStack with an AWS::CloudFormation::WaitConditionHandle plus an AWS::CloudFormation::WaitCondition referencing it, read {"Ref": "Handle"} out of the stack, and try to PUT the signal document to that URL.
