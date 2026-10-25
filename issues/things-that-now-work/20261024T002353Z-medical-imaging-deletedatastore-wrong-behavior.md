Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# DatastoreStatus DELETED is unreachable: a deleted data store jumps from DELETING to 404

_Filed: 2026-10-24T00:23:53Z_
_Operation: DeleteDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the deletion the clock owes falls due, `advance_clock` sets the record's status to `Deleted` and in the SAME step drops the record from the map, so no response can ever carry `datastoreStatus: DELETED`:

- `GetDatastore` reports `DELETING` and then 404 `ResourceNotFoundException`.
- `ListDatastores?datastoreStatus=DELETED` can only ever return an empty page.

The model declares `DELETED` as a `DatastoreStatus` value reported by `DatastoreProperties` / `DatastoreSummary` and accepted as the `datastoreStatus` listing filter, which only makes sense if a finished deletion is observable in that status for a window before the record is reaped. Expected: after the deletion completes a caller can observe the data store in `DELETED` (via `GetDatastore` and via the `datastoreStatus=DELETED` filter) for some retention window, then it disappears. Actual: `DELETED` is dead code in the emulator.

## Evidence

Spec: `smithy_query(query="shape", shape="DatastoreStatus")` / `smithy_query(query="operation", operation="ListDatastores")` — `DELETED` is a modeled status value and an accepted `datastoreStatus` query filter; `DatastoreProperties.datastoreStatus` and `DatastoreSummary.datastoreStatus` are the members that would report it.
Handler/state: `src/state.rs` `Datastore::advance_to` lines 471-476 sets `DatastoreStatus::Deleted` and returns `false`, and `advance_clock` (lines 570-579) `retain`s it out of the map in the same pass — so the `Deleted` value is assigned and immediately discarded.
Tests: `tests/all/datastore_lifecycle.rs::listing_filters_by_every_modelled_datastore_status` encodes `("DELETED", BTreeSet::new())` (line 760) and `a_deleted_datastore_disappears_and_leaves_its_neighbours_alone` asserts a 404 right after the deletion span — i.e. the current (unreachable-DELETED) behavior is pinned, not the modeled one.

## Steps to Reproduce

1. CreateDatastore and wait for ACTIVE
2. DELETE /datastore/{datastoreId} → datastoreStatus DELETING
3. Wait past the deletion span
4. GET /datastore/{datastoreId} → 404 ResourceNotFoundException; no response ever reported datastoreStatus DELETED
5. GET /datastore?datastoreStatus=DELETED&maxResults=50 → always an empty datastoreSummaries list
