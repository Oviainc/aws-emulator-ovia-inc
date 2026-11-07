Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfnuf-drift-detection-on-update-failed-stack-rejected cloudformation__lambda.cfnuf-execute-change-set-on-update-failed-without-disable-rollback-rejected cloudformation__lambda.cfnuf-continue-update-rollback-and-cancel-update-on-update-failed-rejected cloudformation__lambda.cfnuf-update-from-update-failed-without-disable-rollback-rejected cloudformation__lambda.cfnuf-change-set-on-update-failed-stack-diffs-against-the-rolled-back-state
Issue-Key: cloudformation__lambda.cfnuf-drift-detection-on-update-failed-stack-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudformation__lambda.cfnuf-execute-change-set-on-update-failed-without-disable-rollback-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cloudformation__lambda.cfnuf-continue-update-rollback-and-cancel-update-on-update-failed-rejected|run#7|AssertionError|assert msg == expected, f"ContinueUpdateRollback message {msg!r}, expected {expected!r}"
Issue-Key: cloudformation__lambda.cfnuf-update-from-update-failed-without-disable-rollback-rejected|run#8|AssertionError|assert msg == expected, f"UpdateStack on UPDATE_FAILED message {msg!r}, expected {expected!r}"
Issue-Key: cloudformation__lambda.cfnuf-change-set-on-update-failed-stack-diffs-against-the-rolled-back-state|run#5|AssertionError|assert stable["Status"] == "FAILED" and stable["Changes"] == [], (
Issue-Commit: afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b

# cloudformation__lambda: 5 net-new causes — an UPDATE_FAILED stack accepts DetectStackDrift and ExecuteChangeSet that AWS refuses, two status-refusal messages are misworded, and a change set is diffed against the failed template instead of the rolled-back live state

> _Note: this issue was identified against the code base at commit `afd9c28cd10d0281c7cc3c2a5c51ef0c1f3c612b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Five net-new root causes in CloudFormation's handling of a stack parked in UPDATE_FAILED (an update submitted with DisableRollback=true whose AWS::Lambda::Alias pointed at a nonexistent FunctionVersion). No open issue carries these keys; 20261107T012236Z-cloudformation-ssm-createchangeset-on-a-stack-in-review.md is about a REVIEW_IN_PROGRESS stack, not UPDATE_FAILED, and 20261029T214321Z-cloudformation-the-change-set-status-surface-diverges-delete.md is about change-set delete/execute status transitions, not the UPDATE_FAILED status gate.

Cause 1 of 5: DetectStackDrift is accepted on an UPDATE_FAILED stack.
AWS: Smithy models DetectStackDrift with no operation-specific errors, so the refusal is the service-common ValidationError (HTTP 400). Recorded behavior: on a stack in UPDATE_FAILED the call is refused with code `ValidationError` and the message, fixed text `Drift detection cannot be performed on stack [` + the stack's StackId + `] while it is in [` + the stack's current status + `] state`. The id and the status are values from state; the wording is the requirement. Once a fix-forward brings the same stack to UPDATE_COMPLETE the call is accepted and returns a StackDriftDetectionId for that stack.
Emulator: the call SUCCEEDS on the UPDATE_FAILED stack (no error at all), so no status gate exists.
Why: a client that uses the refusal to decide "this stack must be rolled back or fixed first" instead gets a drift report computed over a half-applied stack and treats a broken stack as healthy.
Fix: the DetectStackDrift handler's stack-status pre-flight.
Traces: cloudformation__lambda.cfnuf-drift-detection-on-update-failed-stack-rejected.

Cause 2 of 5: ExecuteChangeSet without DisableRollback is accepted on an UPDATE_FAILED stack.
AWS: Smithy models ExecuteChangeSet with ChangeSetNotFound/InsufficientCapabilities/InvalidChangeSetStatus/TokenAlreadyExists; this refusal is the common ValidationError (HTTP 400) with the message, fixed text `This stack is currently in a non-terminal [` + the stack's status + `] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API` (no trailing period). The stack must stay UPDATE_FAILED and the change set must stay ExecutionStatus=AVAILABLE; the same ExecuteChangeSet WITH DisableRollback=true is accepted and reaches UPDATE_COMPLETE.
Emulator: the un-flagged ExecuteChangeSet SUCCEEDS, consuming the change set and moving the stack.
Why: the DisableRollback opt-in that AWS requires to act on a non-terminal stack is not enforced, so a client that relies on the refusal (CLI/CDK deploy flows that catch it and switch to rollback-stack) silently executes an unguarded update.
Fix: the ExecuteChangeSet handler's stack-status pre-flight (shared with UpdateStack's non-terminal-state gate).
Traces: cloudformation__lambda.cfnuf-execute-change-set-on-update-failed-without-disable-rollback-rejected.

Cause 3 of 5: ContinueUpdateRollback's refusal message on an UPDATE_FAILED stack is the wrong text (and names the wrong operation).
AWS: ContinueUpdateRollback is modeled with only TokenAlreadyExistsException, so the status refusal is the common ValidationError (HTTP 400). Recorded message: fixed text `ContinueUpdateRollback cannot be called from stack with ` + the stack's current status + ` status.` — the status is a value from state, the rest including the trailing period is fixed.
Emulator: the code is right but the message is `RollbackUpdatedStack cannot be called from current stack status` — it names an internal operation name (`RollbackUpdatedStack`), drops the status, and drops the trailing period.
Why: a client or operator matching on the documented message (or merely reading it) is told about an operation it never called and cannot see which status blocked it.
Fix: the ContinueUpdateRollback handler's refusal message.
Traces: cloudformation__lambda.cfnuf-continue-update-rollback-and-cancel-update-on-update-failed-rejected.

Cause 4 of 5: UpdateStack's non-terminal-state refusal message carries a trailing period AWS does not emit.
AWS: the common ValidationError (HTTP 400) with exactly `This stack is currently in a non-terminal [<status>] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API` — the bracketed status is from state, the text ends at `API` with NO full stop.
Emulator: the identical sentence plus a trailing `.`: '... use the rollback-stack API.'
Why: an exact-match on the refusal message (the way automation distinguishes this recoverable refusal from other ValidationErrors) fails.
Fix: the UpdateStack handler's non-terminal-state refusal message (same string the ExecuteChangeSet gate of cause 2 must use).
Traces: cloudformation__lambda.cfnuf-update-from-update-failed-without-disable-rollback-rejected.

Cause 5 of 5: a change set created against an UPDATE_FAILED stack is diffed against the FAILED template instead of the rolled-back live state, and never comes back FAILED-with-no-changes.
AWS: recorded behavior only (CreateChangeSet is modeled with AlreadyExists/InsufficientCapabilities/LimitExceeded). Because the failed resources were already rolled back individually, a change set of the LAST STABLE template proposes nothing: DescribeChangeSet reports Status=FAILED with an empty Changes list, and executing it is refused with code `InvalidChangeSetStatus` and the message fixed text `ChangeSet [` + the change set's ChangeSetId + `] cannot be executed in its current status of [FAILED]`. A change set of the FAILED template is CREATE_COMPLETE with exactly one in-place Modify of the alias's FunctionVersion.
Emulator: the change set of the last stable template comes back Status=CREATE_COMPLETE carrying a change [('Fn0Alias', 'Modify', 'False', ['FunctionVersion'])] — it diffs template-against-template and never produces the empty FAILED change set, so the InvalidChangeSetStatus refusal is unreachable too.
Why: a client cannot use a change set to ask "does my last stable template still match reality?" — it is told there is work to do when there is none, and an executable change set is handed back where AWS hands back an unexecutable one. (Related to the missing per-resource rollback filed in the companion cloudformation__lambda issue, but the FAILED/empty change-set status and its refusal are computed by the change-set engine itself.)
Fix: the CreateChangeSet/DescribeChangeSet diff engine (diff against live resource state, emit FAILED with the no-changes reason) and the ExecuteChangeSet InvalidChangeSetStatus refusal.
Traces: cloudformation__lambda.cfnuf-change-set-on-update-failed-stack-diffs-against-the-rolled-back-state.

Handler surface: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/ (detect_stack_drift.rs, execute_change_set.rs, continue_update_rollback.rs, update_stack.rs, create_change_set.rs/describe_change_set.rs).

## Evidence
Cause 1: AssertionError: the call succeeded; real AWS rejects it, so there is no error code — DetectStackDrift(StackName=<UPDATE_FAILED stack>) returned 200 with a StackDriftDetectionId; expected ValidationError 'Drift detection cannot be performed on stack [<stack-id>] while it is in [UPDATE_FAILED] state'.
Cause 2: AssertionError: the call succeeded; real AWS rejects it, so there is no error code — ExecuteChangeSet(StackName=..., ChangeSetName='fix') with no DisableRollback returned 200; expected ValidationError 'This stack is currently in a non-terminal [UPDATE_FAILED] state. To update the stack from this state, please use the disable-rollback parameter with update-stack API. To rollback to the last known good state, use the rollback-stack API', the stack still UPDATE_FAILED and the change set still ExecutionStatus=AVAILABLE.
Cause 3: AssertionError: ContinueUpdateRollback message 'RollbackUpdatedStack cannot be called from current stack status', expected 'ContinueUpdateRollback cannot be called from stack with UPDATE_FAILED status.'
Cause 4: AssertionError: UpdateStack on UPDATE_FAILED message '...use the rollback-stack API.' (trailing period), expected '...use the rollback-stack API' (no trailing period).
Cause 5: AssertionError: a change set of the last stable template is 'CREATE_COMPLETE' with changes [('Fn0Alias', 'Modify', 'False', ['FunctionVersion'])]; the live state already matches it, so it must FAIL with no change (and ExecuteChangeSet on it must answer InvalidChangeSetStatus 'ChangeSet [<change-set-arn>] cannot be executed in its current status of [FAILED]').

## Steps to Reproduce
1. CreateStack (Capabilities=[CAPABILITY_IAM]) a template with one IAM role, an AWS::Lambda::Function, an AWS::Lambda::Version and an AWS::Lambda::Alias Name=LATEST, FunctionVersion=!GetAtt FnVersion.Version; wait for CREATE_COMPLETE.
2. UpdateStack with the same template but FunctionVersion hardcoded to a nonexistent literal (e.g. '593'), DisableRollback=True; wait for UPDATE_FAILED.
3. Cause 1: call DetectStackDrift(StackName=<stack>) — must be refused ValidationError with the drift-detection-while-in-[status] message; then fix forward to UPDATE_COMPLETE and confirm the same call is accepted.
4. Cause 2: CreateChangeSet with the corrected template, wait for CREATE_COMPLETE, then ExecuteChangeSet WITHOUT DisableRollback — must be refused ValidationError (non-terminal state) leaving ExecutionStatus=AVAILABLE; the same call with DisableRollback=True must execute to UPDATE_COMPLETE.
5. Cause 3: call ContinueUpdateRollback(StackName=<stack>) and compare the ValidationError message; also CancelUpdateStack ('CancelUpdateStack cannot be called from current stack status').
6. Cause 4: call UpdateStack with the corrected template and no DisableRollback and compare the ValidationError message byte for byte.
7. Cause 5: CreateChangeSet of the LAST STABLE template on the UPDATE_FAILED stack and DescribeChangeSet — expect Status=FAILED with Changes=[]; then ExecuteChangeSet on it and expect InvalidChangeSetStatus. A change set of the FAILED template must be CREATE_COMPLETE with one in-place Modify of FunctionVersion.
