Issue-Source: trace-sweep-divergence
Issue-Trace: verifiedpermissions-policy-store-lifecycle-and-tags
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: verifiedpermissions-policy-store-lifecycle-and-tags verifiedpermissions-policy-store-deletion-protection
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions|GetPolicyStore|2xx->2xx|$.cedarVersion
Issue-Key: verifiedpermissions|GetPolicyStore|2xx->2xx|$.createdDate
Issue-Key: verifiedpermissions|GetPolicyStore|2xx->2xx|$.encryptionState
Issue-Key: verifiedpermissions|GetPolicyStore|2xx->2xx|$.lastUpdatedDate
Issue-Key: verifiedpermissions|GetPolicyStore|2xx->2xx|$.validationSettings.mode
Issue-Key: verifiedpermissions|GetPolicyStore|2xx->2xx|$.deletionProtection
Issue-Key: verifiedpermissions|DeletePolicyStore|2xx->4xx|status
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# Verified Permissions GetPolicyStore returns stale state after UpdatePolicyStore and omits encryptionState/cedarVersion

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicyStore

## Description
The emulator's policy-store state is both under-populated (two modeled GetPolicyStore members are never emitted) and not mutated by UpdatePolicyStore, so a store's validationSettings and deletionProtection keep their creation-time values forever and the store can never be deleted once protection was enabled. This bundle has 4 causes.

Cause 1 of 4: GetPolicyStore omits the modeled encryptionState and cedarVersion members.
AWS: Smithy GetPolicyStore output declares encryptionState (union com.amazonaws.verifiedpermissions#EncryptionState, whose "default" variant means "encrypted with an AWS owned key") and cedarVersion (enum CedarVersion, values CEDAR_2 | CEDAR_4). Recorded AWS responses for a plainly-created store carry encryptionState = {"default": {}} and cedarVersion = "CEDAR_4". createdDate/lastUpdatedDate are also required date-time timestamps (see the separate timestamp-encoding issue).
Emulator: the GetPolicyStore 200 body has no encryptionState and no cedarVersion key at all, and renders createdDate/lastUpdatedDate as epoch numbers (1792644261.1474373) instead of ISO-8601 strings.
Why: a client that reads cedarVersion to pick the Cedar dialect it compiles policies against, or reads encryptionState to assert the store is not customer-managed-key encrypted, gets None and cannot distinguish "AWS owned key / Cedar 4" from "unknown".
Fix: the policy-store record in the verifiedpermissions crate's state must carry an encryption state and a Cedar version, defaulted at CreatePolicyStore, and the GetPolicyStore response builder (services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/get_policy_store.rs) must serialize both.
Traces: verifiedpermissions-policy-store-lifecycle-and-tags#task#3.

Cause 2 of 4: UpdatePolicyStore's validationSettings change is not persisted.
AWS: UpdatePolicyStore takes a required validationSettings and replaces the store's setting; a subsequent GetPolicyStore reports the new mode and a bumped lastUpdatedDate (the recording shows createdDate unchanged at 2026-09-06T06:27:19.14526Z while lastUpdatedDate advances to 2026-09-06T06:27:19.294386Z).
Emulator: after an UpdatePolicyStore that sets mode STRICT, GetPolicyStore still reports $.validationSettings.mode = "OFF", and lastUpdatedDate is unchanged from createdDate (both the same epoch number), i.e. the update mutated nothing observable.
Why: strict validation is what makes CreatePolicy reject schema-violating policies, so a client that enables STRICT and then relies on validation silently keeps an unvalidated store; and lastUpdatedDate never advancing breaks any change-detection or optimistic-refresh logic.
Fix: UpdatePolicyStore's handler (src/ops/update_policy_store.rs) must write validationSettings back into the stored policy store and set lastUpdatedDate to the mutation time.
Traces: verifiedpermissions-policy-store-lifecycle-and-tags#task#9.

Cause 3 of 4: UpdatePolicyStore's deletionProtection change is not persisted.
AWS: UpdatePolicyStore's optional deletionProtection member (enum DeletionProtection) replaces the store's setting; GetPolicyStore afterwards reports DISABLED.
Emulator: after an UpdatePolicyStore setting deletionProtection=DISABLED, GetPolicyStore still reports $.deletionProtection = "ENABLED" (and again omits encryptionState/cedarVersion and emits epoch-number timestamps).
Why: the client has no way to observe that it successfully turned protection off, and — see cause 4 — the store is permanently undeletable.
Fix: same handler as cause 2; the deletionProtection field must be written back to state.
Traces: verifiedpermissions-policy-store-deletion-protection#task#4.

Cause 4 of 4: DeletePolicyStore keeps refusing with 406 after deletion protection was disabled.
AWS: Smithy DeletePolicyStore returns an empty 200 body on success and models InvalidStateException (httpError 406) only for the deletion-protection refusal. Once deletionProtection is DISABLED the delete succeeds; the recorded teardown step is a 200.
Emulator: returns status 406 where 200 is expected — it re-reads the stale ENABLED flag (cause 3) and refuses the delete, so the store cannot be torn down at all.
Why: this is a resource leak and a hard blocker: any client workflow that enables deletion protection, later disables it, and then deletes the store fails permanently against the emulator, and cleanup code that treats 406 as fatal aborts.
Fix: DeletePolicyStore's refusal predicate (src/ops/delete_policy_store.rs) must consult the current, post-update deletionProtection value; the underlying defect is the dropped write in cause 3.
Traces: verifiedpermissions-policy-store-deletion-protection#teardown#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator. state_reset_failures = 0, so each fixture ran against a clean store and these are genuine within-fixture state losses.

Cause 1 — fixture verifiedpermissions-policy-store-lifecycle-and-tags, step task#3, GetPolicyStore:
  expected status 200, actual status 200
  $.encryptionState: missing (expected {'default': {}}, got None)
  $.cedarVersion: missing (expected 'CEDAR_4', got None)
  $.createdDate: value_mismatch (expected '2026-09-06T06:27:19.14526Z', got 1792644261.1474373)
  $.lastUpdatedDate: value_mismatch (expected '2026-09-06T06:27:19.14526Z', got 1792644261.1474373)

Cause 2 — fixture verifiedpermissions-policy-store-lifecycle-and-tags, step task#9, GetPolicyStore (after UpdatePolicyStore -> STRICT):
  expected status 200, actual status 200
  $.validationSettings.mode: value_mismatch (expected 'STRICT', got 'OFF')
  $.lastUpdatedDate: value_mismatch (expected '2026-09-06T06:27:19.294386Z', got 1792644261.1474373 — same value as createdDate, never bumped)
  $.encryptionState: missing (expected {'default': {}}, got None); $.cedarVersion: missing (expected 'CEDAR_4', got None)

Cause 3 — fixture verifiedpermissions-policy-store-deletion-protection, step task#4, GetPolicyStore (after UpdatePolicyStore -> deletionProtection DISABLED):
  expected status 200, actual status 200
  $.deletionProtection: value_mismatch (expected 'DISABLED', got 'ENABLED')
  $.createdDate: value_mismatch (expected '2026-09-06T06:03:16.634756Z', got 1792644260.4944196)
  $.lastUpdatedDate: value_mismatch (expected '2026-09-06T06:03:16.682586Z', got 1792644260.4944196)
  $.encryptionState: missing (expected {'default': {}}, got None); $.cedarVersion: missing (expected 'CEDAR_4', got None)

Cause 4 — fixture verifiedpermissions-policy-store-deletion-protection, step teardown#5, DeletePolicyStore:
  status: expected 200, got 406

Model evidence: GetPolicyStoreOutput members encryptionState (union EncryptionState with variants kmsEncryptionState | default) and cedarVersion (enum CedarVersion: CEDAR_2, CEDAR_4); UpdatePolicyStoreInput members validationSettings (required) and deletionProtection; DeletePolicyStore output is an empty structure with InvalidStateException @httpError(406) as its only state-based error.

## Steps to Reproduce
1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}, deletionProtection="ENABLED").
3. GetPolicyStore(policyStoreId=store) -> 200, but the body has no encryptionState and no cedarVersion (cause 1).
4. UpdatePolicyStore(policyStoreId=store, validationSettings={"mode":"STRICT"}) then GetPolicyStore -> validationSettings.mode is still "OFF" and lastUpdatedDate still equals createdDate (cause 2).
5. UpdatePolicyStore(policyStoreId=store, validationSettings={"mode":"OFF"}, deletionProtection="DISABLED") then GetPolicyStore -> deletionProtection is still "ENABLED" (cause 3).
6. DeletePolicyStore(policyStoreId=store) -> HTTP 406 InvalidStateException instead of a 200 empty success (cause 4).
7. Re-replay verifiedpermissions-policy-store-lifecycle-and-tags steps task#3 and task#9, and verifiedpermissions-policy-store-deletion-protection steps task#4 and teardown#5.
