Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# TagResource writes tags onto a DELETED data store that every other handler in the crate treats as gone

_Filed: 2026-10-25T15:36:15Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` resolves its `ResourceARN` to a stored `Datastore` and merges the request's tags into it WITHOUT looking at the record's `DatastoreStatus`. Because a deleted data store stays in `state.datastores` for `DELETED_RETENTION` after its `DELETING -> DELETED` transition falls due (state.rs:1522-1523 arms `reap_at`, and `reap_expired_records` only removes it once that instant passes), there is a 60-second window in which `TagResource` answers 200 and persists tags onto a record every other handler in this crate refuses as gone:

* `DeleteFHIRDatastore` answers `ResourceNotFoundException` for a `DELETED` record ("A deleted data store is not in transition — it is gone");
* `StartFHIRExportJob` and `RestoreFHIRDatastore` answer the same way, with the same comment;
* `UpdateFHIRDatastore` refuses ANY non-`ACTIVE` record with `AccessDeniedException` and `DATASTORE_NOT_ACTIVE_MESSAGE`, before any write, so "a refused update leaves the record exactly as it was";
* the FHIR data plane's `serving()` refuses `DELETING` and `DELETED` because "its endpoint holds nothing a caller may read or write".

So the crate has one rule — a `DELETED` record takes no mutation — and `TagResource` is the one mutating handler that skips it, even though it already takes the same `settle_lifecycle()` step that materialises the `DELETED` status it then ignores. The effect is the state resurrection the crate elsewhere guards against: a caller that deletes a data store and then tags it gets a 200, `ListTagsForResource` reports the new tag, and `DeleteFHIRDatastore` on the same id simultaneously answers 404 for the same record. `UntagResource` (untag_resource.rs:150-193) resolves the record through the identical status-blind lookup and removes keys from a DELETED record for the same reason; fixing `TagResource` should gate that path too. No comment, doc or test in the crate states a reason for the tagging surface to be exempt: `tests/all/resource_tagging.rs:9-14` states the rule under test as being about WHICH resource form the ARN names, and spans no `DatastoreStatus` axis at all.

## Evidence

No status gate in the write path — services/service-com-amazonaws-healthlake-healthlake/src/ops/tag_resource.rs:153-171,203:
```
        let mut store = state.lock();
        store.settle_lifecycle();
        // Whichever resource it is, the stored record's own ARN must equal the
        // one the caller handed in, ...
        let (tags, max, noun, not_found) = match resolved {
            Some(crate::state::TaggedResource::Datastore(id)) => {
                let datastore = store
                    .datastores
                    .get_mut(id)
                    .filter(|datastore| datastore.arn == input.resource_arn);
...
        merge_tags(tags, input.tags, max, noun)?;
```
(the only refusal between those lines is the `None => ResourceNotFoundException` arm for an unresolvable ARN; `datastore.status` is never read)

The rule every sibling applies — src/ops/delete_fhir_datastore.rs:204-212:
```
            // A deleted data store is not in transition — it is gone.
            crate::state::DatastoreStatus::Deleted => {
                return Err(DeleteFHIRDatastoreErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string()),
                    },
                )
                .into());
            }
```
src/ops/start_fhir_export_job.rs:239-248: "// A deleted data store is not in transition — it is gone, and\n                // there is nothing left to export from it." -> `ResourceNotFoundException`.
src/ops/restore_fhir_datastore.rs:272-281: same comment, same `ResourceNotFoundException`.
src/ops/update_fhir_datastore.rs:205-212:
```
        if datastore.status != crate::state::DatastoreStatus::Active {
            return Err(UpdateFHIRDatastoreErrorModeled::AccessDeniedException(
                AccessDeniedException {
                    message: Some(crate::state::DATASTORE_NOT_ACTIVE_MESSAGE.to_string()),
```
with the comment at :204 "The gate comes BEFORE any write, so a refused call leaves the record — and the transition it was already on — exactly as it was."
src/service/fhir_rest.rs:599-616: "A data store serves once it has been created and until it is destroyed: while it is still `CREATING`, and once it is `DELETING` or `DELETED`, its endpoint holds nothing a caller may read or write" — `serving()` matches only `Active | Updating`.

The window is real — src/state.rs:1520-1523:
```
                    // A record that has reached its terminal status starts its
                    // retention clock; everything else stays live.
                    if next == DatastoreStatus::Deleted {
                        datastore.reap_at = Some(now + DELETED_RETENTION);
```
(`DELETED_RETENTION` = 60s, state.rs:75), and tests/all/fhir_datastore_lifecycle.rs:1002-1007 pins that in that same window `DeleteFHIRDatastore` answers 400/`ResourceNotFoundException` for the id.

No test pins the tagging surface against a status: tests/all/resource_tagging.rs:9-14 states the rule as "`TagResource` merges the request's tags BY KEY onto the resource its `ResourceARN` names, whichever of the modelled resource forms that ARN has", and a grep for `DELETED|Deleted` over tests/all/resource_tagging.rs and tests/all/resource_untagging.rs matches nothing.

## Steps to Reproduce

1. CreateFHIRDatastore(DatastoreTypeVersion="R4", DatastoreName="tag.after.delete") and note DatastoreId and DatastoreArn
2. Poll DescribeFHIRDatastore until DatastoreStatus is ACTIVE
3. DeleteFHIRDatastore(DatastoreId) and poll DescribeFHIRDatastore until DatastoreStatus is DELETED (within the 60s DELETED_RETENTION window)
4. Call DeleteFHIRDatastore(DatastoreId) again — observe 400 ResourceNotFoundException "Could not find specified datastore.": the crate says the record is gone
5. Call TagResource(ResourceARN=<that datastore ARN>, Tags=[{Key:"k",Value:"v"}]) — observe HTTP 200 and no refusal
6. Call ListTagsForResource(ResourceARN=<same ARN>) — observe the tag was persisted onto the deleted data store, i.e. a mutation landed on a record DeleteFHIRDatastore/StartFHIRExportJob/RestoreFHIRDatastore/UpdateFHIRDatastore and the FHIR data plane all refuse as gone
7. Repeat step 5 with UntagResource(TagKeys=["k"]) — observe the same status-blind mutation succeeds
