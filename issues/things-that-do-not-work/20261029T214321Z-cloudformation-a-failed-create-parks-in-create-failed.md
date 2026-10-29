Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.stack-rollback-cleans-up cloudformation.stack-rollback-on-resource-failure cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete cloudformation.deep-cloudformation-2-retain-except-on-create cloudformation.create-failure-rollback cloudformation.changeset-execution-dependency-ordering cloudformation.stack-rollback-after-create-failed-lifecycle cloudformation.disable-rollback cloudformation.continue-update-rollback-wrong-state-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.stack-rollback-cleans-up|run#7|AssertionError|assert status == "ROLLBACK_COMPLETE", f"Expected ROLLBACK_COMPLETE, got {status}"
Issue-Key: cloudformation.stack-rollback-on-resource-failure|run#8|AssertionError|assert final["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete|run#11|AssertionError|assert got == {"reoc-failed": "ROLLBACK_COMPLETE", "retain-failed": "ROLLBACK_COMPLETE",
Issue-Key: cloudformation.deep-cloudformation-2-retain-except-on-create|_create_and_roll_back#3|AssertionError|assert stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.create-failure-rollback|_expect_error#4|AssertionError|raise AssertionError(f'expected {codes!r}, but the call succeeded: {result!r}')
Issue-Key: cloudformation.changeset-execution-dependency-ordering|_client_error#23|AssertionError|raise AssertionError(f"expected {what} to be rejected, but the call succeeded")
Issue-Key: cloudformation.stack-rollback-after-create-failed-lifecycle|run#7|AssertionError|assert failed_status == "CREATE_FAILED", (
Issue-Key: cloudformation.disable-rollback|run#10|AssertionError|assert resources.get("Ok") == "CREATE_COMPLETE", (
Issue-Key: cloudformation.continue-update-rollback-wrong-state-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: a failed create parks in CREATE_FAILED and never rolls back, creates designed to fail reach CREATE_COMPLETE, a dangling DependsOn is accepted, DisableRollback drops the stack's resources, and ContinueUpdateRollback's refusal wording diverges (5 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five net-new behavioral root causes of service `cloudformation`, grounded in nine failing behavioral checks. Each cause is independent and must be closed separately. The open issue 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md carries a different set of replay keys (none of these nine) and does not cover any cause below; these checks are marked neither ALREADY FILED nor OWNED in the replay report.

=== Cause 1 of 5: a create whose resource fails terminates in CREATE_FAILED — the ROLLBACK_IN_PROGRESS → ROLLBACK_COMPLETE transition never happens ===
AWS: recorded behavior only (Smithy models StackStatus as an enum including CREATE_FAILED, ROLLBACK_IN_PROGRESS and ROLLBACK_COMPLETE but states no transition rule). With default OnFailure/rollback behaviour, a CreateStack whose resource creation fails — including an AWS::CloudFormation::WaitCondition that exceeds its Timeout — moves the stack to ROLLBACK_IN_PROGRESS, deletes the resources it created, and settles in ROLLBACK_COMPLETE. CREATE_FAILED is the terminal status only when rollback is suppressed (DisableRollback=True / OnFailure=DO_NOTHING).
Emulator: the stack settles in CREATE_FAILED with rollback never attempted.
Why: every create-failure workflow built on AWS reads ROLLBACK_COMPLETE as 'failed and cleaned up' and CREATE_FAILED as 'failed and left in place'. Against the emulator a client sees the leave-in-place status for a default create, so it will not delete the stack before retrying (AWS requires deleting a ROLLBACK_COMPLETE stack before re-creating it) and it cannot tell whether the partial resources were removed.
Fix: the create-failure state machine in the cloudformation service's stack lifecycle/engine — drive ROLLBACK_IN_PROGRESS, the reverse-order resource deletion, and ROLLBACK_COMPLETE unless rollback is explicitly disabled.
Traces: cloudformation.stack-rollback-cleans-up, cloudformation.stack-rollback-on-resource-failure, cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete.

=== Cause 2 of 5: a create that must fail reaches CREATE_COMPLETE ===
AWS: recorded behavior only — the RetainExceptOnCreate scenario's failing resource makes the stack fail (and then roll back per Cause 1); CREATE_COMPLETE means every declared resource was provisioned.
Emulator: the same stack reports CREATE_COMPLETE with StackStatusReason None, i.e. the resource failure is not detected at all, which is strictly worse than Cause 1 (there the failure is seen but mis-terminated).
Why: CREATE_COMPLETE is the signal every deployment pipeline gates on. A client proceeds to use resources that were never created, and no event or status anywhere reports the fault.
Fix: resource-provisioning failure propagation into stack status in the cloudformation service's stack engine, for the RetainExceptOnCreate create path.
Traces: cloudformation.deep-cloudformation-2-retain-except-on-create.

=== Cause 3 of 5: a DependsOn that names a logical id absent from Resources is accepted ===
AWS: Smithy models no specific error for this; recorded behavior is HTTP 400, Error.Code `ValidationError` from CreateStack AND from CreateChangeSet, because an unresolvable DependsOn target is a template-validation failure resolved before provisioning starts.
Emulator: CreateStack answers HTTP 200 with a StackId/OperationId, and CreateChangeSet likewise succeeds — the dangling reference is never validated.
Why: the client is handed a stack id for a template AWS refuses, so the error surfaces (if at all) as a mid-provision failure or not at all, and change-set review shows a plan that could never be executed on AWS.
Fix: template graph validation (DependsOn target resolution) in the cloudformation service's template validator, applied by both the CreateStack and CreateChangeSet handlers before any state write.
Traces: cloudformation.create-failure-rollback, cloudformation.changeset-execution-dependency-ordering.

=== Cause 4 of 5: DisableRollback=True on a failing create neither parks in CREATE_FAILED nor keeps the resources it created ===
AWS: Smithy models CreateStack.DisableRollback (boolean) and ExecuteChangeSet.DisableRollback. Recorded behavior: with DisableRollback=True a failing create settles in CREATE_FAILED and the resources that DID succeed are retained and remain listed by ListStackResources/DescribeStackResources.
Emulator: the stack reaches CREATE_COMPLETE instead of CREATE_FAILED, and in the sibling check the successful resource is not left in place — the resource view comes back empty ({}).
Why: DisableRollback exists precisely so an operator can inspect the partial stack after a failure. Against the emulator there is neither a failure status to notice nor a partial resource set to inspect, so the debugging workflow the flag exists for is impossible.
Fix: the DisableRollback branch of the create-failure path in the cloudformation service's stack engine, plus the resource-view retention for a stack that stopped in CREATE_FAILED.
Traces: cloudformation.stack-rollback-after-create-failed-lifecycle, cloudformation.disable-rollback.

=== Cause 5 of 5: ContinueUpdateRollback's wrong-state refusal message is the emulator's own sentence ===
AWS: Smithy models ContinueUpdateRollback with only TokenAlreadyExistsException, so a call made from a stack status that does not permit it is refused by the service-common `ValidationError` (HTTP 400). Message is recorded behavior only and is the fixed text `RollbackUpdatedStack cannot be called from current stack status` — it interpolates NOTHING from the request or state (neither the stack ARN nor the status).
Emulator: `Stack:<stack ARN> is in CREATE_COMPLETE state and can not be continued.` — a different template that interpolates both the ARN and the status.
Why: taxonomy assertions and operator-facing diagnostics read a string AWS never emits, and because the emulator's variant embeds a per-run ARN it is not even stable across runs.
Fix: the ContinueUpdateRollback handler's state-guard refusal in the cloudformation service's stack ops.
Traces: cloudformation.continue-update-rollback-wrong-state-rejected.

## Evidence
Cause 1:
  cloudformation.stack-rollback-cleans-up [mismatch] — AssertionError: Expected ROLLBACK_COMPLETE, got CREATE_FAILED
  cloudformation.stack-rollback-on-resource-failure [mismatch] — AssertionError: expected ROLLBACK_COMPLETE after the WaitCondition timed out, got CREATE_FAILED
  cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete [mismatch] — AssertionError: terminal stack statuses: {'reoc-failed': 'CREATE_FAILED', 'retain-failed': 'CREATE_FAILED', 'reoc-ok': 'CREATE_COMPLETE'}

Cause 2:
  cloudformation.deep-cloudformation-2-retain-except-on-create [mismatch] — AssertionError: [retain] failed create should roll back; stack is 'CREATE_COMPLETE' (None)  (the None is the absent StackStatusReason)

Cause 3:
  cloudformation.create-failure-rollback [mismatch] — AssertionError: expected ('ValidationError',), but the call succeeded: {'StackId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-behav-stk-2e9bea78519c/370734bc-...', 'OperationId': '4af5dc36-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
    request: CreateStack with {"Resources": {"Bad": {"Type": "AWS::CloudFormation::WaitConditionHandle", "DependsOn": "DoesNotExist"}}}
  cloudformation.changeset-execution-dependency-ordering [mismatch] — AssertionError: expected CreateChangeSet with a DependsOn on a nonexistent logical id to be rejected, but the call succeeded

Cause 4:
  cloudformation.stack-rollback-after-create-failed-lifecycle [mismatch] — AssertionError: expected CREATE_FAILED (DisableRollback=True), got CREATE_COMPLETE
  cloudformation.disable-rollback [mismatch] — AssertionError: DisableRollback=True must leave the successful resource in place, got {}
  Smithy: CreateStack.DisableRollback (DisableRollback, boolean); ExecuteChangeSet.DisableRollback (boolean).

Cause 5:
  cloudformation.continue-update-rollback-wrong-state-rejected [mismatch] — AssertionError: expected message 'RollbackUpdatedStack cannot be called from current stack status', got 'Stack:arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-continue-rb-4156aa01a4d4/e90a358e-... is in CREATE_COMPLETE state and can not be continued.'
  Smithy: ContinueUpdateRollback errors = [TokenAlreadyExistsException (client, 400)] only — the state guard refuses with the service-common ValidationError; the expected message is entirely fixed text.

## Steps to Reproduce
Cause 1:
1. CreateStack with an AWS::CloudFormation::WaitCondition whose Timeout is a few seconds and which is never signalled (plus one resource that creates fine).
2. Poll DescribeStacks. Observe terminal CREATE_FAILED. AWS goes ROLLBACK_IN_PROGRESS → ROLLBACK_COMPLETE and removes the created resource.

Cause 2:
1. CreateStack with RetainExceptOnCreate=True and a resource whose creation must fail.
2. Observe CREATE_COMPLETE with no StackStatusReason; AWS fails the stack.

Cause 3:
1. CreateStack(TemplateBody='{"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Bad":{"Type":"AWS::CloudFormation::WaitConditionHandle","DependsOn":"DoesNotExist"}}}'). Observe HTTP 200 + StackId; AWS answers 400 ValidationError.
2. Same template through CreateChangeSet — also accepted; AWS refuses.

Cause 4:
1. CreateStack(DisableRollback=True) with one resource that succeeds and one that fails.
2. Observe CREATE_COMPLETE (AWS: CREATE_FAILED) and ListStackResources/DescribeStackResources returning no retained resource.

Cause 5:
1. On a stack in CREATE_COMPLETE call ContinueUpdateRollback(StackName=<stack>).
2. Observe 400 ValidationError 'Stack:<arn> is in CREATE_COMPLETE state and can not be continued.'. AWS returns the fixed 'RollbackUpdatedStack cannot be called from current stack status'.

Re-replay all nine: cloudformation.stack-rollback-cleans-up, cloudformation.stack-rollback-on-resource-failure, cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete, cloudformation.deep-cloudformation-2-retain-except-on-create, cloudformation.create-failure-rollback, cloudformation.changeset-execution-dependency-ordering, cloudformation.stack-rollback-after-create-failed-lifecycle, cloudformation.disable-rollback, cloudformation.continue-update-rollback-wrong-state-rejected.
