Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource cloudformation.stack-rollback-stack-client-request-token-marks-rollback-events cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events|run#8|AssertionError|assert _settle(cfn, stack_id, timeout_s=_cap(ctx, _DEADLINE_S)) == "UPDATE_ROLLBACK_FAILED", f"the failing update ended {_stack(cfn, stack_id)!r}"
Issue-Key: cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource|run#6|AssertionError|assert _settle(cfn, stack_id, timeout_s=_cap(ctx, _DEADLINE_S)) == "UPDATE_ROLLBACK_FAILED", f"the failing update ended {_stack(cfn, stack_id)!r}"
Issue-Key: cloudformation.stack-rollback-stack-client-request-token-marks-rollback-events|run#14|AssertionError|assert failed == [
Issue-Key: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty|run#12|AssertionError|assert auto_resource is None or auto_resource["ResourceStatus"] == "DELETE_COMPLETE", (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation: 4 of the 7 failed-create/rollback causes remain open — a create designed to fail still completes, a failing update does not end UPDATE_ROLLBACK_FAILED, the failed update's event list still diverges, and a CDK auto-delete bucket is never emptied

> _This report carries the replay keys that do NOT pass, re-filed here so they are not recorded
> as working. The three that DO pass — the `Rules` section evaluated against the values in
> force, a create whose wait runs out rolled back to ROLLBACK_COMPLETE, and
> `DeletionPolicy: Snapshot` leaving one snapshot behind — are in
> `issues/things-that-now-work/20261102T101202Z-cloudformation-7-net-new-root-causes-failed-creates.md`,
> whose description holds the full account of all seven causes._

## Operation
CreateStack

## Description
Four of the seven causes of the original report are still open after the fix that closed causes
1, 3 and 7. Each is independent and must be closed on its own.

Cause A (cause 2 of the original) — a create whose resource fails settles CREATE_COMPLETE.
AWS: recorded behavior only — the five-stack estate expects {alpha..delta: CREATE_COMPLETE,
echo: ROLLBACK_COMPLETE}: the resource `echo` declares fails, so that stack fails and rolls
back.
Emulator: every stack of the estate, `echo` included, reaches CREATE_COMPLETE.
What is already in place: the failure-and-rollback rule itself. A create whose resource an
owning service refuses does fail and roll back to ROLLBACK_COMPLETE, removing what it had made
(held to by `tests/all/failed_create_rollback.rs`, `tests/all/duplicate_resource_identifiers.rs`
and `tests/all/unprovisioned_resources.rs`), and so does one whose resource-signal wait runs
out (`tests/all/expired_wait_rollback.rs`).
What is missing: WHICH declaration of `echo` AWS refuses. The replay reports only the six
stacks' statuses, so the next fixer needs the fixture's template: the cause is that some
property or identity this emulator accepts is one AWS's own resource provider rejects, and
until that is known a fix would be a guess keyed to one input.
Traces: cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle.

Cause B (cause 4 of the original) — a failing update does not settle UPDATE_ROLLBACK_FAILED.
AWS: recorded behavior only — the failing update ends UPDATE_ROLLBACK_FAILED, which is the
state `ContinueUpdateRollback` (with and without `ResourcesToSkip`) then acts on.
Emulator: the update does not reach that status.
What is already in place: the signals of an earlier deployment no longer satisfy a wait
condition a later deployment deploys again; a rollback waits at the wait conditions the
configuration it puts back deploys again; and a rollback whose wait runs out leaves the stack
UPDATE_ROLLBACK_FAILED with the resource it stopped at reported UPDATE_FAILED, which
`ContinueUpdateRollback(ResourcesToSkip)` then completes (`tests/all/expired_wait_rollback.rs`).
What is missing: how the fixture's update fails. The replay's divergence text is truncated
before the stack's status, so the mechanism (which resource the update changes, and what makes
its ROLLBACK fail rather than complete) could not be established from the replay alone.
Traces: cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events,
cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource.

Cause C (cause 5 of the original) — the failed update's event list still diverges.
AWS: recorded behavior only — the failed update's events end with the stack's UPDATE_FAILED
event followed by UPDATE_ROLLBACK_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE, all stamped with
the operation's ClientRequestToken.
Emulator: the half of this cause about re-created resources is FIXED — a resource the template
re-declares unchanged is no longer created again, whether or not any service provisions its
type, so the pre-existing `Handle` no longer emits CREATE_* events — and the three stack-level
rollback events are recorded (`tests/all/expired_wait_rollback.rs` holds both). The unit still
fails on the rest of its expected list, which the replay truncates.
Traces: cloudformation.stack-rollback-stack-client-request-token-marks-rollback-events.

Cause D (cause 6 of the original) — a CDK auto-delete bucket is never emptied, so its deletion
is refused.
AWS: recorded behavior only — the auto bucket's resource is gone (DELETE_COMPLETE) after the
destroy, while the manual bucket's deletion blocks on BucketNotEmpty.
Emulator: the auto bucket is now reported DELETE_FAILED rather than CREATE_COMPLETE: the
deletion walk no longer stops at the refused sibling (a refusal now holds back only the
resources the refused one needs), so the bucket IS asked for, and Amazon S3 refuses it because
it still holds objects.
What is missing: the CDK `Custom::S3AutoDeleteObjects` resource that empties the bucket on
delete is never invoked — this emulator runs no custom-resource provider, so nothing calls the
`ServiceToken` Lambda CloudFormation would invoke with the DELETE request. Closing this cause
means invoking a custom resource's provider, which is the subject of the open custom-resource
issues rather than of the deletion walk.
Traces: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty.

## Evidence
Cause A: got {'echo': 'CREATE_COMPLETE'}, expected {'echo': 'ROLLBACK_COMPLETE'}.
Cause B: "the failing update ended {...}" — the replay's dict is truncated before StackStatus.
Cause C: the events of the failed update no longer carry ('Handle', 'CREATE_IN_PROGRESS') and
do carry the stack's UPDATE_FAILED, UPDATE_ROLLBACK_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE;
the assertion compares the whole list, which the replay truncates.
Cause D: "the auto bucket's resource 'Auto29EF6E69' is 'DELETE_FAILED', expected it to have
been deleted despite the sibling's failure".
All values above are from one replay; names, ids and timestamps vary per run — the required
behaviour is the status/event/message template, not these literals.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks listed in trace_ids.
