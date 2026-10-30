Issue-Source: trace-sweep-divergence
Issue-Trace: backup-legal-hold-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-legal-hold-lifecycle backup-legal-hold-not-found-errors backup-copy-job-error-paths backup-copy-job-listing-and-describe-error
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.CreationDate
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.Description
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.LegalHoldArn
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.LegalHoldId
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.RecoveryPointSelection
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.Status
Issue-Key: backup|CreateLegalHold|2xx->4xx|$.Title
Issue-Key: backup|CreateLegalHold|2xx->4xx|status
Issue-Key: backup|ListLegalHolds|2xx->2xx|$.LegalHolds[*]
Issue-Key: backup|GetLegalHold|4xx->4xx|$.Code
Issue-Key: backup|GetLegalHold|4xx->4xx|$.Message
Issue-Key: backup|GetLegalHold|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|DescribeCopyJob|4xx->4xx|$.Code
Issue-Key: backup|DescribeCopyJob|4xx->4xx|$.Message
Issue-Commit: 65165f71b3b3c14984c314d4becd7f5d11cc460b

# AWS Backup legal-hold create refuses the wildcard vault selection, and legal-hold and copy-job not-found refusals are misclassified (4 causes)

> _Note: this issue was identified against the code base at commit `65165f71b3b3c14984c314d4becd7f5d11cc460b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLegalHold

## Description
Four net-new wire divergences in this crate's legal-hold and copy-job paths. All four are in service `backup` (crate `services/service-com-amazonaws-backup-cryocontrollerusermanager`). Causes 1 and 2 share one root (a create AWS accepts is refused, so the hold never exists to be listed); causes 3 and 4 are each a not-found refusal whose classification, `Code` member and message are the emulator's own rather than AWS's. A fixer must close ALL FOUR before submitting.

Note: an older open issue `20261029T175130Z-backup-createlegalhold-stub.md` describes `CreateLegalHold` as a 501 scaffold stub. That no longer holds — the operation is implemented and now answers 400 — so that issue does not cover cause 1.

Cause 1 of 4: CreateLegalHold refuses a RecoveryPointSelection whose VaultNames is the wildcard `*`.
AWS: Smithy: `CreateLegalHold` input member `RecoveryPointSelection` -> `RecoveryPointSelection`, whose `VaultNames` targets `com.amazonaws.backup#VaultNames`, a list of plain `string` with NO `@pattern` and no `@length` (unlike `BackupVaultName`, which carries `^[a-zA-Z0-9\-\_]{2,50}$`). The model therefore places no name-shape constraint on a selection's vault names, and the recording shows AWS accepting `VaultNames: ["*"]` — the all-vaults wildcard — with HTTP 200 and echoing the selection back (`RecoveryPointSelection.VaultNames = ["*"]`, `DateRange.FromDate`/`ToDate` as sent), plus `Status: "CREATING"`, the request's `Title` and `Description`, and a minted `LegalHoldId`/`LegalHoldArn`/`CreationDate`.
Emulator: answers HTTP 400 `InvalidParameterValueException` with the emulator-authored message `RecoveryPointSelection VaultNames entry <name> is not a backup vault name` and persists nothing. `crate::state::rejected_recovery_point_selection` validates every `VaultNames` entry with `is_modelled_vault_name` — the `BackupVaultName` pattern (2..=50 chars, alphanumeric/`-`/`_`) — which rejects `*` both on length and on the character class.
Why: the wildcard is how a client places a hold across all vaults, which is the documented and common form of the call. Every client that creates a legal hold that way gets a 400 for a valid request, no hold is stored, and the whole legal-hold workflow (list, get, cancel) is unreachable behind it.
Fix: the validation of the selection shared by the legal-hold path — `rejected_recovery_point_selection` in the service's state module must not apply the `BackupVaultName` pattern to `VaultNames`, which the model leaves unconstrained (at minimum the wildcard `*` must be accepted); `CreateLegalHold`'s handler then stores and echoes the selection as sent.
Traces: backup-legal-hold-lifecycle#task#1.

Cause 2 of 4: ListLegalHolds reports no legal hold at all.
AWS: recorded behavior only — after the create above, `ListLegalHolds` returns HTTP 200 with one entry in `LegalHolds` carrying that hold's `LegalHoldId`, `LegalHoldArn`, `Title`, `Description`, `Status: "ACTIVE"` and `CancellationDate` absent/null.
Emulator: answers HTTP 200 with `LegalHolds` empty (`$.LegalHolds[0]` missing), because the create in cause 1 was refused and nothing was inserted into the legal-hold store.
Why: a client that creates a hold and then enumerates holds cannot see it, so nothing downstream (get, cancel, the delete guard a hold places on a recovery point) can ever be exercised.
Fix: downstream of cause 1 — once the create stores the hold, `ListLegalHolds` must project it. This step is listed as its own cause so the fixer re-replays it: it is the only step that proves the stored hold becomes visible with `Status: ACTIVE` after the settle window.
Traces: backup-legal-hold-lifecycle#task#4.

Cause 3 of 4: an unknown legal hold is refused as not-found instead of as an invalid parameter value, with no `Code`.
AWS: recorded behavior only for the token and sentence (the model's `InvalidParameterValueException` and `ResourceNotFoundException` both declare optional `Code`, `Message`, `Type`, `Context` and both default to HTTP 400). `GetLegalHold` for a legal-hold id this account does not hold answers HTTP 400 with `x-amzn-errortype: InvalidParameterValueException`, `Code: "ERROR_4218"` and `Message: "Can not find Legal Hold ARN."` — that sentence is FIXED text: it names no part of the request, so no id, name or ARN is interpolated into it.
Emulator: answers HTTP 400 with `x-amzn-errortype: ResourceNotFoundException`, no `Code` member at all, and the emulator-authored message `Cannot find legal hold` (`crate::state::MESSAGE_LEGAL_HOLD_NOT_FOUND` / `legal_hold_not_found`).
Why: the error class is part of the contract — a client that catches `InvalidParameterValueException` (or switches on `Code == "ERROR_4218"`) sees an unhandled `ResourceNotFoundException` instead, so retry/branching logic around a missing hold takes the wrong path, and any logging keyed on `Code` sees an absent member.
Fix: the legal-hold not-found refusal in the service's state module (`legal_hold_not_found`, used by `GetLegalHold`'s handler) must build the modeled `InvalidParameterValueException` with `Code` `ERROR_4218` and AWS's fixed sentence, not a `ResourceNotFoundException`.
Traces: backup-legal-hold-not-found-errors#task#1.

Cause 4 of 4: the copy-job not-found refusal omits `Code` and uses the emulator's own sentence.
AWS: recorded behavior only for the token and sentence (model: `ResourceNotFoundException`, HTTP 400, optional `Code`/`Message`/`Type`/`Context`). `DescribeCopyJob` for a copy-job id this account does not hold answers HTTP 400 `ResourceNotFoundException` with `Code: "ERROR_21001"` and `Message: "Copy job not found"` — again FIXED text that interpolates no part of the request.
Emulator: right status and right error class, but no `Code` member and the message `Cannot find copy job` (`crate::state::MESSAGE_COPY_JOB_NOT_FOUND` / `copy_job_not_found`, whose doc comment states outright that no `Code` is set because the condition "has no observed token" — the corpus observes one).
Why: a client or test that asserts on AWS's wording or dispatches on `Code` cannot match the response; the word order differs from AWS's (`Copy job not found` vs `Cannot find copy job`) and the `Code` a caller reads for this condition is simply missing.
Fix: the copy-job not-found refusal in the service's state module (`copy_job_not_found`, used by `DescribeCopyJob`'s handler) must set `Code` to `ERROR_21001` and the message to AWS's fixed sentence.
Traces: backup-copy-job-error-paths#task#1 (cluster also covers backup-copy-job-listing-and-describe-error#task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (84 fixtures, 481 steps, score 36.2%). The four clusters below are the net-new ones: none carries a `stub_path` and none is `owned_by` an open issue.

Cause 1 — fixture backup-legal-hold-lifecycle, step task#1, CreateLegalHold:
  status: expected 200, got 400
  $.CreationDate: missing (expected <shaped-not-asserted>)
  $.Description: missing (expected 'Probe-created legal hold for audit workflow')
  $.LegalHoldArn: missing; $.LegalHoldId: missing
  $.RecoveryPointSelection: missing (expected {'DateRange': {'FromDate': <shaped>, 'ToDate': <shaped>}, 'ResourceIdentifiers': None, 'VaultNames': ['*']})
  $.Status: missing (expected 'CREATING'); $.Title: missing (expected the request's Title)
  Emulator source: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_legal_hold.rs:195-199 calls crate::state::rejected_recovery_point_selection; src/state.rs:941-954 rejects any VaultNames entry failing is_modelled_vault_name (src/state.rs:449-454, the BackupVaultName pattern ^[a-zA-Z0-9\-\_]{2,50}$), which '*' cannot satisfy.

Cause 2 — fixture backup-legal-hold-lifecycle, step task#4, ListLegalHolds (status 200 both sides):
  $.LegalHolds[0]: missing (expected {'CancellationDate': None, 'Description': 'Probe-created legal hold for audit workflow', 'LegalHoldArn': <the created hold's ARN>, 'LegalHoldId': <the created hold's id>, 'Status': 'ACTIVE', 'Title': <the request's Title>}, got None)
  Emulator source: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/list_legal_holds.rs projects state.legal_holds, which cause 1 left empty.

Cause 3 — fixture backup-legal-hold-not-found-errors, step task#1, GetLegalHold (status 400 both sides):
  $.Code: missing (expected 'ERROR_4218', got None)
  $.Message: value_mismatch (expected 'Can not find Legal Hold ARN.', got 'Cannot find legal hold')
  $headers.x-amzn-errortype: value_mismatch (expected 'InvalidParameterValueException', got 'ResourceNotFoundException')
  Emulator source: src/state.rs:620-625 (MESSAGE_LEGAL_HOLD_NOT_FOUND) and src/state.rs:689-695 (legal_hold_not_found -> ResourceNotFoundException), reached via src/state.rs:1750-1753 from src/ops/get_legal_hold.rs.

Cause 4 — fixture backup-copy-job-error-paths, step task#1, DescribeCopyJob (status 400 both sides; cluster member backup-copy-job-listing-and-describe-error#task#4):
  $.Code: missing (expected 'ERROR_21001', got None)
  $.Message: value_mismatch (expected 'Copy job not found', got 'Cannot find copy job')
  Emulator source: src/state.rs:552-557 (MESSAGE_COPY_JOB_NOT_FOUND) and src/state.rs:660-666 (copy_job_not_found), reached from src/ops/describe_copy_job.rs.

Model evidence cited: smithy_query(query="shape", shape="RecoveryPointSelection,VaultNames,InvalidParameterValueException,ResourceNotFoundException") on services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json — VaultNames is a list of unconstrained `string`; both exceptions are client errors defaulting to HTTP 400 with optional Code/Message/Type/Context members.

## Steps to Reproduce
1. Boot the backup emulator from this checkout.
2. Cause 1: call CreateLegalHold with Title and Description set and RecoveryPointSelection = {VaultNames: ["*"], DateRange: {FromDate: <a past instant>, ToDate: <now-or-past>}}. Observe HTTP 400 InvalidParameterValueException 'RecoveryPointSelection VaultNames entry * is not a backup vault name' where AWS answers HTTP 200 with Status 'CREATING' and the selection echoed back. Replay fixture backup-legal-hold-lifecycle step task#1.
3. Cause 2: after that create, call ListLegalHolds and observe HTTP 200 with an empty LegalHolds list where AWS reports the hold with Status 'ACTIVE'. Replay fixture backup-legal-hold-lifecycle step task#4.
4. Cause 3: call GetLegalHold with a legal-hold id no hold of the account answers to. Observe HTTP 400, x-amzn-errortype ResourceNotFoundException, no Code, Message 'Cannot find legal hold' — AWS answers x-amzn-errortype InvalidParameterValueException, Code 'ERROR_4218', Message 'Can not find Legal Hold ARN.'. Replay fixture backup-legal-hold-not-found-errors step task#1.
5. Cause 4: call DescribeCopyJob with a copy-job id the account does not hold. Observe HTTP 400 ResourceNotFoundException with no Code and Message 'Cannot find copy job' — AWS answers Code 'ERROR_21001', Message 'Copy job not found'. Replay fixture backup-copy-job-error-paths step task#1 (and backup-copy-job-listing-and-describe-error step task#4).
6. Re-replay all four fixture steps to confirm the fix.
