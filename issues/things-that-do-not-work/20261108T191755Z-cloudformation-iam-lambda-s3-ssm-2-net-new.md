Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-rollback-blocked-when-prior-code-object-deleted cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-update-disable-rollback-keeps-applied-param-until-rollback-stack
Issue-Key: cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-rollback-blocked-when-prior-code-object-deleted|run#39|AssertionError|assert status == "UPDATE_ROLLBACK_FAILED", (
Issue-Key: cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-update-disable-rollback-keeps-applied-param-until-rollback-stack|run#38|AssertionError|assert rolled == [
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudformation__iam__lambda__s3__ssm: 2 net-new causes — a Lambda Function rollback never re-fetches the previous Code.S3Key (deleted object still yields UPDATE_ROLLBACK_COMPLETE), and a DisableRollback update emits no per-resource rollback events for the failed resource

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two distinct net-new root causes in CloudFormation's update-failure/rollback path for AWS::Lambda::Function resources whose Code lives in S3. The only open issue for this service pairing, 20261105T081225Z-cloudformation-iam-lambda-s3-ssm-invoke-of-a.md (Invoke of a CFN-created S3-code function refused 502 InvalidZipFileException), does not cover either cause and carries neither replay key — both checks below get past create and invoke the v1 function successfully, and fail later in the update/rollback phase.

Cause 1 of 2: rolling a Lambda Function back to its previous Code does not re-read the previous object from S3, so a rollback that AWS cannot perform is reported as having succeeded.
AWS: recorded behavior only (Smithy models no error for this — it is a provisioning outcome surfaced through DescribeStacks/DescribeStackEvents). When an UpdateStack of an AWS::Lambda::Function fails, CloudFormation rolls the resource back by re-applying the previous template's Code.S3Bucket/Code.S3Key, which means it FETCHES that object again. If the previous object has been deleted, the rollback of that resource fails: the stack ends in StackStatus `UPDATE_ROLLBACK_FAILED`, DescribeStackEvents carries exactly one stack-level `UPDATE_ROLLBACK_FAILED` event whose ResourceStatusReason names only the failed logical id in brackets, and the Function records TWO `UPDATE_FAILED` events — one for the forward update and one DURING the rollback (after the stack's `UPDATE_ROLLBACK_IN_PROGRESS` event) — each with S3's `NoSuchKey` surfaced in ResourceStatusReason. A stack in UPDATE_ROLLBACK_FAILED then refuses UpdateStack with ValidationError `Stack:<stack-id> is in UPDATE_ROLLBACK_FAILED state and can not be updated.` (fixed text around the stack id and the state name taken from state), and ContinueUpdateRollback converges to UPDATE_ROLLBACK_COMPLETE only once the previous object is restored.
Emulator: the same update settles in `UPDATE_ROLLBACK_COMPLETE` with StackStatusReason `The following resource(s) failed to update: [Fn]. ` even though the previous Code object was deleted before the update — the rollback is performed from cached/internal state and never touches S3, so the second (rollback-time) Function failure and the whole UPDATE_ROLLBACK_FAILED branch (its stack-level event, the UpdateStack refusal in that state, and the ContinueUpdateRollback recovery) never happen.
Why: a client is told a rollback succeeded when the artifact it would have been rolled back from no longer exists. Deployment tooling that keys on UPDATE_ROLLBACK_FAILED (to page an operator, to restore the artifact and call ContinueUpdateRollback) never fires, and the stack is reported as consistent with a previous template that can no longer be re-materialised.
Fix: the rollback path of the CloudFormation provisioner for AWS::Lambda::Function — the previous-state restore must re-resolve Code.S3Bucket/S3Key through S3 and fail the resource (surfacing NoSuchKey) when the object is gone, plus the stack-level UPDATE_ROLLBACK_FAILED state machine it feeds (event emission, the UpdateStack state guard and ContinueUpdateRollback).
Traces: cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-rollback-blocked-when-prior-code-object-deleted.

Cause 2 of 2: with DisableRollback=True, the failed resource is never rolled back and no per-resource rollback events are emitted.
AWS: recorded behavior only. An UpdateStack with DisableRollback=True whose Lambda Function update fails (missing Code.S3Key) stops the stack in `UPDATE_FAILED` and keeps already-applied sibling changes (the SSM parameter keeps its new value), but CloudFormation still rolls back the FAILED resource itself and says so in the event stream: for the failing logical id it emits `UPDATE_ROLLBACK_IN_PROGRESS` with ResourceStatusReason `Rolling back the failed resource only.` followed by `UPDATE_ROLLBACK_COMPLETE` with ResourceStatusReason `Rollback succeeded for the failed resources.` — those two reason sentences are fixed text, and the logical id is taken from the template.
Emulator: the stack does reach UPDATE_FAILED, but DescribeStackEvents for that update contains NO event whose ResourceStatus contains `ROLLBACK` at all (the observed list is empty), so neither the per-resource rollback nor its two reason sentences are produced.
Why: an operator or tool reading the event stream cannot tell that the failed resource was returned to its previous configuration while the rest of the update was intentionally left applied — exactly the distinction DisableRollback exists to expose. Tools that wait for the resource-scoped UPDATE_ROLLBACK_COMPLETE before deciding whether to fix-forward or RollbackStack hang or mis-decide.
Fix: the DisableRollback branch of the UpdateStack provisioner's failure handling — it must perform the failed-resource-only rollback and emit the two resource-level events with AWS's reason text, in the stack's event store.
Traces: cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-update-disable-rollback-keeps-applied-param-until-rollback-stack.

## Evidence
Cause 1 — check cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-rollback-blocked-when-prior-code-object-deleted:
AssertionError: rolling the Function back must re-fetch the deleted v1 object and fail; stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Fn]. ')
Sequence: CreateStack (Function Code from s3://<bucket>/code/v1.zip) -> CREATE_COMPLETE and the function serves 'one'; DeleteObject on code/v1.zip (HeadObject then 404); UpdateStack pointing Code.S3Key at code/does-not-exist.zip.
Expected: StackStatus 'UPDATE_ROLLBACK_FAILED'; one stack-level UPDATE_ROLLBACK_FAILED event blaming [Fn]; two Fn UPDATE_FAILED events, each surfacing 'NoSuchKey', the second after the stack's UPDATE_ROLLBACK_IN_PROGRESS event; then UpdateStack refused ValidationError 'Stack:<stack-id> is in UPDATE_ROLLBACK_FAILED state and can not be updated.'; then, with code/v1.zip restored, ContinueUpdateRollback -> UPDATE_ROLLBACK_COMPLETE.
Actual: StackStatus 'UPDATE_ROLLBACK_COMPLETE' with reason 'The following resource(s) failed to update: [Fn]. ' — only one Fn failure, no rollback-time S3 read.

Cause 2 — check cloudformation__iam__lambda__s3__ssm.deploy-lambda-3-update-disable-rollback-keeps-applied-param-until-rollback-stack:
AssertionError: DisableRollback must roll back only the failed Function, but saw []
Sequence: CreateStack (Function Code from s3://<bucket>/code/v1.zip, SSM parameter value 'v1') -> CREATE_COMPLETE; UpdateStack(DisableRollback=True) with Code.S3Key=code/does-not-exist.zip and the parameter set to a new value.
Expected, among the events of that update: ('Fn','UPDATE_ROLLBACK_IN_PROGRESS','Rolling back the failed resource only.') followed by ('Fn','UPDATE_ROLLBACK_COMPLETE','Rollback succeeded for the failed resources.').
Actual: the filtered list of events whose ResourceStatus contains 'ROLLBACK' is [] — no such events exist.
Bucket names, stack/function/parameter names, stack ids and object keys above are per-run fillers; the requirements are the stack statuses, the event statuses and the quoted reason sentences.

## Steps to Reproduce
Replay the two checks in trace_ids. Cause 1: create a stack with an AWS::Lambda::Function whose Code points at an S3 object (plus any sibling resource, e.g. AWS::SSM::Parameter); wait for CREATE_COMPLETE and invoke the function; delete the S3 object the function was deployed from; UpdateStack with Code.S3Key pointing at a key that does not exist; poll DescribeStacks — AWS lands in UPDATE_ROLLBACK_FAILED (and the Function logs a second UPDATE_FAILED with NoSuchKey during the rollback), the emulator lands in UPDATE_ROLLBACK_COMPLETE. Then call UpdateStack again (AWS refuses ValidationError 'Stack:<id> is in UPDATE_ROLLBACK_FAILED state and can not be updated.'), restore the object and call ContinueUpdateRollback. Cause 2: from the same CREATE_COMPLETE stack (with the original object intact), call UpdateStack(DisableRollback=True) with a nonexistent Code.S3Key and a changed sibling parameter; wait for UPDATE_FAILED and read DescribeStackEvents — no event with a ROLLBACK status is emitted for the failed Function.
