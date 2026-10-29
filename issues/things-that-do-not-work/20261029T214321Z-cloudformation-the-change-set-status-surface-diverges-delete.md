Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.changeset-delete-invalid-status-rejected cloudformation.execute-change-set-twice-rejected cloudformation.execute-changeset-already-executed-error cloudformation.delete-executed-changeset-error cloudformation.delete-changeset-invalid-status-rejected cloudformation.create-changeset-missing-capabilities-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.changeset-delete-invalid-status-rejected|run#25|AssertionError|assert msg == "Cannot delete ChangeSet in execution status EXECUTE_IN_PROGRESS", msg
Issue-Key: cloudformation.execute-change-set-twice-rejected|run#19|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: cloudformation.execute-changeset-already-executed-error|run#12|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: cloudformation.delete-executed-changeset-error|run#15|AssertionError|assert executed == expected_exec, (
Issue-Key: cloudformation.delete-changeset-invalid-status-rejected|_expect_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: cloudformation.create-changeset-missing-capabilities-rejected|run#15|AssertionError|assert describe_code == "ValidationError", (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: the change-set status surface diverges — Delete/Execute refusal messages, Status reported in place of ExecutionStatus, executed change sets vanish, and a non-deletable change set is deleted (5 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteChangeSet

## Description
Five net-new behavioral root causes of service `cloudformation`, grounded in six failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 5: DeleteChangeSet's InvalidChangeSetStatus message is the emulator's own sentence ===
AWS: Smithy models DeleteChangeSet with exactly one error, `InvalidChangeSetStatusException` (errorKind client, httpError 400, awsQueryError code `InvalidChangeSetStatus`/400). Message is recorded behavior only: deleting a change set whose ExecutionStatus is EXECUTE_IN_PROGRESS is refused with Error.Message `Cannot delete ChangeSet in execution status EXECUTE_IN_PROGRESS` — fixed text `Cannot delete ChangeSet in execution status ` plus the current ExecutionStatus value as its only variable part. The change set's ARN is NOT part of the message.
Emulator: same status/code, message `ChangeSet [<change-set ARN>] cannot be deleted while its execution status is [<status>]` — a different template that additionally interpolates the ARN.
Why: Message is the only discriminator between the two change-set status refusals; taxonomy suites and operators comparing against AWS/CLI output read a string AWS never emits, and the interpolated ARN makes it unstable across runs.
Fix: the DeleteChangeSet handler's status-guard refusal in the cloudformation service's change-set ops.
Traces: cloudformation.changeset-delete-invalid-status-rejected.

=== Cause 2 of 5: ExecuteChangeSet's refusal reports the change set's Status where AWS reports its ExecutionStatus ===
AWS: Smithy models ExecuteChangeSet errors ChangeSetNotFoundException (404/ChangeSetNotFound), InsufficientCapabilitiesException, `InvalidChangeSetStatusException` (400/InvalidChangeSetStatus), TokenAlreadyExistsException; DescribeChangeSet output carries BOTH `Status` (ChangeSetStatus: CREATE_COMPLETE …) and `ExecutionStatus` (AVAILABLE/EXECUTE_IN_PROGRESS/EXECUTE_COMPLETE …). Recorded behavior: re-executing an already-executed change set is refused 400 InvalidChangeSetStatus with `ChangeSet [<arn>] cannot be executed in its current execution status of [EXECUTE_COMPLETE]` — fixed text `ChangeSet [` + `] cannot be executed in its current execution status of [` + `]`, variable parts = the change-set ARN and the EXECUTION status.
Emulator: `ChangeSet [<arn>] cannot be executed in its current status of [CREATE_COMPLETE]` — the words `execution status` are reduced to `status` and the value substituted is the ChangeSetStatus (CREATE_COMPLETE) instead of the ExecutionStatus (EXECUTE_COMPLETE).
Why: the message tells the caller the change set is merely CREATE_COMPLETE, i.e. ready to execute, when the true reason is that it has already run; retry logic keyed on that reading re-executes forever.
Fix: the ExecuteChangeSet handler's status guard — read and render the change set's ExecutionStatus, with AWS's wording.
Traces: cloudformation.execute-change-set-twice-rejected, cloudformation.execute-changeset-already-executed-error.

=== Cause 3 of 5: a change set that has been executed is removed from state, so later calls 404 instead of being refused on status ===
AWS: an executed change set remains addressable by name and ARN; DeleteChangeSet/ExecuteChangeSet against it answer 400 InvalidChangeSetStatus (`… cannot be executed in its current execution status of [EXECUTE_COMPLETE]`), never ChangeSetNotFound.
Emulator: after execution the record is gone — ExecuteChangeSet answers 404 `ChangeSetNotFound` with `ChangeSet [<name>] does not exist`.
Why: a caller cannot distinguish 'already applied' (a benign, idempotent outcome) from 'this change set never existed' (a programming or race error); 404 also pushes SDK retry/paging layers down a different path than 400.
Fix: change-set lifecycle state in the cloudformation service — retain executed change sets and let the status guard, not the existence lookup, produce the refusal.
Traces: cloudformation.delete-executed-changeset-error.

=== Cause 4 of 5: DeleteChangeSet succeeds on a change set whose status does not permit deletion ===
AWS: DeleteChangeSet is refused 400 InvalidChangeSetStatus while the change set is in a non-deletable execution status (see Cause 1); the call does not return 200.
Emulator: returns HTTP 200 with an empty DeleteChangeSetOutput for that same request — the guard is simply absent on this path.
Why: a client believes the change set is gone while it is still executing; subsequent reconciliation (list/describe, or a re-create under the same name) diverges and the stack keeps applying a change set the caller thinks it cancelled.
Fix: the DeleteChangeSet handler's status guard in the cloudformation service's change-set ops (the same guard Cause 1 reworded must also actually fire here).
Traces: cloudformation.delete-changeset-invalid-status-rejected.

=== Cause 5 of 5: DescribeChangeSet for a name on a stack that was never created answers ChangeSetNotFound instead of ValidationError ===
AWS: Smithy models DescribeChangeSet error `ChangeSetNotFoundException` (404/ChangeSetNotFound) only; recorded behavior for a CreateChangeSet(ChangeSetType=CREATE) that was REFUSED (e.g. for a missing CAPABILITY_IAM) is that neither the stack nor the change set exists, and DescribeChangeSet is answered with the service-common `ValidationError` naming the absent stack — not ChangeSetNotFound, which AWS reserves for a change set missing from an existing stack.
Emulator: answers Error.Code `ChangeSetNotFound`.
Why: the caller is told the change set is missing from a stack that in fact does not exist, so cleanup/retry logic targets the wrong object and never notices the stack was never created.
Fix: DescribeChangeSet's resolution order in the cloudformation service — resolve the stack first and refuse with the stack-level ValidationError when it is absent.
Traces: cloudformation.create-changeset-missing-capabilities-rejected.

## Evidence
Cause 1 — cloudformation.changeset-delete-invalid-status-rejected [mismatch]:
  assert msg == "Cannot delete ChangeSet in execution status EXECUTE_IN_PROGRESS"
  AssertionError: ChangeSet [arn:aws:cloudformation:us-west-2:000000000000:changeSet/sim2real-cs-416b96933d9d/5bebd1d2-28c1-1ada-afde-b16be40832c2] cannot be deleted while its execution status is [EXECUTE_IN_PROGRESS]
  (the ARN is a per-run value the emulator should not be interpolating at all; EXECUTE_IN_PROGRESS is the one variable part AWS's message carries)
  Smithy: DeleteChangeSet errors = [InvalidChangeSetStatusException, client, httpError 400, awsQueryError {code: InvalidChangeSetStatus, httpResponseCode: 400}].

Cause 2 — cloudformation.execute-change-set-twice-rejected [mismatch]:
  AssertionError: expected message 'ChangeSet [arn:...:changeSet/sim2real-cs-a1d26135a435/bfa5787c-...] cannot be executed in its current execution status of [EXECUTE_COMPLETE]', got 'ChangeSet [arn:...:changeSet/sim2real-cs-a1d26135a435/bfa5787c-...] cannot be executed in its current status of [CREATE_COMPLETE]'
  cloudformation.execute-changeset-already-executed-error [mismatch]: identical divergence on a different per-run change set.
  Smithy: DescribeChangeSet output has Status (ChangeSetStatus) AND ExecutionStatus (ExecutionStatus enum); ExecuteChangeSet errors include InvalidChangeSetStatusException (400/InvalidChangeSetStatus).

Cause 3 — cloudformation.delete-executed-changeset-error [mismatch]:
  AssertionError: ExecuteChangeSet on the deleted change set: expected ('InvalidChangeSetStatus', 'ChangeSet [arn:...:changeSet/sim2real-behav-cs-cb5a3defe114/c5408a9e-...] cannot be executed in its current execution status of [EXECUTE_COMPLETE]'), got ('ChangeSetNotFound', 'ChangeSet [sim2real-behav-cs-cb5a3defe114] does not exist')

Cause 4 — cloudformation.delete-changeset-invalid-status-rejected [mismatch]:
  AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'RequestId': 'f489b906-...', 'HTTPStatusCode': 200, ...}}

Cause 5 — cloudformation.create-changeset-missing-capabilities-rejected [mismatch]:
  assert describe_code == "ValidationError"
  AssertionError: expected no change set to exist after rejection, got describe code 'ChangeSetNotFound'
  Smithy: CreateChangeSet errors = AlreadyExistsException, InsufficientCapabilitiesException (400), LimitExceededException; DescribeChangeSet errors = ChangeSetNotFoundException (404/ChangeSetNotFound).

## Steps to Reproduce
Setup for causes 1-4: CreateStack with a trivial AWS::SNS::Topic; CreateChangeSet(ChangeSetType=UPDATE) whose template adds an AWS::CloudFormation::WaitCondition so the execution is held in EXECUTE_IN_PROGRESS.
1. (Cause 1) ExecuteChangeSet, then immediately DeleteChangeSet on it. Observe 400 InvalidChangeSetStatus with 'ChangeSet [<arn>] cannot be deleted while its execution status is [EXECUTE_IN_PROGRESS]'. AWS: 'Cannot delete ChangeSet in execution status EXECUTE_IN_PROGRESS'.
2. (Cause 4) Repeat the DeleteChangeSet on a change set in a non-deletable status and observe HTTP 200 with an empty body instead of a refusal.
3. (Cause 2) Let the change set finish, then ExecuteChangeSet again. Observe '... cannot be executed in its current status of [CREATE_COMPLETE]'. AWS: '... cannot be executed in its current execution status of [EXECUTE_COMPLETE]'.
4. (Cause 3) After that execution, DescribeChangeSet/ExecuteChangeSet by the change set NAME. Observe 404 ChangeSetNotFound 'ChangeSet [<name>] does not exist'; AWS keeps the record and refuses 400 InvalidChangeSetStatus.
5. (Cause 5) CreateChangeSet(ChangeSetType='CREATE', StackName=<fresh>, TemplateBody=<an AWS::IAM::Role>) WITHOUT Capabilities so it is refused, then DescribeChangeSet(ChangeSetName=<that name>, StackName=<that stack>). Observe Error.Code 'ChangeSetNotFound'; AWS answers ValidationError because the stack itself does not exist.
6. Re-replay: cloudformation.changeset-delete-invalid-status-rejected, cloudformation.execute-change-set-twice-rejected, cloudformation.execute-changeset-already-executed-error, cloudformation.delete-executed-changeset-error, cloudformation.delete-changeset-invalid-status-rejected, cloudformation.create-changeset-missing-capabilities-rejected.
