Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier cloudformation.stack-rollback-events-golden-failed-and-cancelled cloudformation.deep-cloudformation-2-snapshot-volume
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier|run#27|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.stack-rollback-events-golden-failed-and-cancelled|run#4|AssertionError|assert _settle(cfn, stack_id, timeout_s=_cap(ctx, _DEADLINE_S)) == "ROLLBACK_COMPLETE", f"create ended {_stack(cfn, stack_id)!r}"
Issue-Key: cloudformation.deep-cloudformation-2-snapshot-volume|run#28|AssertionError|assert len(snaps) == 1, (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

> _This report carries the replay keys that NOW PASS: cause 1 (the `Rules` section is
> evaluated against the values in force, with an SSM parameter type resolved through Parameter
> Store), cause 3 (a create whose resource-signal wait runs out fails and is rolled back to
> ROLLBACK_COMPLETE, removing what it had made) and cause 7 (`DeletionPolicy: Snapshot` leaves
> exactly one snapshot of the resource behind). The keys of causes 2, 4, 5 and 6 do NOT pass
> and are re-filed under
> `issues/things-that-do-not-work/20261104T170000Z-cloudformation-4-of-the-7-failed-create-causes-remain-open.md`
> so nothing that still fails is recorded as working._

# cloudformation: 7 net-new root causes — failed creates/updates are never rolled back, rollback events are absent, and DeletionPolicy:Snapshot is a no-op

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Seven distinct net-new root causes in the CloudFormation provisioning/rollback engine. Related open issues 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md and 20261029T214321Z-cloudformation-a-failed-create-parks-in-create-failed.md describe neighbouring symptoms but do not carry any of the replay keys below; each cause here must be closed on its own.

Cause 1 of 7: the template `Rules` section is not evaluated.
AWS: recorded behavior only — a template whose `Rules` section asserts a parameter value (here a bootstrap-version Rule whose RuleCondition excludes versions 1..5) fails the create when the assertion is false; the stack settles ROLLBACK_COMPLETE and the rule's AssertDescription appears as the failure reason. Smithy models no per-section validation, so the requirement is the recorded outcome.
Emulator: the stack reaches CREATE_COMPLETE with the excluded version supplied — the Rules section is accepted and ignored.
Why: a client that guards a deploy with Rules (the CDK bootstrap-version guard is the canonical one) gets a stack built on an unsupported bootstrap, instead of a refusal.
Fix: template-processing/validation stage of the CreateStack handler (Rules evaluation before resource provisioning).
Traces: cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier.

Cause 2 of 7: a create whose resource fails settles CREATE_COMPLETE.
AWS: recorded behavior only — when a resource in a create fails, the stack rolls back and ends ROLLBACK_COMPLETE (the five-stack estate expects {alpha..delta: CREATE_COMPLETE, echo: ROLLBACK_COMPLETE}).
Emulator: the failing stack `echo` ends CREATE_COMPLETE, so the whole estate reads as healthy.
Why: a deploy pipeline that gates on stack status treats a broken stack as deployed.
Fix: the create path's failure handling in the stack state machine.
Traces: cloudformation.cfn-topstacks-e2e-top-stacks-resource-breakdown-lifecycle.

Cause 3 of 7: a failed create parks in CREATE_FAILED and emits no ROLLBACK_* events.
AWS: recorded behavior only — after a resource failure (here `WaitCondition timed out. Received 0 conditions when expecting 1`) the stack transitions CREATE_FAILED -> ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE, with one stack-level event per transition carrying that reason.
Emulator: terminal status is CREATE_FAILED with StackStatusReason 'WaitCondition timed out. Received 0 conditions when expecting 1' and no ROLLBACK_IN_PROGRESS/ROLLBACK_COMPLETE events at all.
Why: waiters that poll for ROLLBACK_COMPLETE never finish, and event-driven tooling never sees the rollback.
Fix: stack state machine + DescribeStackEvents emission on the create-failure path.
Traces: cloudformation.stack-rollback-events-golden-failed-and-cancelled.

Cause 4 of 7: an update whose resource fails settles UPDATE_COMPLETE.
AWS: recorded behavior only — a failing update ends UPDATE_FAILED (and then UPDATE_ROLLBACK_* unless rollback is disabled); ContinueUpdateRollback is only valid from UPDATE_ROLLBACK_FAILED/UPDATE_FAILED.
Emulator: DescribeStacks after the failing update reports StackStatus 'UPDATE_COMPLETE' with no StackStatusReason, so the subsequent ContinueUpdateRollback has no failed state to act on.
Why: a client cannot detect, nor recover from, a failed update.
Fix: the UpdateStack handler's failure accounting and the stack state machine.
Traces: cloudformation.stack-continue-update-rollback-client-request-token-marks-its-events, cloudformation.stack-continue-update-rollback-force-rollback-skips-failed-resource.

Cause 5 of 7: a failed update emits no UPDATE_ROLLBACK_* events, and the event stream re-creates resources the update did not touch.
AWS: recorded behavior only — the failed update's events end with the stack's UPDATE_FAILED event followed by UPDATE_ROLLBACK_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE, all stamped with the operation's ClientRequestToken; resources unchanged by the template emit no CREATE_* events.
Emulator: the recorded event list stops at ('<stack>', 'UPDATE_FAILED', 'WaitCondition timed out. Received 0 conditions when expecting 1') and additionally contains CREATE_IN_PROGRESS/CREATE_COMPLETE events for the pre-existing `Handle` resource.
Why: rollback is invisible to event consumers and the resource-level event history is wrong.
Fix: DescribeStackEvents emission for the update/rollback path; the update planner must not re-create unchanged resources.
Traces: cloudformation.stack-rollback-stack-client-request-token-marks-rollback-events.

Cause 6 of 7: rollback does not delete siblings that were already created.
AWS: recorded behavior only — when one resource of a create fails, resources already created in the same operation are deleted during rollback and are gone from the stack's resource list.
Emulator: the sibling bucket's resource 'Auto29EF6E69' is still 'CREATE_COMPLETE' after the failure.
Why: a failed deploy leaves live, billable, name-claiming resources behind, and a retry then collides with them.
Fix: the rollback cleanup step of the stack state machine.
Traces: cloudformation.cdk-s3-auto-delete-objects-empties-past-destroy-manual-bucket-blocks-on-bucketnotempty.

Cause 7 of 7: `DeletionPolicy: Snapshot` takes no snapshot.
AWS: recorded behavior only — deleting a resource that declares DeletionPolicy: Snapshot leaves exactly one snapshot of that physical resource behind.
Emulator: zero snapshots exist for the volume after the delete (`DeletionPolicy:Snapshot must leave exactly one snapshot of <volume-id>; got []`).
Why: data a client expects to survive stack deletion is destroyed silently.
Fix: the delete path's DeletionPolicy handling in the resource provisioner.
Traces: cloudformation.deep-cloudformation-2-snapshot-volume.

## Evidence
Cause 1: expected ROLLBACK_COMPLETE, got CREATE_COMPLETE — "stack '<name>' reached 'CREATE_COMPLETE' with bootstrap version '5' against a Rule excluding ['1','2','3','4','5']; expected ROLLBACK_COMPLETE".
Cause 2: got {'echo': 'CREATE_COMPLETE'}, expected {'echo': 'ROLLBACK_COMPLETE'}.
Cause 3: DescribeStacks -> StackStatus 'CREATE_FAILED', StackStatusReason 'WaitCondition timed out. Received 0 conditions when expecting 1'; no ROLLBACK_* events.
Cause 4: DescribeStacks after the failing update -> StackStatus 'UPDATE_COMPLETE', no StackStatusReason, LastOperations [{'OperationType': 'UPDATE_STACK', ...}].
Cause 5: events of the failed update end at ('<stack>', 'UPDATE_FAILED', 'WaitCondition timed out. Received 0 conditions when expecting 1', '<token>') and include ('Handle','CREATE_IN_PROGRESS'/'CREATE_COMPLETE') for an untouched resource.
Cause 6: "the auto bucket's resource 'Auto29EF6E69' is 'CREATE_COMPLETE', expected it to have been deleted despite the sibling's failure".
Cause 7: "DeletionPolicy:Snapshot must leave exactly one snapshot of vol-...; got []".
All values above are from one replay; names, ids and timestamps vary per run — the required behaviour is the status/event/message template, not these literals.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks listed in trace_ids: 1) create a stack whose template has a Rules assertion that the supplied parameter violates; 2) create a stack whose resource fails and poll DescribeStacks/DescribeStackEvents; 3) create a stack with a WaitCondition that times out; 4) update a stack so one resource fails, then call ContinueUpdateRollback; 5) read DescribeStackEvents for that failed update; 6) create a stack where one of two resources fails and list stack resources; 7) delete a stack whose resource declares DeletionPolicy: Snapshot and list snapshots.
