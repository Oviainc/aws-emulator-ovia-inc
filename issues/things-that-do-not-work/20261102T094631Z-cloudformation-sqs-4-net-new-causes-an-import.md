Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails cloudformation__sqs.cfn-stack-policy-protected-resource-replacement-rejected cloudformation__sqs.stack-event-detailed-status-configuration-complete-precedes-create-complete
Issue-Key: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one|run#20|AssertionError|assert len(failed_cs) == 1, f"expected one FAILED change set, got {failed_cs!r}"
Issue-Key: cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails|run#26|AssertionError|assert re.fullmatch(
Issue-Key: cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails|run#23|AssertionError|assert rc == 1, f"`cdk rollback` with the queue gone exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__sqs.cfn-stack-policy-protected-resource-replacement-rejected|run#19|AssertionError|assert protected_events == [
Issue-Key: cloudformation__sqs.stack-event-detailed-status-configuration-complete-precedes-create-complete|run#6|AssertionError|assert rows == [
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__sqs: 4 net-new causes — an IMPORT change set for a missing resource never fails, rollback ignores force/orphan, stack-policy denial events are wrong, and stack events carry no DetailedStatus

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Four net-new root causes in CloudFormation's import, rollback, stack-policy and stack-event surfaces.

Cause 1 of 4: an IMPORT change set naming a resource that does not exist never reaches FAILED.
AWS: CreateChangeSet with ChangeSetType=IMPORT whose ResourcesToImport names an object that does not exist produces a change set in status FAILED (with a StatusReason naming the missing resource); importing an existing queue succeeds and adopts it without recreating it. Smithy (cloudformation CreateChangeSet/DescribeChangeSet -> ChangeSetStatus FAILED) models the status; the reason text is recorded behavior only.
Emulator: no FAILED change set is ever produced — 'expected one FAILED change set, got []' — and the recorded sibling leg shows CreateChangeSet with ChangeSetType IMPORT answered NotImplemented, so resource import has no failure path at all.
Why: a client adopting existing infrastructure cannot tell a typo in ResourcesToImport from a successful import.
Fix: the IMPORT path of the change-set creator (existence check per entry, FAILED status with the reason).
Traces: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one.

Cause 2 of 4: RollbackStack does not honour the force / orphan semantics for a resource whose rollback cannot succeed.
AWS: rolling back a stack whose resource was deleted out of band fails that resource's rollback; with the force/orphan options the operation completes past it (the resource is skipped/orphaned) and the stack reaches a terminal rolled-back status. Smithy (cloudformation RollbackStack) models the operation but not this per-resource behavior — recorded behavior only.
Emulator: the force leg leaves the rollback failed with the provider's error 'Resource handler returned message: "The specified queue does not exist." (RequestToken: <uuid>, HandlerErrorCode: NotFound)' instead of completing past the resource, and the orphan leg instead exits 0 immediately ('`cdk rollback` with the queue gone exited 0') without the failing rollback the orphan option exists to skip.
Why: a stack whose resource was deleted outside CloudFormation can never be recovered the way the documented options promise; operators lose the only escape hatch out of a wedged rollback.
Fix: the RollbackStack executor's per-resource failure handling and its force/orphan (ResourcesToSkip) semantics.
Traces: cloudformation__sqs.cdk-rollback-force-completes-past-the-resource-whose-rollback-fails, cloudformation__sqs.cdk-rollback-orphan-skips-the-resource-whose-rollback-fails.

Cause 3 of 4: a stack-policy denial produces the wrong event stream for the protected resource.
AWS: when a stack policy denies the replacement of a protected resource, that resource's events during the update are exactly two: UPDATE_FAILED with reason 'Action denied by stack policy: Statement [#<n>] does not allow [Update:Replace] for resource [LogicalResourceId/<LogicalId>];' — fixed text whose variable parts are the denying statement's index as CloudFormation numbers them and the resource's logical id — followed by UPDATE_COMPLETE with reason 'Resource skipped during UpdateRollback'; the stack ends UPDATE_ROLLBACK_COMPLETE and the resource is never replaced. Smithy models ResourceStatus/ResourceStatusReason but not the texts — recorded behavior only.
Emulator: the resource's events are ('UPDATE_IN_PROGRESS', None) then ('UPDATE_FAILED', 'Action denied by stack policy: Statement [#0] does not allow [Update:Replace] for resource [LogicalResourceId/Protected];') — an UPDATE_IN_PROGRESS AWS does not emit for a denied resource, the denying statement identified as [#0] where the recording shows AWS naming the other statement index, and no 'Resource skipped during UpdateRollback' event at all.
Why: a client auditing why an update was blocked sees the wrong statement blamed and no record that the resource was skipped during the rollback.
Fix: the stack-policy evaluation and event emission in the update executor (statement attribution and the UpdateRollback skip event).
Traces: cloudformation__sqs.cfn-stack-policy-protected-resource-replacement-rejected.

Cause 4 of 4: stack events carry no DetailedStatus, so CONFIGURATION_COMPLETE is never reported.
AWS: a resource's provisioning emits a CREATE_IN_PROGRESS event whose DetailedStatus is CONFIGURATION_COMPLETE before the resource's CREATE_COMPLETE, letting callers see configuration finish ahead of stabilization. Smithy (cloudformation DescribeStackEvents -> StackEvent.DetailedStatus) models the member.
Emulator: the queue's events are [('CREATE_IN_PROGRESS', None, None), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', None), ('CREATE_COMPLETE', None, None)] — DetailedStatus is absent on every event.
Why: clients that wait on CONFIGURATION_COMPLETE to start dependent work never see it.
Fix: populate StackEvent.DetailedStatus in the stack-event emitter.
Traces: cloudformation__sqs.stack-event-detailed-status-configuration-complete-precedes-create-complete.

## Evidence
Cause 1 — Actual: 'AssertionError: expected one FAILED change set, got []'; sibling leg recorded ClientError (NotImplemented) 'operation CreateChangeSet with ChangeSetType IMPORT (importing existing resources) is scaffolded but not implemented in this emulator'.
Cause 2 — Actual: 'the queue's rollback failed with \'Resource handler returned message: "The specified queue does not exist." (RequestToken: ..., HandlerErrorCode: NotFound)\'' (force leg) and '`cdk rollback` with the queue gone exited 0' (orphan leg).
Cause 3 — Actual protected-resource events: [('UPDATE_IN_PROGRESS', None), ('UPDATE_FAILED', 'Action denied by stack policy: Statement [#0] does not allow [Update:Replace] for resource [LogicalResourceId/Protected];')]. Expected: [('UPDATE_FAILED', 'Action denied by stack policy: Statement [#1] does not allow [Update:Replace] for resource [LogicalResourceId/Protected];'), ('UPDATE_COMPLETE', 'Resource skipped during UpdateRollback')].
Cause 4 — Actual (status, reason, DetailedStatus) triples: [('CREATE_IN_PROGRESS', None, None), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', None), ('CREATE_COMPLETE', None, None)]. Expected a CREATE_IN_PROGRESS event with DetailedStatus 'CONFIGURATION_COMPLETE' before CREATE_COMPLETE.

## Steps to Reproduce
Cause 1: CreateChangeSet ChangeSetType=IMPORT with ResourcesToImport naming (a) an existing queue, (b) a queue that does not exist; poll DescribeChangeSet for status. Cause 2: deploy a stack, delete its queue out of band, then RollbackStack / `cdk rollback` with the force and with the orphan option. Cause 3: SetStackPolicy denying Update:Replace for one logical id, then UpdateStack with a change that replaces it; read DescribeStackEvents for that logical id and DescribeStacks for the stack status. Cause 4: CreateStack with a queue and read DescribeStackEvents including DetailedStatus. Replay the five check ids in trace_ids.
