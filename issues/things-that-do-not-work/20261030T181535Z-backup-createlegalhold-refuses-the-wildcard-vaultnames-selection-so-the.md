Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.legal-hold-lifecycle
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CreateLegalHold|InvalidParameterValueException
Issue-Commit: f7507ccd8e733df5ba5b0e6ba5890532707d636f

# CreateLegalHold refuses the wildcard VaultNames selection, so the whole legal-hold lifecycle (get/list/cancel) is unreachable

> _Note: this issue was identified against the code base at commit `f7507ccd8e733df5ba5b0e6ba5890532707d636f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLegalHold

## Description
One root cause, grounded in behavioral check `backup.legal-hold-lifecycle`.

Note on prior art: open issue `20261030T140839Z-backup-aws-backup-legal-hold-create-refuses-the-wildcard.md` describes the same emulator defect from the WIRE corpus (its cause 1, fixture `backup-legal-hold-lifecycle#task#1`). That issue does not carry this behavioral check's replay key, so this issue is filed to bind the behavioral check to it; a fixer closing either must re-replay `backup.legal-hold-lifecycle`. Also note the older open issue `20261029T175130Z-backup-createlegalhold-stub.md` calling CreateLegalHold a 501 scaffold stub no longer holds: the operation is implemented and answers 400.

Cause 1 of 1: CreateLegalHold rejects a RecoveryPointSelection whose VaultNames contains the all-vaults wildcard `*`.

AWS: Smithy (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): `CreateLegalHold` input member `RecoveryPointSelection` -> `com.amazonaws.backup#RecoveryPointSelection`, whose member `VaultNames` targets `com.amazonaws.backup#VaultNames`, a list of plain `string` with NO `@pattern` and NO `@length` — unlike `BackupVaultName`, which carries `^[a-zA-Z0-9\-\_]{2,50}$`. The model therefore places no backup-vault-name shape constraint on a selection's vault names, and the recorded behavior shows AWS accepting `VaultNames: ["*"]` with HTTP 200, echoing the selection back as sent (`VaultNames`, `DateRange.FromDate`/`ToDate`), plus `Status` in the creating/active progression, the request's `Title`/`Description` and a minted `LegalHoldId`/`LegalHoldArn`/`CreationDate`. The hold then settles to `Status: ACTIVE`, is enumerated by `ListLegalHolds`, and after `CancelLegalHold` it remains listed with `Status: CANCELED` and the request's `CancelDescription`.

Emulator: answers HTTP 400 `InvalidParameterValueException` (modeled: client error, httpError 400, optional `Code`/`Message`/`Type`/`Context`) with the emulator-authored message `RecoveryPointSelection VaultNames entry * is not a backup vault name` and persists nothing. In that message the fixed text is `RecoveryPointSelection VaultNames entry ` + ` is not a backup vault name`; the only variable part is the offending entry taken from the request (here `*`). The refusal itself is the defect — AWS does not refuse this request at all, so no message is correct for it.

Why: the wildcard is how a client places a legal hold across all vaults, the common documented form of the call. Every such client gets a 400 for a valid request; because no hold is stored, the entire downstream workflow is unreachable: `GetLegalHold` never reports `Status: ACTIVE`, `ListLegalHolds` reports the hold nowhere, `CancelLegalHold` has nothing to cancel, and the CANCELED-but-still-listed contract ("active and previous legal holds") can never be observed.

Fix: the shared validation of the recovery-point selection in the service's state module (`rejected_recovery_point_selection`, called from the CreateLegalHold handler) must not apply the `BackupVaultName` pattern to `VaultNames`, which the model leaves unconstrained — at minimum the wildcard `*` must be accepted; the handler must then store the hold and echo the selection as sent, so the legal-hold store feeds Get/List/Cancel.

Traces: behavioral check `backup.legal-hold-lifecycle` (also recorded in the wire corpus as `backup-legal-hold-lifecycle#task#1`).

## Evidence
Behavioral suite replay against the emulator built from this checkout (12/97 passed).

[error] backup.legal-hold-lifecycle:
  botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValueException) when calling the CreateLegalHold operation: RecoveryPointSelection VaultNames entry * is not a backup vault name

The check calls:
  create_legal_hold(Title=<unique>, Description=..., RecoveryPointSelection={"VaultNames": ["*"], "DateRange": {"FromDate": now-3600, "ToDate": now}}, Tags={...})
expecting HTTP 200 and a LegalHoldId, then polls GetLegalHold for Status ACTIVE, asserts exactly one ListLegalHolds entry with Status ACTIVE, cancels it and asserts the entry remains listed with Status CANCELED and the sent CancelDescription. The call never gets past the create: the emulator answers 400.

Expected (AWS): 200, Status in the creating/active progression, RecoveryPointSelection echoed with VaultNames ['*'].
Actual (emulator): 400 InvalidParameterValueException, message 'RecoveryPointSelection VaultNames entry * is not a backup vault name', nothing persisted.

Model evidence: smithy_query(query="shape", shape="RecoveryPointSelection,VaultNames,InvalidParameterValueException") on services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json — VaultNames is a list of unconstrained `string`; InvalidParameterValueException is a client error defaulting to HTTP 400.

## Steps to Reproduce
1. Boot the backup emulator from this checkout.
2. Call CreateLegalHold with a Title and Description and RecoveryPointSelection = {VaultNames: ["*"], DateRange: {FromDate: <a past instant>, ToDate: <now>}}.
3. Observe HTTP 400 InvalidParameterValueException with message 'RecoveryPointSelection VaultNames entry * is not a backup vault name', where AWS answers HTTP 200 and echoes the selection back with a minted LegalHoldId/LegalHoldArn.
4. Re-replay behavioral check backup.legal-hold-lifecycle to confirm the fix: after the create succeeds, GetLegalHold must settle to Status ACTIVE, ListLegalHolds must report exactly one entry for the hold, and after CancelLegalHold the entry must remain listed with Status CANCELED and the sent CancelDescription.
