Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events|run#12|AssertionError|assert rows == [
Issue-Commit: aa67ef9b7926b9d86487a1c336b627f99e898509

# cloudformation: 5 net-new root causes in the CancelUpdateStack rollback path — WaitCondition auto-completes, unchanged resources are re-created, a spurious stack UPDATE_FAILED event, wrong rollback reason wording, and the in-flight resource is never failed or deleted

> _Note: this issue was identified against the code base at commit `aa67ef9b7926b9d86487a1c336b627f99e898509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelUpdateStack

## Description
Five net-new root causes of service `cloudformation`, all grounded in the one failing behavioral check listed in trace_ids. The replay report marked this check neither ALREADY FILED nor OWNED. Two open issues describe adjacent shapes and are named per cause so a fixer can close them together: 20261101T212352Z-cloudformation-3-net-new-behavioral-root-causes-waitconditionhandle.md (the WaitConditionHandle physical id is not a presigned URL) and 20261025T015306Z-cloudformation-is-not-region-scoped-stack-names-and.md (which recorded CancelUpdateStack as a NotImplemented stub — it now answers, so the defect filed here is the behaviour of that answer, not its absence). A fixer must close all five causes before submitting.

=== Cause 1 of 5: an AWS::CloudFormation::WaitCondition reaches CREATE_COMPLETE with no signal, so nothing holds an update in progress ===
AWS: Smithy models no CloudFormation resource types (ResourceStatus/StackStatus are plain enums; CancelUpdateStack takes StackName required + ClientRequestToken and returns Unit), so this is recorded behavior only. An AWS::CloudFormation::WaitCondition with Count 1 and Timeout 600 stabilises only when that many SUCCESS signals have been PUT to its handle's presigned URL; until then the resource stays at CREATE_IN_PROGRESS with ResourceStatusReason "Resource creation Initiated" and the enclosing stack operation stays UPDATE_IN_PROGRESS. Nothing in the stream advances the WaitCondition on its own.
Emulator: the WaitCondition emits CREATE_IN_PROGRESS, CREATE_IN_PROGRESS/"Resource creation Initiated" and then CREATE_COMPLETE immediately, with no signal ever sent, so the update settles by itself and there is no in-progress window for CancelUpdateStack to interrupt.
Why: a client cannot hold a stack behind a signal at all — the standard bootstrap/user-data signalling pattern, cfn-signal, CreationPolicy/ResourceSignal gating and every check that needs a long-running update to cancel, pause or roll back is unusable, and a stack reports CREATE/UPDATE_COMPLETE while the work the signal represents has not happened.
Fix: the CloudFormation provisioning engine's stabilisation barrier for AWS::CloudFormation::WaitCondition — the resource must remain CREATE_IN_PROGRESS until the modelled Count of SUCCESS signals arrives (or the Timeout elapses, which fails it), rather than being completed by the provisioner. Related open issue: 20261101T212352Z-cloudformation-3-net-new-behavioral-root-causes-waitconditionhandle.md cause 1 covers the handle's physical id not being a presigned URL; it does not cover the WaitCondition completing without a signal.
Traces: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events.

=== Cause 2 of 5: a resource that is identical in the old and new template is re-provisioned on the update and again on the rollback ===
AWS: recorded behavior only. On UpdateStack, CloudFormation diffs the template and touches only resources whose declaration changed; an unmodified resource produces no stack events and keeps its physical id. The recorded update adds one resource to a template whose other resource (an AWS::CloudFormation::WaitConditionHandle, logical id `Handle`) is byte-identical in both templates, and AWS emits no `Handle` events in either the update or the rollback leg.
Emulator: `Handle` gets a full re-create triplet — CREATE_IN_PROGRESS (no reason), CREATE_IN_PROGRESS/"Resource creation Initiated", CREATE_COMPLETE — under the update's ClientRequestToken, and the identical triplet again under the cancel's token during the rollback. The logical ids and reasons are the fixed part; the per-run token values are not.
Why: every update looks like a full replacement. A client reading DescribeStackEvents cannot tell what an update actually changed, drift/change-set previews disagree with the event stream, and any resource whose re-creation has side effects (a new physical id, a fresh handle URL, data loss on a stateful resource) is silently rebuilt on an unrelated update.
Fix: the update path's template diff / resource-change classification in the CloudFormation provisioning engine — resources with no property, metadata or policy change must be left alone and emit no events, on the forward update and on the rollback alike.
Traces: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events.

=== Cause 3 of 5: the cancelled update emits a stack-level UPDATE_FAILED event AWS never emits, and omits UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS ===
AWS: recorded behavior only (ResourceStatus/StackStatus are modelled enums but the per-operation sequence is not in the model; StackStatus contains UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS). After CancelUpdateStack during an update, the stack's own events are exactly UPDATE_IN_PROGRESS, then UPDATE_ROLLBACK_IN_PROGRESS, then UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS, then UPDATE_ROLLBACK_COMPLETE. There is no stack-level UPDATE_FAILED event in a user-cancelled update.
Emulator: it inserts a stack event with ResourceStatus UPDATE_FAILED (carrying the update's ClientRequestToken) between UPDATE_IN_PROGRESS and UPDATE_ROLLBACK_IN_PROGRESS, and it never emits UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS at all — it goes straight from UPDATE_ROLLBACK_IN_PROGRESS to UPDATE_ROLLBACK_COMPLETE.
Why: tooling that classifies a stack operation from its events (CDK/CLI progress printers, event-driven automation, CloudWatch-Events-style consumers) sees a failure where AWS reports a user cancellation, and never sees the cleanup phase, so a consumer that waits for UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS before reading post-rollback state waits forever.
Fix: the stack-level event emission in the CancelUpdateStack / update-rollback state machine — drop the UPDATE_FAILED event on a user-initiated cancel and emit the UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS transition before UPDATE_ROLLBACK_COMPLETE.
Traces: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events.

=== Cause 4 of 5: the rollback's ResourceStatusReason is the emulator's own sentence instead of AWS's wording, and is set on events AWS leaves without a reason ===
AWS: recorded behavior only (StackEvent$ResourceStatusReason is an unconstrained optional string documented as the "Success/failure message associated with the resource"). For a cancelled update the stack's UPDATE_ROLLBACK_IN_PROGRESS event carries ResourceStatusReason exactly `User Initiated` — the same fixed text UPDATE_IN_PROGRESS carries when a user starts the update — and the UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE events carry no ResourceStatusReason at all (the member is absent). This message has no variable parts.
Emulator: it writes the sentence `Stack update cancelled by user.` as the ResourceStatusReason of the stack's UPDATE_ROLLBACK_IN_PROGRESS event (where AWS writes `User Initiated`), of the spurious UPDATE_FAILED event from cause 3, and of the terminal UPDATE_ROLLBACK_COMPLETE event (where AWS sets no reason).
Why: the reason string is a requirement, not decoration — a client that branches on `User Initiated` to distinguish a user cancellation from a provisioning failure, or that treats a non-empty reason on a terminal event as an error detail, takes the wrong branch against this emulator.
Fix: the ResourceStatusReason the CancelUpdateStack / update-rollback event writer stamps on stack-level events — `User Initiated` on UPDATE_ROLLBACK_IN_PROGRESS, and no reason on the cleanup and complete transitions.
Traces: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events.

=== Cause 5 of 5: the resource the cancelled update was still creating is never failed and never deleted ===
AWS: recorded behavior only. CancelUpdateStack aborts the in-flight resource and then removes it as part of the rollback, so the resource that the update was adding produces, under the cancel's ClientRequestToken, CREATE_FAILED with ResourceStatusReason exactly `Resource creation cancelled`, then DELETE_IN_PROGRESS and DELETE_COMPLETE (both with no reason), leaving only the resources the pre-update template declared.
Emulator: the event stream after the cancel contains no CREATE_FAILED event and no DELETE_IN_PROGRESS/DELETE_COMPLETE events for the resource the update added — the rollback emits only stack-level transitions plus the spurious re-create of the unchanged resource from cause 2, so the aborted resource is neither marked failed nor released.
Why: a cancelled update leaves the stack's recorded resource set out of step with its last stable template. A client that cancels an update expecting the partially created resource to be gone keeps paying for / tripping over an orphan, and cannot distinguish "cancelled before it was created" from "created successfully".
Fix: the resource-level rollback in the CancelUpdateStack / update-rollback engine — abort the in-flight resource with CREATE_FAILED / `Resource creation cancelled` and then delete it, emitting its DELETE_IN_PROGRESS and DELETE_COMPLETE events under the cancel's ClientRequestToken.
Traces: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events.

## Evidence
Check: cloudformation.stack-cancel-update-client-request-token-marks-rollback-events [mismatch]
Scenario: CreateStack of {"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}} (ClientRequestToken create_tok), then UpdateStack adding {"Wait":{"Type":"AWS::CloudFormation::WaitCondition","Properties":{"Handle":{"Ref":"Handle"},"Timeout":"600","Count":1}}} (ClientRequestToken update_tok), then CancelUpdateStack (ClientRequestToken cancel_tok). Rows below are (LogicalResourceId, ResourceStatus, ResourceStatusReason, ClientRequestToken) from DescribeStackEvents, oldest first, for the events after the create.

Expected (AWS):
  (<stack>, UPDATE_IN_PROGRESS, 'User Initiated', update_tok)
  (Wait, CREATE_IN_PROGRESS, None, update_tok)
  (Wait, CREATE_IN_PROGRESS, 'Resource creation Initiated', update_tok)
  (<stack>, UPDATE_ROLLBACK_IN_PROGRESS, 'User Initiated', cancel_tok)
  (Wait, CREATE_FAILED, 'Resource creation cancelled', cancel_tok)
  (<stack>, UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS, None, cancel_tok)
  (Wait, DELETE_IN_PROGRESS, None, cancel_tok)
  (Wait, DELETE_COMPLETE, None, cancel_tok)
  (<stack>, UPDATE_ROLLBACK_COMPLETE, None, cancel_tok)

Actual (emulator), from the AssertionError "events of the cancelled update":
  (<stack>, UPDATE_IN_PROGRESS, 'User Initiated', update_tok)
  (Handle, CREATE_IN_PROGRESS, None, update_tok)                      <- cause 2, unchanged resource re-created
  (Handle, CREATE_IN_PROGRESS, 'Resource creation Initiated', update_tok)
  (Handle, CREATE_COMPLETE, None, update_tok)
  (Wait, CREATE_IN_PROGRESS, None, update_tok)
  (Wait, CREATE_IN_PROGRESS, 'Resource creation Initiated', update_tok)
  (Wait, CREATE_COMPLETE, None, update_tok)                           <- cause 1, no signal was ever sent
  (<stack>, UPDATE_FAILED, 'Stack update cancelled by user.', update_tok)   <- cause 3 (event AWS never emits) + cause 4 (wording)
  (<stack>, UPDATE_ROLLBACK_IN_PROGRESS, 'Stack update cancelled by user.', cancel_tok)  <- cause 4: AWS says 'User Initiated'
  (Handle, CREATE_IN_PROGRESS, None, cancel_tok)                      <- cause 2 again, on the rollback leg
  (Handle, CREATE_IN_PROGRESS, 'Resource creation Initiated', cancel_tok)
  (Handle, CREATE_COMPLETE, None, cancel_tok)
  (<stack>, UPDATE_ROLLBACK_COMPLETE, 'Stack update cancelled by user.', cancel_tok)     <- cause 3 (no CLEANUP_IN_PROGRESS) + cause 4 (AWS sets no reason)

No (Wait, CREATE_FAILED, 'Resource creation cancelled', cancel_tok), (Wait, DELETE_IN_PROGRESS, ...) or (Wait, DELETE_COMPLETE, ...) event appears anywhere — cause 5.
Stack name, the hex suffixes in it and the three ClientRequestToken values are per-run values and are not part of any requirement; the fixed text is the statuses, the reason strings 'User Initiated' / 'Resource creation Initiated' / 'Resource creation cancelled', and the absence of a reason on the cleanup and complete events.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation.stack-cancel-update-client-request-token-marks-rollback-events from suite simparity-behavioral-aws-v1.
2. By hand: CreateStack(StackName=<name>, TemplateBody={"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}, ClientRequestToken="create-<hex>") and wait for CREATE_COMPLETE. Note how many events exist.
3. UpdateStack(StackName=<name>, ClientRequestToken="update-<hex>") with the same template plus {"Wait":{"Type":"AWS::CloudFormation::WaitCondition","Properties":{"Handle":{"Ref":"Handle"},"Timeout":"600","Count":1}}}. Poll DescribeStackEvents: observe (a) CREATE_IN_PROGRESS/CREATE_COMPLETE events for the unchanged `Handle` (cause 2) and (b) `Wait` reaching CREATE_COMPLETE although no signal was PUT, so the update settles on its own (cause 1).
4. While the update is in flight, call CancelUpdateStack(StackName=<name>, ClientRequestToken="cancel-<hex>") and wait until the stack status stops ending in _IN_PROGRESS.
5. Page DescribeStackEvents oldest-first from the point noted in step 2 and compare (LogicalResourceId, ResourceStatus, ResourceStatusReason, ClientRequestToken) against the expected list in Evidence: observe the extra stack UPDATE_FAILED event and the missing UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS (cause 3), the reason 'Stack update cancelled by user.' where AWS has 'User Initiated' or no reason (cause 4), and the absence of any CREATE_FAILED/DELETE_IN_PROGRESS/DELETE_COMPLETE event for `Wait` (cause 5).
6. DescribeStackResources(StackName=<name>) and check that only the pre-update resource remains.
