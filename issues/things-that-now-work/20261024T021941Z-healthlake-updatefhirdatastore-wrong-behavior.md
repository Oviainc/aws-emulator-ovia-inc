Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# UpdateFHIRDatastore never puts the data store in UPDATING; the modelled transition statuses are unreachable

_Filed: 2026-10-24T02:19:41Z_
_Operation: UpdateFHIRDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A successful UpdateFHIRDatastore writes the requested configuration into the stored record synchronously and answers with DatastoreProperties whose DatastoreStatus is still ACTIVE. The record is never moved into the modelled UPDATING status and no later transition back to ACTIVE is scheduled, so neither UPDATING nor UPDATE_FAILED can ever be observed by any caller of DescribeFHIRDatastore / ListFHIRDatastores (including the ListFHIRDatastores DatastoreStatus filter, which accepts both values and can only ever match nothing).

Expected: an update of a data store's configuration (identity provider, analytics, profile, NLP, backup, name) is asynchronous work on the real service — the store reports DatastoreStatus=UPDATING while the change is being applied and settles back to ACTIVE (or UPDATE_FAILED) afterwards, which is why DatastoreStatus models those two values at all and why UpdateFHIRDatastore is the only operation that could produce them. A caller polling DescribeFHIRDatastore after an update should be able to observe the transient status, exactly as it can after CreateFHIRDatastore (CREATING) and DeleteFHIRDatastore (DELETING).

Actual: `src/ops/update_fhir_datastore.rs` mutates the record under the lock and returns with `status` untouched; the handler comment states the choice explicitly ("The data store stays ACTIVE … neither status is invented"). The crate's own state machine (`crate::state::DatastoreStatus`) has only Creating/Active/Deleting/Deleted variants, so there is no UPDATING state to enter and the `pending` due-instant mechanism the crate already uses for CREATING->ACTIVE and DELETING->DELETED is not used by this operation.

## Evidence

Smithy model: DatastoreStatus enum = CREATING | ACTIVE | DELETING | DELETED | CREATE_FAILED | UPDATING | UPDATE_FAILED (smithy_query shape=DatastoreStatus, quoted in tests/all/control_plane_error_semantics.rs:264-274); UpdateFHIRDatastore's output is DatastoreProperties, whose DatastoreStatus member is @required. Handler: services/service-com-amazonaws-healthlake-healthlake/src/ops/update_fhir_datastore.rs lines ~200-236 — fields are written and the record's status/pending are left alone (comment at lines 228-232 documents the decision). State: src/state.rs `DatastoreStatus` enum carries no Updating variant, and LIFECYCLE_TRANSITION/`pending` are only set by create and delete. Tests: tests/all/update_fhir_datastore.rs only asserts the store is ACTIVE *before* the update (line ~230) and never asserts the post-update status, so no test covers the transition either way. Distinct from the open issue 20261024T000307Z-healthlake-updatefhirdatastore-error-response, which is about AccessDeniedException vs ConflictException on the refusal path only.

## Steps to Reproduce

1. CreateFHIRDatastore with DatastoreTypeVersion=R4 and wait for DescribeFHIRDatastore to report DatastoreStatus=ACTIVE
2. Call UpdateFHIRDatastore with that DatastoreId and a changed member (e.g. DatastoreName=renamed, or an AnalyticsConfiguration)
3. Inspect the UpdateFHIRDatastore response: DatastoreProperties.DatastoreStatus is ACTIVE, not UPDATING
4. Immediately call DescribeFHIRDatastore for the same DatastoreId: the status is ACTIVE at every instant, so the UPDATING phase is never observable
5. Call ListFHIRDatastores with Filter={"DatastoreStatus":"UPDATING"} at any point after an update: the result is always empty, because no code path ever assigns that status
