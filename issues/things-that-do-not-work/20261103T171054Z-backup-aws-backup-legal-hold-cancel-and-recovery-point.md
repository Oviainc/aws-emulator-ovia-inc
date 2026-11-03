Issue-Source: trace-sweep-divergence
Issue-Trace: backup-legal-hold-cancel-creating-state-error
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-legal-hold-cancel-creating-state-error backup-legal-hold-not-found-errors
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CancelLegalHold|4xx->4xx|$.Code
Issue-Key: backup|CancelLegalHold|4xx->4xx|$.Context
Issue-Key: backup|CancelLegalHold|4xx->4xx|$.Message
Issue-Key: backup|CancelLegalHold|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|ListRecoveryPointsByLegalHold|4xx->4xx|$.Code
Issue-Key: backup|ListRecoveryPointsByLegalHold|4xx->4xx|$.Context
Issue-Key: backup|ListRecoveryPointsByLegalHold|4xx->4xx|$.Message
Issue-Commit: 4448e4a5a9b1561678308c7c69716e4ea37b45bd

# AWS Backup legal-hold cancel and recovery-point listing refusals carry the emulator's own class, Code and wording (3 causes)

> _Note: this issue was identified against the code base at commit `4448e4a5a9b1561678308c7c69716e4ea37b45bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelLegalHold

## Description
Three net-new wire divergences in the legal-hold refusal paths of service `backup` (crate `services/service-com-amazonaws-backup-cryocontrollerusermanager`). They share ONE root cause shape: the emulator answers a legal-hold refusal with its OWN exception class, its OWN `Code`/`Context` members and its OWN sentence, where AWS answers with a condition-specific `ERROR_nnnn` token and fixed wording. The status code is right in all three; the error IDENTITY is wrong. A fixer must close ALL THREE before submitting.

Related open issue (does NOT cover these steps): `20261030T140839Z-backup-aws-backup-legal-hold-create-refuses-the-wildcard.md` cause 3 describes the SAME unknown-legal-hold condition as answered by `GetLegalHold`. The clusters below are the same condition reached through `CancelLegalHold` and `ListRecoveryPointsByLegalHold`, plus the not-cancellable-state condition; the replay reports them as owned by no open issue, so fixing only `GetLegalHold` leaves them diverging.

Cause 1 of 3: CancelLegalHold on a hold that is not yet in force refuses with the emulator's own sentence and omits `Code` and `Context`.
AWS: Smithy: `CancelLegalHold` (DELETE /legal-holds/{LegalHoldId}, success 201) models `InvalidResourceStateException` (client, HTTP 400) whose members `Code`, `Message`, `Type`, `Context` are all optional strings. The recording shows AWS refusing a cancel of a hold still in `CREATING` with HTTP 400 `InvalidResourceStateException`, `Code: "ERROR_22003"`, `Context` carrying the hold's CURRENT status, and `Message: "Only ACTIVE legal holds can be canceled, the current state of provided legal hold is <status>."`. In that sentence `Only ACTIVE legal holds can be canceled, the current state of provided legal hold is ` and the trailing `.` are FIXED text; `<status>` is a value taken from the emulator's state — the stored hold's status (`CREATING` in the recording) — and it is the same value that must appear in `Context`. `ERROR_22003` is the condition's token and is constant.
Emulator: answers HTTP 400 with the right class but NO `Code` member, NO `Context` member, and the emulator-authored sentence `Legal hold cannot be canceled because it is not in the ACTIVE state` (`crate::state::MESSAGE_LEGAL_HOLD_NOT_ACTIVE`, built by `crate::state::legal_hold_not_active`). The source comment at that constant asserts "No recording of this condition is available, so the wording is this service's own" and "No `Code`: this condition has no observed token" — the corpus observes both.
Why: a client that branches on `Code == "ERROR_22003"`, or that reads `Context` to learn which state blocked the cancel, sees an absent member and has no way to distinguish "still being placed" from "already released". Any assertion on AWS's wording fails.
Fix: this condition's refusal builder in the service's state module (`legal_hold_not_active`, called from `CancelLegalHold`'s handler) must set `Code` to the condition's token, set `Context` to the stored hold's current status, and format AWS's sentence with that status interpolated instead of emitting a constant of its own.
Traces: backup-legal-hold-cancel-creating-state-error#task#2.

Cause 2 of 3: CancelLegalHold for an unknown legal-hold id is misclassified as not-found instead of as an invalid parameter value, with no `Code`.
AWS: Smithy: `CancelLegalHold` models BOTH `InvalidParameterValueException` and `ResourceNotFoundException` as client errors defaulting to HTTP 400, each with optional `Code`/`Message`/`Type`/`Context`. The recording shows AWS refusing a cancel of a legal-hold id the account does not hold with `x-amzn-errortype: InvalidParameterValueException`, `Code: "ERROR_4218"` and `Message: "Can not find Legal Hold ARN."` — entirely FIXED text (note AWS's spelling `Can not`), interpolating no id, name or ARN from the request. AWS Backup treats an id that resolves to nothing as a value of the member that carried it, not as a missing resource.
Emulator: answers HTTP 400 with `x-amzn-errortype: ResourceNotFoundException`, no `Code` member at all, and the emulator-authored sentence `Cannot find legal hold` (`crate::state::MESSAGE_LEGAL_HOLD_NOT_FOUND`, built by `crate::state::legal_hold_not_found`, selected by `CancelLegalHold`'s handler).
Why: the exception class is part of the contract. A client whose catch/`except` arm handles `InvalidParameterValueException` (or dispatches on `Code == "ERROR_4218"`) sees an unhandled `ResourceNotFoundException`, so cleanup and retry logic around "this hold is already gone" takes the wrong branch and the request fails hard instead of being handled.
Fix: the legal-hold not-found refusal in the service's state module must build the modeled `InvalidParameterValueException` with the condition's `Code` and AWS's fixed sentence, and `CancelLegalHold`'s handler must select that class rather than `ResourceNotFoundException`. The same builder serves `GetLegalHold`, so one correction covers both read paths.
Traces: backup-legal-hold-not-found-errors#task#2.

Cause 3 of 3: ListRecoveryPointsByLegalHold answers an unknown legal-hold id with the generic rejected-parameter template instead of this condition's own token and sentence.
AWS: Smithy: `ListRecoveryPointsByLegalHold` (GET /legal-holds/{LegalHoldId}/recovery-points) models only `InvalidParameterValueException` and `MissingParameterValueException` as client errors (both HTTP 400) — so the class the emulator chooses is right. The recording shows AWS refusing an id the account does not hold with the SAME identity it uses everywhere else for this condition: `Code: "ERROR_4218"`, `Message: "Can not find Legal Hold ARN."` (fixed text, nothing interpolated) and NO `Context` member. The condition is "this legal hold does not exist", and AWS words it once regardless of which operation reached it.
Emulator: answers HTTP 400 `InvalidParameterValueException` with the class-wide generic refusal instead: `Code: "INVALID_PARAMETER_VALUE"` (`crate::state::ERROR_CODE_INVALID_PARAMETER_VALUE`), `Context: "LegalHoldId"` and `Message: "Parameter LegalHoldId is invalid."`, produced by `crate::state::rejected_parameter("LegalHoldId")` from this operation's handler.
Why: a client gets a different `Code` and a different sentence for the same condition depending on which legal-hold operation it called, and the `Context` member is populated where AWS sends none — so code keyed on `ERROR_4218` never matches, and code that treats a populated `Context` as "a malformed input value, do not retry" misreads a plain missing-resource condition as a request-shape bug.
Fix: `ListRecoveryPointsByLegalHold`'s handler must use the shared legal-hold not-found refusal builder from the service's state module (the one corrected in cause 2, kept as this service's single wording for the condition) rather than the generic `rejected_parameter` member-rejection builder, while keeping the modeled `InvalidParameterValueException` class this operation declares.
Traces: backup-legal-hold-not-found-errors#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 92 fixtures replayed, 571 steps, 238 passed, score 41.7%, 48 divergence clusters. 45 clusters are `owned_by` an existing open issue and none carries a `stub_path`; the three clusters below are the only net-new ones, all with `service: "backup"` (bootstrapped).

Cause 1 — fixture backup-legal-hold-cancel-creating-state-error, step task#2, CancelLegalHold (status 400 on both sides):
  $.Code: missing (expected 'ERROR_22003', got None)
  $.Context: missing (expected 'CREATING', got None)
  $.Message: value_mismatch (expected 'Only ACTIVE legal holds can be canceled, the current state of provided legal hold is CREATING.', got 'Legal hold cannot be canceled because it is not in the ACTIVE state')
  divergence_key: service=backup|CancelLegalHold|status=400->400|keys=$.Code,$.Context,$.Message
  Emulator source: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/state.rs:813-820 (MESSAGE_LEGAL_HOLD_NOT_ACTIVE, whose doc comment claims no recording and no observed token for this condition), reached via crate::state::legal_hold_not_active from src/ops/cancel_legal_hold.rs:181-186.

Cause 2 — fixture backup-legal-hold-not-found-errors, step task#2, CancelLegalHold (status 400 on both sides):
  $.Code: missing (expected 'ERROR_4218', got None)
  $.Message: value_mismatch (expected 'Can not find Legal Hold ARN.', got 'Cannot find legal hold')
  $headers.x-amzn-errortype: value_mismatch (expected 'InvalidParameterValueException', got 'ResourceNotFoundException')
  divergence_key: service=backup|CancelLegalHold|status=400->400|keys=$.Code,$.Message,$headers.x-amzn-errortype
  Emulator source: src/state.rs:806-811 (MESSAGE_LEGAL_HOLD_NOT_FOUND, "No `Code`: this condition has no observed token") and crate::state::legal_hold_not_found, selected as ResourceNotFoundException at src/ops/cancel_legal_hold.rs:172-180.

Cause 3 — fixture backup-legal-hold-not-found-errors, step task#3, ListRecoveryPointsByLegalHold (status 400 on both sides):
  $.Code: value_mismatch (expected 'ERROR_4218', got 'INVALID_PARAMETER_VALUE')
  $.Context: type_mismatch (expected None, got 'LegalHoldId')
  $.Message: value_mismatch (expected 'Can not find Legal Hold ARN.', got 'Parameter LegalHoldId is invalid.')
  divergence_key: service=backup|ListRecoveryPointsByLegalHold|status=400->400|keys=$.Code,$.Context,$.Message
  Emulator source: src/ops/list_recovery_points_by_legal_hold.rs:184-191 calls crate::state::rejected_parameter("LegalHoldId"); that builder is src/state.rs:879-886 (Code INVALID_PARAMETER_VALUE = src/state.rs:668, Message format!("Parameter {member} is invalid."), Context = member).

Model evidence cited: smithy_query(query="operation", operation="CancelLegalHold,ListRecoveryPointsByLegalHold") and smithy_query(query="shape", shape="InvalidParameterValueException,InvalidResourceStateException,ResourceNotFoundException") on services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json. CancelLegalHold: DELETE /legal-holds/{LegalHoldId}, success 201, errors InvalidParameterValueException / InvalidResourceStateException / MissingParameterValueException / ResourceNotFoundException (all client, HTTP 400 defaulted) + ServiceUnavailableException (500). ListRecoveryPointsByLegalHold: GET /legal-holds/{LegalHoldId}/recovery-points, success 200, errors InvalidParameterValueException / MissingParameterValueException (400) + ServiceUnavailableException (500) — it models NO ResourceNotFoundException. All three exception shapes carry optional Code/Message/Type/Context string members and default to HTTP 400.

## Steps to Reproduce
1. Boot the backup emulator built from this checkout.
2. Cause 1: create a legal hold and, before it settles out of `CREATING`, call CancelLegalHold on it with a CancelDescription. Observe HTTP 400 InvalidResourceStateException with no Code, no Context and Message 'Legal hold cannot be canceled because it is not in the ACTIVE state', where AWS answers Code 'ERROR_22003', Context set to the hold's current status and Message 'Only ACTIVE legal holds can be canceled, the current state of provided legal hold is <status>.'. Replay fixture backup-legal-hold-cancel-creating-state-error step task#2.
3. Cause 2: call CancelLegalHold with a LegalHoldId no hold of the account answers to. Observe HTTP 400 with x-amzn-errortype ResourceNotFoundException, no Code and Message 'Cannot find legal hold', where AWS answers x-amzn-errortype InvalidParameterValueException, Code 'ERROR_4218' and Message 'Can not find Legal Hold ARN.'. Replay fixture backup-legal-hold-not-found-errors step task#2.
4. Cause 3: call ListRecoveryPointsByLegalHold with that same unknown LegalHoldId. Observe HTTP 400 InvalidParameterValueException with Code 'INVALID_PARAMETER_VALUE', Context 'LegalHoldId' and Message 'Parameter LegalHoldId is invalid.', where AWS answers Code 'ERROR_4218', no Context and Message 'Can not find Legal Hold ARN.'. Replay fixture backup-legal-hold-not-found-errors step task#3.
5. Re-replay all three fixture steps (backup-legal-hold-cancel-creating-state-error#task#2, backup-legal-hold-not-found-errors#task#2, backup-legal-hold-not-found-errors#task#3) to confirm the fix, and re-replay backup-legal-hold-not-found-errors#task#1 (GetLegalHold) to confirm the shared not-found wording did not regress.
