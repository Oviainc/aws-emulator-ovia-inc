Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: healthlake.describe-fhir-datastore-not-found healthlake.create-datastore-invalid-type-version-validation-error healthlake.fhir-datastore-full-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake.describe-fhir-datastore-not-found|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: healthlake.create-datastore-invalid-type-version-validation-error|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: healthlake.fhir-datastore-full-lifecycle-transitions|run#15|AssertionError|assert rename_code == "AccessDeniedException", (
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# HealthLake control-plane error semantics diverge: not-found and enum-validation messages are emulator-authored, and a mutation on a DELETED datastore is not refused (3 causes)

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFHIRDatastore

## Description
Three independent control-plane divergences in the HealthLake emulator. All three must be closed before submitting.

Cause 1 of 3: DescribeFHIRDatastore's ResourceNotFoundException message is synthesized from the request instead of HealthLake's fixed text.
AWS: Smithy models ResourceNotFoundException (httpError 404, client error, single Message member) on DescribeFHIRDatastore, documented "The requested data store was not found." The message itself is recorded behavior only: HealthLake answers a well-formed but never-created DatastoreId with the constant message "Could not find specified datastore." — the same text regardless of the id supplied, i.e. the rule is "a fixed service text that does not quote the requested id".
Emulator: the error code (ResourceNotFoundException) is correct, but Message is built from the request, e.g. "Datastore with ID 16b8b743def881049c293b98063f5970 does not exist" (recorded example; the id varies per run).
Why: callers and SDK/CLI users that match on the service's not-found text — and golden-output comparisons — see a different sentence, and the response echoes the requested resource id where AWS deliberately does not.
Fix: the not-found path of the DescribeFHIRDatastore handler / the service's error construction must emit HealthLake's fixed not-found text rather than formatting the requested id into it (services/service-com-amazonaws-healthlake-healthlake/src/ops/describe_fhir_datastore.rs and the crate's error helper).
Traces: healthlake.describe-fhir-datastore-not-found.

Cause 2 of 3: CreateFHIRDatastore's enum ValidationException message echoes the rejected value, which HealthLake omits.
AWS: Smithy: CreateFHIRDatastoreRequest.DatastoreTypeVersion targets FHIRVersion, an enum whose only value is R4; ValidationException is the service-common error (httpError 400). The message is request-built, so its rule is known from the recording: HealthLake frames an enum violation as "1 validation error detected: Value at 'datastoreTypeVersion' failed to satisfy constraint: Member must satisfy enum value set: [R4]" — it names the member and the permitted value set but does NOT quote the offending value.
Emulator: same code (ValidationException) and same framing, but it inserts the rejected value: "1 validation error detected: Value 'R5' at 'datastoreTypeVersion' failed to satisfy constraint: Member must satisfy enum value set: [R4]". (Positive control R4 succeeds and the rejected name creates nothing, so only the message construction diverges.)
Why: a client that surfaces or asserts on HealthLake's validation text gets a different sentence for every enum violation, and the emulator reflects caller input back into error messages where this service does not.
Fix: the enum-constraint branch of the shared request-validation/message-rendering component used by CreateFHIRDatastore must omit the offending value for this service's framing (services/service-com-amazonaws-healthlake-healthlake/src/ops/create_fhir_datastore.rs validation path / the crate's validation message builder).
Traces: healthlake.create-datastore-invalid-type-version-validation-error.

Cause 3 of 3: UpdateFHIRDatastore against a datastore that has reached DELETED is not state-gated — it answers NotImplemented instead of AccessDeniedException.
AWS: Smithy: UpdateFHIRDatastore declares AccessDeniedException (httpError 403), ConflictException, ResourceNotFoundException. Recorded behavior: a mutation on a datastore whose DatastoreStatus is DELETED is refused with error code AccessDeniedException and the service text "Specified datastore is not active." — not ResourceNotFoundException and not ConflictException.
Emulator: the full CREATING->ACTIVE->DELETING->DELETED status narrative replays correctly, but the final UpdateFHIRDatastore returns code 'NotImplemented' ("operation UpdateFHIRDatastore is scaffolded but not implemented in this emulator"), so the dead-id refusal cannot be observed. Note: a separate already-filed issue covers implementing the UpdateFHIRDatastore scaffold stub itself (it was filed from the update-datastore-while-creating-rejected check); that issue does not carry this check's replay key or the DELETED-state gate, so the requirement is recorded here: once implemented, UpdateFHIRDatastore must reject a non-ACTIVE (DELETED) target with 403 AccessDeniedException rather than succeeding, 404ing or conflicting.
Why: a client retrying a rename against an id whose datastore is gone gets an unmodeled NotImplemented today, and once the stub is filled in would silently succeed unless the status gate exists — resurrecting state for a deleted data store.
Fix: UpdateFHIRDatastore's handler must check the target datastore's status before mutating and return AccessDeniedException for anything that is not ACTIVE (services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs plus the datastore status in the service state).
Traces: healthlake.fhir-datastore-full-lifecycle-transitions.

## Evidence
Cause 1 — healthlake.describe-fhir-datastore-not-found: code assertion on ResourceNotFoundException passes; AssertionError: expected message 'Could not find specified datastore.', got 'Datastore with ID 16b8b743def881049c293b98063f5970 does not exist' (id value is per-run).

Cause 2 — healthlake.create-datastore-invalid-type-version-validation-error: code assertion on ValidationException passes; AssertionError: expected message "1 validation error detected: Value at 'datastoreTypeVersion' failed to satisfy constraint: Member must satisfy enum value set: [R4]", got "1 validation error detected: Value 'R5' at 'datastoreTypeVersion' failed to satisfy constraint: Member must satisfy enum value set: [R4]".

Cause 3 — healthlake.fhir-datastore-full-lifecycle-transitions: all status assertions (CREATING on create, ACTIVE after polling, DELETING on delete, DELETED after polling) pass; AssertionError: expected update on a DELETED datastore to fail with AccessDeniedException, got 'NotImplemented' (detail: operation UpdateFHIRDatastore is scaffolded but not implemented in this emulator). Expected message example: 'Specified datastore is not active.'

## Steps to Reproduce
Cause 1: CreateFHIRDatastore(DatastoreTypeVersion="R4") to get a real DatastoreId, then call DescribeFHIRDatastore with that id perturbed in one character (still matching DatastoreId's length/pattern). Expect 404 ResourceNotFoundException with the fixed text 'Could not find specified datastore.'; observe a message that formats the requested id.

Cause 2: Call CreateFHIRDatastore(DatastoreName=<unused name>, DatastoreTypeVersion="R5"). Expect 400 ValidationException whose message names 'datastoreTypeVersion' and the [R4] value set WITHOUT quoting 'R5'; observe the value echoed as "Value 'R5' at ...".

Cause 3: CreateFHIRDatastore, poll DescribeFHIRDatastore to ACTIVE, DeleteFHIRDatastore (DatastoreStatus DELETING), poll to DELETED, then call UpdateFHIRDatastore(DatastoreId=<that id>, DatastoreName=<new name>). Expect 403 AccessDeniedException ('Specified datastore is not active.'); observe NotImplemented.
