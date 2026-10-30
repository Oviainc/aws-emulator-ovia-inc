Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs-realtime.stop-composition-terminal-state-conflict-rejected
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime.stop-composition-terminal-state-conflict-rejected|run#17|AssertionError|assert message == f"Resource: {comp_arn} is in a transitioning state", message
Issue-Commit: f436928141e9968ccb87a436b810c90c79edbabf

# StopComposition's terminal-state ConflictException uses the emulator's own sentence ("is already STOPPED") instead of AWS's "is in a transitioning state"

> _Note: this issue was identified against the code base at commit `f436928141e9968ccb87a436b810c90c79edbabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopComposition

## Description
One net-new root cause in the ivs-realtime emulator, grounded in one failing behavioral check of this suite.

Cause 1 of 1 — the message of StopComposition's ConflictException for a composition that is no longer composing is worded with the emulator's own template, and leaks the composition's internal state value.

AWS: Smithy: StopComposition takes only `arn` (CompositionArn, @length 1..128, @pattern ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:composition/[a-zA-Z0-9-]+$), answers an empty StopCompositionResponse at HTTP 200, and models ConflictException (HTTP 409, "Updating or deleting a resource can cause an inconsistent state") alongside ResourceNotFoundException (404) and ValidationException (400). The model does not state the refusal text, so for the wording this is recorded behavior only: the recording shows a second StopComposition against a composition already in the terminal STOPPED state refused with HTTP 409, error Code "ConflictException" and Message "Resource: <composition arn> is in a transitioning state". The message template is the fixed text "Resource: " + the composition ARN taken from the request + the fixed text " is in a transitioning state" — the ARN is the only variable part, and AWS names NO state value in it. The refusal has no effect: the composition stays STOPPED and its endTime is unchanged.

Emulator: the code and status are right (ConflictException/409) but the sentence is different. For the same call it answers Message "Resource: <composition arn> is already STOPPED" — fixed text " is already " plus the record's own state value interpolated as a second variable part. src/ops/stop_composition.rs:176-183 refuses any composition for which `is_running()` is false by building the sentence through `crate::state::composition_not_running_message(&composition.arn, &composition.state)`, i.e. the state is deliberately formatted into the message; the AWS template has no slot for it.

Why: the error message is part of the refusal contract clients log, assert on and match. A client (or an operator runbook) that keys the "stop raced another stop / the composition is already terminal" condition on AWS's sentence never matches, and because the emulator's variant names a concrete state it also varies with the record's internal lifecycle value (STOPPING vs STOPPED vs FAILED) where AWS always sends one fixed sentence — so even a substring match on the stable part fails.

Fix: the message builder for this refusal — `crate::state::composition_not_running_message` in src/state.rs, as called from src/ops/stop_composition.rs — must emit AWS's template ("Resource: " + the request's composition ARN + " is in a transitioning state") and must not interpolate the composition's state. The classification (ConflictException, 409) and the no-effect behaviour already match and must be preserved.

Related, not duplicate: the open issue 20261029T194619Z-ivs-realtime-refusals-use-the-wrong-error-codes.md covers other refusal codes/templates of this crate (tagging, CreateParticipantToken, CreateIngestConfiguration, CreateEncoderConfiguration, GetStageSession) and does not carry StopComposition. The open issue 20261029T194619Z-ivs-realtime-composition-ingest-storage-lifecycle-diverges-a.md names this check under its Cause 1 (a stopped composition being reclaimed ~1s after the stop, so GetComposition 404s); that failure mode is no longer what the check hits — the replay now reaches the repeat-stop assertion with the composition readable and STOPPED, and fails only on this message. The replay report does not mark this check as owned by either issue.

Traces: ivs-realtime.stop-composition-terminal-state-conflict-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, ivs-realtime 44/69 passed. This was the only failing check not already filed (as an unimplemented-stub blocker) or owned by an existing open issue.

Cause 1: [mismatch] ivs-realtime.stop-composition-terminal-state-conflict-rejected —
  check_ivs_realtime_stop_composition_terminal_state_conflict_rejected.py, line 102:
    assert message == f"Resource: {comp_arn} is in a transitioning state", message
  AssertionError: Resource: arn:aws:ivs:us-west-2:000000000000:composition/000004840a67 is already STOPPED

The preceding assertions in the same check passed: the first StopComposition returned 200, GetComposition polled to state "STOPPED" with an endTime, and the repeat StopComposition was refused with Code "ConflictException". Only the Message diverges. (The ARN and id above are one run's values; the requirement is the template, not those digits.)

Code: src/ops/stop_composition.rs:176-183 — `if !composition.is_running() { let message = crate::state::composition_not_running_message(&composition.arn, &composition.state); return Err(crate::state::conflict(StopCompositionErrorModeled::ConflictException, message)); }`.

Smithy (services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/smithy-model/com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json): StopComposition -> POST /StopComposition, success 200, errors include ConflictException (httpError 409).

## Steps to Reproduce
Cause 1:
1. CreateStage(name=<n>) and CreateEncoderConfiguration(name=<n>).
2. Create an S3 bucket in the request region and CreateStorageConfiguration(name=<n>, s3={"bucketName": <bucket>}).
3. StartComposition(stageArn=<stage arn>, destinations=[{"s3": {"storageConfigurationArn": <storage arn>, "encoderConfigurationArns": [<encoder arn>]}}]) and poll GetComposition until state == "ACTIVE".
4. StopComposition(arn=<composition arn>) — 200 — then poll GetComposition until state == "STOPPED" and note endTime.
5. Call StopComposition(arn=<composition arn>) a second time.
   Expected: HTTP 409, Code "ConflictException", Message "Resource: <composition arn> is in a transitioning state" (fixed text around the request's ARN, no state value).
   Actual: HTTP 409, Code "ConflictException", Message "Resource: <composition arn> is already STOPPED".
6. GetComposition(arn=<composition arn>) again: state must still be STOPPED and endTime unchanged (this part already holds).
