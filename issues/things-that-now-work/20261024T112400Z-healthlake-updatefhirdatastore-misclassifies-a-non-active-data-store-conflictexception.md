Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.update-datastore-while-creating-rejected healthlake.fhir-datastore-full-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.update-datastore-while-creating-rejected|run#12|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r} detail={detail!r}"
Issue-Key: healthlake.fhir-datastore-full-lifecycle-transitions|run#15|AssertionError|assert rename_code == "AccessDeniedException", (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# UpdateFHIRDatastore misclassifies a non-ACTIVE data store: ConflictException while CREATING and ResourceNotFoundException when DELETED, instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFHIRDatastore

## Description
REOPENS 20261024T000307Z-healthlake-updatefhirdatastore-error-response.md: its fix left the call refused. That (now 'fixed') issue reasoned from the Smithy docs that a data store in transition should be refused with ConflictException and the handler was changed accordingly; replayed behavior shows real AWS refuses ANY non-ACTIVE data store with AccessDeniedException ('Specified datastore is not active.'), so the refusal path is still wrong — only the code it answers with changed. This issue bundles 2 root causes, both in the UpdateFHIRDatastore status gate. BOTH must be closed before submitting.

Cause 1 of 2: a data store still in a lifecycle transition (CREATING, and by the same gate DELETING) is refused with the wrong error code.
AWS: UpdateFHIRDatastore models AccessDeniedException (HTTP 403), ConflictException (409), ResourceNotFoundException (404), ValidationException (400), ThrottlingException (429), InternalServerException (500) (Smithy: UpdateFHIRDatastore.errors; input DatastoreId @required, @length 1-32). The model does not state which status maps to which error, so this is recorded behavior only: against real AWS, an UpdateFHIRDatastore sent to a data store that is not ACTIVE is refused with error code AccessDeniedException, message 'Specified datastore is not active.' (message quoted as the recorded example; the requirement is the 403/AccessDeniedException classification for a non-ACTIVE target). The name is NOT applied — the check confirms DescribeFHIRDatastore still reports the original name after the refusal, and the same update succeeds once the store is ACTIVE.
Emulator: returns error code ConflictException (409) with message 'Specified datastore is in status CREATING and cannot be updated.'
Why: a client that distinguishes a transient resource-state conflict from an authorization/state refusal branches wrongly — ConflictException reads as retryable-after-transition and as a different failure class, so retry/backoff and error-handling paths that key on AccessDeniedException never fire, and code that keys on ConflictException fires only against the emulator.
Fix: the status gate in the UpdateFHIRDatastore handler (the operation's validation/refusal path), not its serializer; the sibling classification in DeleteFHIRDatastore must not be relabelled to match, since only UpdateFHIRDatastore's recorded refusal is in evidence here.
Traces: healthlake.update-datastore-while-creating-rejected (assertion: expected AccessDeniedException, got 'ConflictException'). Handler gate at services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs:195-222 (CREATING/DELETING arm at :200-210). Note tests/all/control_plane_error_semantics.rs pins the current classification and will need to move with the fix.

Cause 2 of 2: a data store in the terminal DELETED status is refused as if it did not exist.
AWS: recorded behavior only — after DeleteFHIRDatastore settles to DatastoreStatus=DELETED, the record remains readable by DescribeFHIRDatastore (the check polls it to DELETED through Describe), and a mutation against that id is refused with error code AccessDeniedException and message 'Specified datastore is not active.' — i.e. the same non-ACTIVE refusal as Cause 1, NOT a not-found. ResourceNotFoundException is reserved for an id with no record at all.
Emulator: returns error code ResourceNotFoundException (404) with the generic datastore-not-found message, by an explicit Deleted arm that treats the terminal record as gone even though the record is still present and still described.
Why: the emulator contradicts itself — DescribeFHIRDatastore says the id exists and is DELETED while UpdateFHIRDatastore says the id does not exist. A client that treats ResourceNotFoundException as 'safe to recreate/ignore' or that uses not-found as an existence probe draws the opposite conclusion from the two operations, and the refusal class a real client sees (403 AccessDenied) never appears.
Fix: the same status gate in the UpdateFHIRDatastore handler — the Deleted arm must take the non-ACTIVE refusal path rather than the not-found path; the not-found path stays for ids absent from state.
Traces: healthlake.fhir-datastore-full-lifecycle-transitions (assertion: expected update on a DELETED datastore to fail with AccessDeniedException, got 'ResourceNotFoundException'). Handler Deleted arm at services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs:214-221; the missing-record path it duplicates is at :179-186.

Distinct from open issue 20261024T021941Z-healthlake-updatefhirdatastore-wrong-behavior.md, which concerns the SUCCESS path never entering UPDATING; that issue does not cover either refusal classification above.

## Evidence
Check healthlake.update-datastore-while-creating-rejected: create a datastore (response DatastoreStatus=CREATING), confirm via DescribeFHIRDatastore it is still CREATING, then UpdateFHIRDatastore(DatastoreId, DatastoreName=<new>).
  Expected: ClientError with Error.Code == 'AccessDeniedException' (HTTP 403), Error.Message == 'Specified datastore is not active.'
  Actual:   Error.Code == 'ConflictException', Error.Message == 'Specified datastore is in status CREATING and cannot be updated.'
  (The positive control in the same check — the identical update succeeding once ACTIVE — is not in dispute.)

Check healthlake.fhir-datastore-full-lifecycle-transitions: create -> poll to ACTIVE -> DeleteFHIRDatastore (response DatastoreStatus=DELETING) -> poll Describe to DELETED -> UpdateFHIRDatastore(DatastoreId, DatastoreName=<new>).
  Expected: ClientError with Error.Code == 'AccessDeniedException' (HTTP 403), Error.Message == 'Specified datastore is not active.'
  Actual:   Error.Code == 'ResourceNotFoundException' (HTTP 404), generic datastore-not-found message, even though DescribeFHIRDatastore for the same id still answers with DatastoreStatus=DELETED.

Model (Smithy, healthlake): UpdateFHIRDatastore errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); output DatastoreProperties @required.
Code: services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs:195-222 — Active => {}, Creating|Deleting => ConflictException, Deleted => ResourceNotFoundException.
History: issue 20261024T000307Z-healthlake-updatefhirdatastore-error-response.md (status: fixed) changed this gate from AccessDeniedException to ConflictException on doc reasoning alone; replay against the recorded AWS behavior contradicts that change for both arms.

## Steps to Reproduce
Cause 1 (CREATING):
1. CreateFHIRDatastore(DatastoreName=<unique>, DatastoreTypeVersion='R4') — response DatastoreStatus is CREATING.
2. While the record is still CREATING (verify with DescribeFHIRDatastore), call UpdateFHIRDatastore(DatastoreId=<id>, DatastoreName=<other-unique>).
3. Observe Error.Code: emulator answers ConflictException (409); AWS answers AccessDeniedException (403) with message 'Specified datastore is not active.'
4. DescribeFHIRDatastore and confirm the name is unchanged (this part already matches).

Cause 2 (DELETED):
1. CreateFHIRDatastore(DatastoreName=<unique>, DatastoreTypeVersion='R4') and poll DescribeFHIRDatastore until DatastoreStatus=ACTIVE.
2. DeleteFHIRDatastore(DatastoreId=<id>) — response DatastoreStatus is DELETING — then poll DescribeFHIRDatastore until DatastoreStatus=DELETED.
3. Call UpdateFHIRDatastore(DatastoreId=<id>, DatastoreName=<other-unique>).
4. Observe Error.Code: emulator answers ResourceNotFoundException (404); AWS answers AccessDeniedException (403) with message 'Specified datastore is not active.' while DescribeFHIRDatastore for the same id still returns the DELETED record.

Replay: run_behavioral_suite and confirm healthlake.update-datastore-while-creating-rejected and healthlake.fhir-datastore-full-lifecycle-transitions both pass.
