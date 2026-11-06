Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.cancel-legal-hold-already-canceled-rejected
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.cancel-legal-hold-already-canceled-rejected|run#14|AssertionError|assert message == (
Issue-Commit: 637c0a54c7e9b591effc4af895837fd7874d2d9c

# CancelLegalHold on an already-CANCELED hold refuses with the emulator's own sentence instead of AWS's state-interpolated message (and no Code/Context)

> _Note: this issue was identified against the code base at commit `637c0a54c7e9b591effc4af895837fd7874d2d9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelLegalHold

## Description
One net-new root cause in service `backup` (crate `services/service-com-amazonaws-backup-cryocontrollerusermanager`), reached from the behavioral suite rather than the wire corpus.

Related open issue, which does NOT cover this check: `20261103T171054Z-backup-aws-backup-legal-hold-cancel-and-recovery-point.md` cause 1 describes the SAME refusal builder as reached with a hold in `CREATING` (a wire fixture). The behavioral replay reports this check as owned by no open issue, and it exercises the already-released (`CANCELED`) state — the state value that must be interpolated into the message differs — so a fix that hardcodes one state leaves this path diverging. A fixer closing that issue should make the builder state-driven and re-replay this check.

Cause 1 of 1: the second CancelLegalHold of a hold that a previous cancellation already released is refused with the right exception class and status, but with the emulator's own fixed sentence and without the condition's `Code`/`Context`.
AWS: Smithy: `CancelLegalHold` (DELETE /legal-holds/{LegalHoldId}, success 201) models `InvalidResourceStateException` as a client error, HTTP 400 (defaulted); the shape's members `Code`, `Message`, `Type`, `Context` are all optional strings (smithy_query operation=CancelLegalHold; shape=InvalidResourceStateException on services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json). The recorded behavior for this condition is HTTP 400 `InvalidResourceStateException` with `Message: "Only ACTIVE legal holds can be canceled, the current state of provided legal hold is <status>."`, where `Only ACTIVE legal holds can be canceled, the current state of provided legal hold is ` and the trailing `.` are FIXED text and `<status>` is a value read from the emulator's state — the stored hold's current status, here `CANCELED` — the same value AWS also returns in `Context`; the condition's token is `Code: "ERROR_22003"`, constant. No id, ARN or timestamp from the request appears in the sentence.
Emulator: answers HTTP 400 with the correct class `InvalidResourceStateException` but the emulator-authored constant sentence `Legal hold cannot be canceled because it is not in the ACTIVE state`, and omits `Code` and `Context` entirely. The constant's own doc comment asserts "No recording of this condition is available, so the wording is this service's own" and "No `Code`: this condition has no observed token" — both are contradicted by the recorded behavior above.
Why: a client that branches on `Code == "ERROR_22003"`, or reads `Context` to learn WHICH state blocked the cancel, sees absent members and cannot distinguish "the hold is still being placed" from "the hold has already been released" — the two conditions that share this class. Any client or test asserting AWS's wording fails, and idempotent cleanup paths that swallow "already canceled" take the wrong branch.
Fix: the not-cancellable-state refusal builder in the service's state module (`legal_hold_not_active`, called from the CancelLegalHold handler) must set `Code` to the condition's token, set `Context` to the stored hold's current status, and format AWS's sentence with that status interpolated, instead of emitting a wording-only constant.
Traces: behavioral check backup.cancel-legal-hold-already-canceled-rejected. The surrounding assertions passed — the class/status are right, the first cancellation's `CancelDescription` and `Status` survive the refused second cancel — so only the error identity and wording diverge. Emulator source: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/state.rs:813-820 (`MESSAGE_LEGAL_HOLD_NOT_ACTIVE`), reached via `crate::state::legal_hold_not_active` from src/ops/cancel_legal_hold.rs.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 21/110 passed; service `backup` 13/47. Check backup.cancel-legal-hold-already-canceled-rejected — verdict mismatch:

  File "check_backup_cancel_legal_hold_already_canceled_rejected.py", line 67, in run
    assert message == (
  AssertionError: actual_message='Legal hold cannot be canceled because it is not in the ACTIVE state'

Expected message (AWS, with the hold's current status interpolated): 'Only ACTIVE legal holds can be canceled, the current state of provided legal hold is CANCELED.'
Expected error code/class: InvalidResourceStateException (HTTP 400) — matched by the emulator; the preceding assertion on the code passed.
Expected (and absent in the emulator's body): Code = 'ERROR_22003', Context = the hold's current status.
The check's earlier steps all passed: CreateLegalHold, the poll to ACTIVE, the positive-control cancel, and GetLegalHold reporting Status=CANCELED with CancelDescription='first cancellation'.

Model evidence: smithy_query(query="operation", operation="CancelLegalHold") — DELETE /legal-holds/{LegalHoldId}, success 201, errors InvalidParameterValueException / InvalidResourceStateException / MissingParameterValueException / ResourceNotFoundException (client, HTTP 400 defaulted) + ServiceUnavailableException (500); smithy_query(query="shape", shape="InvalidResourceStateException") — optional Code/Message/Type/Context string members, httpError 400.

Emulator source: src/state.rs:813-820 defines MESSAGE_LEGAL_HOLD_NOT_ACTIVE as the constant above with a doc comment claiming no recording and no token exist for this condition.

## Steps to Reproduce
1. Boot the backup emulator built from this checkout.
2. CreateBackupVault with a fresh name, then CreateLegalHold selecting that vault (RecoveryPointSelection.VaultNames=[vault], DateRange covering now) and poll GetLegalHold until Status=ACTIVE.
3. CancelLegalHold(LegalHoldId=<id>, CancelDescription='first cancellation') — succeeds; poll GetLegalHold until Status=CANCELED.
4. Call CancelLegalHold again on the same id with CancelDescription='second cancellation'. Observe HTTP 400 InvalidResourceStateException with NO Code, NO Context and Message 'Legal hold cannot be canceled because it is not in the ACTIVE state', where AWS answers Code 'ERROR_22003', Context set to the hold's current status ('CANCELED' here) and Message 'Only ACTIVE legal holds can be canceled, the current state of provided legal hold is CANCELED.' (fixed text + the stored status).
5. Re-replay behavioral check backup.cancel-legal-hold-already-canceled-rejected to confirm the fix, and re-replay the CREATING-state wire fixture backup-legal-hold-cancel-creating-state-error#task#2 to confirm the state is interpolated rather than hardcoded.
