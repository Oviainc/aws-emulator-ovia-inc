Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4244fec3063c58aa4a2476d7619962eecf74c002

# RestoreFHIRDatastore hand-writes the transition-conflict sentence the crate's shared helper owns

_Filed: 2026-10-25T04:48:01Z_
_Operation: RestoreFHIRDatastore | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4244fec3063c58aa4a2476d7619962eecf74c002`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::datastore_in_transition_message(status, DatastoreAction)` is this crate's single place for the sentence a data store in a lifecycle transition (`CREATING`/`UPDATING`/`DELETING`) is refused with, and its doc comment asserts that EVERY operation of the crate states its resource-state conflict through it. `DeleteFHIRDatastore` and `StartFHIRExportJob` do call it. `RestoreFHIRDatastore` does not: it rebuilds the same sentence with its own inline `format!`, hard-coding the helper's format string ("Specified datastore is in status {} and cannot be ...") and the participle that `DatastoreAction::as_participle` is the typed home for. So the rule is implemented twice in this crate and the two copies can drift: a change to the shared sentence (or to the choice not to echo the requested identifier) silently leaves RestoreFHIRDatastore behind, and `DatastoreAction` — whose doc explicitly enumerates the one operation that is "deliberately absent" (`UpdateFHIRDatastore`) — does not mention restore at all, so the omission reads as an oversight rather than a decision. Adding a `DatastoreAction` variant whose participle is `"restored"` and calling the helper leaves the response bytes identical (the inline format string is character-for-character the helper's) and leaves every existing test unchanged.

## Evidence

Shared helper, with the claim the code breaks — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:187-196: "Every operation in this crate states its resource-state conflict through this one function, so none of them can drift into a different shape of refusal or into echoing the identifier." / `pub fn datastore_in_transition_message(status: DatastoreStatus, action: DatastoreAction) -> String { format!("Specified datastore is in status {} and cannot be {}.", status.as_wire(), action.as_participle()) }`.
The typed action enum that is supposed to hold every such operation — state.rs:198-212: "A typed value rather than a free-form word: the operations of this resource cannot drift into different spellings of the same sentence, and a new one has to add its variant here. `UpdateFHIRDatastore` is deliberately absent..." with `pub enum DatastoreAction { Delete, /* ExportFrom */ }` — no restore variant.
The second copy — services/service-com-amazonaws-healthlake-healthlake/src/ops/restore_fhir_datastore.rs:257-267: `return Err(RestoreFHIRDatastoreErrorModeled::ConflictException(ConflictException { ... message: Some(format!("Specified datastore is in status {} and cannot be restored.", source.status.as_wire())), }`.
The sibling that does use the helper — services/service-com-amazonaws-healthlake-healthlake/src/ops/delete_fhir_datastore.rs:196-199: `message: Some(crate::state::datastore_in_transition_message(datastore.status, crate::state::DatastoreAction::Delete,))` ("Stated through the one shared helper every mutation here uses."), and services/service-com-amazonaws-healthlake-healthlake/src/ops/start_fhir_export_job.rs:231-234 with `DatastoreAction::ExportFrom`.
Test coverage that pins the behaviour the fix must preserve: tests/all/restore_fhir_datastore.rs:735-771 (a CREATING source and a DELETING source are both refused 400 `ConflictException`), and tests/all/datastore_state_conflict.rs for the delete/export conflict paths.

## Steps to Reproduce

1. Read services/service-com-amazonaws-healthlake-healthlake/src/state.rs:173-222 — the shared `datastore_in_transition_message` helper, its claim that every operation in the crate states its resource-state conflict through it, and the `DatastoreAction` enum that is meant to carry one variant per such operation
2. Read services/service-com-amazonaws-healthlake-healthlake/src/ops/restore_fhir_datastore.rs:247-269 — the ConflictException arm rebuilds the helper's sentence with its own `format!` instead of calling it
3. Compare with services/service-com-amazonaws-healthlake-healthlake/src/ops/delete_fhir_datastore.rs:190-203 and src/ops/start_fhir_export_job.rs:229-237, which both call the helper
4. Observe that changing the shared sentence or the no-identifier-echo rule in state.rs would not change RestoreFHIRDatastore's refusal, so the two copies can disagree about one data store's conflict
