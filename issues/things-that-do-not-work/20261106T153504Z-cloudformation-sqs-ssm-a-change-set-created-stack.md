Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes cloudformation__sqs__ssm.cdk-destroy-deletes-a-create-failed-stack-and-its-live-resources cloudformation__sqs__ssm.cdk-rollback-restores-an-update-failed-stack-to-its-last-good-state
Issue-Key: cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes|run#17|AssertionError|assert got == ("CREATE_FAILED", "The following resource(s) failed to create: [Bad]. ", True), (
Issue-Key: cloudformation__sqs__ssm.cdk-destroy-deletes-a-create-failed-stack-and-its-live-resources|run#17|AssertionError|assert parked["StackStatus"] == "CREATE_FAILED", f"the stack is {parked['StackStatus']!r}"
Issue-Key: cloudformation__sqs__ssm.cdk-rollback-restores-an-update-failed-stack-to-its-last-good-state|run#15|AssertionError|assert rc == 0, f"the first deploy exited {rc}; output tail {out[-800:]!r}"
Issue-Commit: 81fe993c0fc493a00f42b69c764d54e4324ad80b

# cloudformation__sqs__ssm: a change-set-created stack stays REVIEW_IN_PROGRESS after ExecuteChangeSet, so --no-rollback parking, cdk destroy of a CREATE_FAILED stack and cdk rollback are all unreachable

> _Note: this issue was identified against the code base at commit `81fe993c0fc493a00f42b69c764d54e4324ad80b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Cause 1 of 1: ExecuteChangeSet on a CREATE change set does not take the stack out of REVIEW_IN_PROGRESS — the stack is never provisioned and never reaches a terminal create status, so every state that the deploy/rollback surface is supposed to park in is unobservable.

AWS: a new stack created with CreateChangeSet ChangeSetType=CREATE sits in REVIEW_IN_PROGRESS; ExecuteChangeSet then moves it to CREATE_IN_PROGRESS and on to a terminal status. With rollback disabled, a resource that fails leaves the stack in CREATE_FAILED with StackStatusReason 'The following resource(s) failed to create: [<logical id>]. ' and DisableRollback=true, and the stack is then updatable, deletable and rollback-able. Smithy (cloudformation DescribeStacks, StackStatus enum) models REVIEW_IN_PROGRESS, CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE and UPDATE_* as distinct statuses; ExecuteChangeSet models only ChangeSetNotFoundException (404), InsufficientCapabilitiesException (400), InvalidChangeSetStatusException (400) and TokenAlreadyExistsException (400) — nothing that leaves the stack in review. The StackStatusReason wording and the DisableRollback flag are recorded behavior only.

Emulator: after the change set is created and executed, DescribeStacks answers StackStatus='REVIEW_IN_PROGRESS' with StackStatusReason='User Initiated' and DisableRollback=false, i.e. ('REVIEW_IN_PROGRESS', 'User Initiated', False) where AWS reports ('CREATE_FAILED', 'The following resource(s) failed to create: [<logical id>]. ', True); and the `cdk deploy` that drives it exits 1 with 'ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state' because the stack has no executed template.

Why: a client cannot deploy through the change-set path at all, and the whole failure-recovery contract built on top of it is lost — a failed --no-rollback create is not parked in CREATE_FAILED for inspection, `cdk destroy` has no CREATE_FAILED stack (and no live resources) to tear down, and a rollback has no last-good state to restore. Tooling that waits on a terminal stack status hangs or reads a status that is neither in-progress nor terminal.

Fix: the ExecuteChangeSet handler and the stack-state machine/provisioner it drives in the CloudFormation service; the executed change set must become the stack's template and advance the stack to a terminal create status (honouring DisableRollback).

Traces: cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes, cloudformation__sqs__ssm.cdk-destroy-deletes-a-create-failed-stack-and-its-live-resources, cloudformation__sqs__ssm.cdk-rollback-restores-an-update-failed-stack-to-its-last-good-state. The open issue 20261102T094631Z-cloudformation-sqs-ssm-2-net-new-causes-aws.md names two of these checks for different causes (AWS::CDK::Metadata refusal, already-exists reason wording) which the deploy no longer fails on; this status cause is not described there. The same cause is filed separately under cloudformation__sqs, cloudformation__sns__sqs and cloudformation__iam__sqs__ssm.

## Evidence
Expected (AWS): after `cdk deploy --no-rollback` of a stack with one failing resource, (StackStatus, StackStatusReason, DisableRollback) == ('CREATE_FAILED', 'The following resource(s) failed to create: [Bad]. ', True); a later `cdk destroy` deletes that CREATE_FAILED stack and its live resources.
Actual (emulator): 'AssertionError: after `cdk deploy --no-rollback` failed the stack is (\'REVIEW_IN_PROGRESS\', \'User Initiated\', False)' and 'AssertionError: the stack is \'REVIEW_IN_PROGRESS\''; the rollback leg's deploy exits 1 with 'ValidationError: Template does not exist for stack [<stack>] in REVIEW_IN_PROGRESS state'.

## Steps to Reproduce
1. CreateChangeSet(StackName=<new stack>, ChangeSetType=CREATE) with a template whose resources include one that must fail, then ExecuteChangeSet(DisableRollback=true).
2. Poll DescribeStacks: the stack remains REVIEW_IN_PROGRESS / 'User Initiated' / DisableRollback=false instead of parking in CREATE_FAILED; DescribeStackEvents records no per-resource create.
3. DeleteStack / RollbackStack then have no provisioned stack to act on.
Replay checks cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes, cloudformation__sqs__ssm.cdk-destroy-deletes-a-create-failed-stack-and-its-live-resources, cloudformation__sqs__ssm.cdk-rollback-restores-an-update-failed-stack-to-its-last-good-state.
