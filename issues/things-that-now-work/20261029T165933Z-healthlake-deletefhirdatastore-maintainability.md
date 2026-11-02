Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# The data-store status gate (which statuses refuse an action, and as what) is copied into three handlers

_Filed: 2026-10-29T16:59:33Z_
_Operation: DeleteFHIRDatastore | Class: maintainability_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers in this crate decide, from a data store's `DatastoreStatus`, whether an action may proceed and which refusal it earns. All three implement the SAME partition, arm for arm: `ACTIVE` proceeds; `CREATING`/`UPDATING`/`DELETING` are the transition conflict, worded by the shared `crate::state::datastore_in_transition_message(status, action)`; `DELETED` is "gone", worded by the shared `crate::state::DATASTORE_NOT_FOUND_MESSAGE`. Only the modelled error SHAPE differs between the copies (`ConflictException` where the operation models one, `ValidationException` for `StartFHIRExportJob`, which does not).

The crate has already factored out everything else about this refusal into `state.rs` — the sentence (`datastore_in_transition_message`), the action vocabulary (`DatastoreAction`, whose doc says "a new one has to add its variant here") and the not-found text — and its own doc states the point of doing so: "Every operation in this crate states its resource-state conflict through this one function, so none of them can drift into a different shape of refusal". The one part that is NOT shared is the status partition itself, so the copies can drift: a status added to the slice (`CREATE_FAILED`, `UPDATE_FAILED` are already declared by the modelled enum) or a fourth caller has to be remembered in three independent `match` arms, and only whichever handler its author edited would change. The test side of the same rule is already centralised — `tests/all/datastore_state_conflict.rs` exports the one `states()` table "so they cannot drift about which states the slice can reach" — which is exactly the asymmetry this reports.

The fix is response-preserving: one helper on the state side (e.g. a `Datastore` method taking a `DatastoreAction` and answering "proceed" / "in transition, with this sentence" / "gone, with this sentence") that each handler maps into its OWN modelled error variant, leaving every status, every error shape and every message byte identical, and leaving `UpdateFHIRDatastore` alone — it deliberately draws a different line (`DATASTORE_NOT_ACTIVE_MESSAGE`, pinned by `tests/all/datastore_update_not_active.rs`) and must keep it.

## Evidence

COPY 1 — services/service-com-amazonaws-healthlake-healthlake/src/ops/delete_fhir_datastore.rs:180-213:
```
        match datastore.status {
            // Only a settled, usable data store can be deleted.
            crate::state::DatastoreStatus::Active => {}
            ...
            crate::state::DatastoreStatus::Creating
            | crate::state::DatastoreStatus::Updating
            | crate::state::DatastoreStatus::Deleting => {
                return Err(DeleteFHIRDatastoreErrorModeled::ConflictException(
                    ConflictException {
                        message: Some(crate::state::datastore_in_transition_message(
                            datastore.status,
                            crate::state::DatastoreAction::Delete,
                        )),
                    },
                )
                .into());
            }
            // A deleted data store is not in transition — it is gone.
            crate::state::DatastoreStatus::Deleted => {
                return Err(DeleteFHIRDatastoreErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string()),
                    },
                )
                .into());
            }
        }
```

COPY 2 — src/ops/restore_fhir_datastore.rs:247-282 (same partition, `DatastoreAction::Restore`):
```
            match source.status {
                // Only a settled, usable data store can be restored from.
                crate::state::DatastoreStatus::Active => {}
                crate::state::DatastoreStatus::Creating
                | crate::state::DatastoreStatus::Updating
                | crate::state::DatastoreStatus::Deleting => {
                    return Err(RestoreFHIRDatastoreErrorModeled::ConflictException(
                        ConflictException {
                            message: Some(crate::state::datastore_in_transition_message(
                                source.status,
                                crate::state::DatastoreAction::Restore,
                            )),
                        },
                    )
                    .into());
                }
                crate::state::DatastoreStatus::Deleted => {
                    return Err(RestoreFHIRDatastoreErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string()),
                        },
                    )
                    .into());
                }
            }
```

COPY 3 — src/ops/start_fhir_export_job.rs:260-291 (same partition, `DatastoreAction::ExportFrom`, only the error shape differs):
```
            match datastore.status {
                // Only a settled, usable data store can be exported from.
                crate::state::DatastoreStatus::Active => {}
                // ... This operation models no `ConflictException`, so the
                // refusal is the `ValidationException` it does model, carrying
                // the one sentence every operation on this resource states a
                // transition state with.
                crate::state::DatastoreStatus::Creating
                | crate::state::DatastoreStatus::Updating
                | crate::state::DatastoreStatus::Deleting => { ... datastore_in_transition_message(
                                datastore.status,
                                crate::state::DatastoreAction::ExportFrom,
                            ) ... }
                crate::state::DatastoreStatus::Deleted => { ... DATASTORE_NOT_FOUND_MESSAGE ... }
            }
```

THE CRATE'S OWN STATED DIRECTION — src/state.rs:199-201:
```
/// Every operation in this crate states its resource-state conflict through
/// this one function, so none of them can drift into a different shape of
/// refusal or into echoing the identifier.
```
and src/state.rs:213-217:
```
/// A typed value rather than a free-form word: the operations of this resource
/// cannot drift into different spellings of the same sentence, and a new one has
/// to add its variant here. `UpdateFHIRDatastore` is deliberately absent...
```

THE TEST SIDE IS ALREADY SHARED — tests/all/datastore_state_conflict.rs:54-57:
```
/// `pub(crate)` beside [`datastore_in`], which drives a data store to one of
/// them: the sibling suites in `tests/all/` span the statuses from this one
/// table rather than each copying it, so they cannot drift about which states
/// the slice can reach.
```

TEST COVERAGE that must stay unchanged: tests/all/datastore_state_conflict.rs:172 (delete, every status), tests/all/datastore_transition_conflict_sentence.rs:80-90,197,260-263 (all three actions × every transition status, sentence read from the crate), tests/all/fhir_export_job_start.rs:421,440 (export-from gate), tests/all/restore_fhir_datastore.rs and tests/all/idempotency_replay_precedes_state_gates.rs:121 (replay precedes the export gate), plus tests/all/datastore_update_not_active.rs (the deliberately different `UpdateFHIRDatastore` line).

## Steps to Reproduce

1. Read `src/ops/delete_fhir_datastore.rs:180-213`, `src/ops/restore_fhir_datastore.rs:247-282` and `src/ops/start_fhir_export_job.rs:260-291` side by side: the same four-arm `DatastoreStatus` partition, the same shared sentence helper and the same shared not-found constant appear three times.
2. Observe that `src/state.rs` already owns every other part of this refusal (the sentence, the `DatastoreAction` vocabulary, `DATASTORE_NOT_FOUND_MESSAGE`) and that its doc at state.rs:199-201 states the drift it is there to prevent.
3. Simulate a change the crate invites: add a status the modelled `DatastoreStatus` enum already declares (e.g. `CREATE_FAILED`) or a fourth action on a data store. Each of the three `match` blocks must be edited independently; editing only one leaves the other two classifying that status differently.
4. Confirm the fix is response-preserving: factor the partition into one state-side helper that answers "proceed / in transition (sentence) / gone (sentence)" and have each handler wrap the answer in its own modelled error variant — every status, shape and message stays identical and the suites listed in the evidence keep passing unchanged.
