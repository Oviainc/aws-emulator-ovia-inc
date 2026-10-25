Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 558d123cf10a98531c236bb7b81525d860a40117

# TagResource writes tags onto a retired image set and onto a data store that has not settled into ACTIVE

_Filed: 2026-10-25T20:52:19Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `558d123cf10a98531c236bb7b81525d860a40117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` resolves the resource its `resourceArn` names through `ComAmazonawsMedicalimagingAhigatewayserviceState::resource_tags_mut`, which applies neither of the two lifecycle guards the rest of this crate applies before writing:

1. **A retired image set still takes tags.** The image-set branch of `resource_tags_mut` is a bare `image_sets.get_mut(image_set_id)` with no `ImageSet::is_deleted()` filter, so an ARN carrying the `/imageset/<id>` suffix of an image set whose `DeleteImageSet` has already landed is resolved, answered HTTP 200, and the caller's tags are persisted onto the tombstone (and read back by `ListTagsForResource`). The crate declares the opposite: a retired image set "takes part in no further work", and every other handler that touches one refuses it.
2. **A data store mid-transition still takes tags.** `resource_tags_mut` reaches the store through `datastore_in_mut`, which filters only records whose deletion has already LANDED — not one still `CREATING` or already `DELETING`. So a `TagResource` against a store whose deletion is in flight answers 200 and persists tags that the clock discards ~`DATASTORE_DELETION_DELAY` later together with the record, i.e. a success whose whole effect is thrown away. Every other write path in this crate refuses that state with the modelled `ConflictException` and this service's own `DATASTORE_NOT_ACTIVE_MESSAGE`.

The same helper serves `UntagResource`, so the resolution gap is shared; `TagResource` is the path that persists caller data on the dead record, which is why it is named here.

## Evidence

Retired image set — the rule the crate states:

`src/state.rs:1079-1084`
```
/// Whether this image set has been deleted — the terminal state a delete lands on,
/// after which the record is a tombstone: it still reports its properties and its
/// `deletedAt`, but holds no pixel data and takes part in no further work.
pub fn is_deleted(&self) -> bool {
    self.latest_version.image_set_state == ImageSetState::Deleted
}
```

`tests/all/image_set_deletion.rs:704-706`
```
/// RULE: a retired image set takes part in no further work, and a data store holds none
/// of them — so the copy of a retired image set is the modelled conflict, and the data
/// store whose image sets have all been retired is deletable.
```

Siblings that honour it:
- `src/ops/search_image_sets.rs:253` — `.filter(|image_set| !image_set.is_deleted())`
- `src/ops/get_image_set_metadata.rs:370-377` — `if image_set.is_deleted() { return Err(... IMAGE_SET_NOT_FOUND_MESSAGE ...) }`
- `src/ops/get_image_frame.rs:189-196` — same refusal for a deleted image set
- `src/ops/copy_image_set.rs:341-346` — a non-`ACTIVE` image set is `ConflictException` / `IMAGE_SET_NOT_ACTIVE_MESSAGE`

The path that does not, reached by `TagResource`:

`src/state.rs:1482-1498`
```
pub fn resource_tags_mut(
    &mut self, region: &str, arn: &ResourceArn,
) -> Result<&mut HashMap<String, String>, MissingResource> {
    let datastore = self
        .datastore_in_mut(region, &arn.datastore_id)
        .filter(|datastore| datastore.datastore_arn == arn.datastore_arn)
        .ok_or(MissingResource::Datastore)?;
    match &arn.image_set_id {
        None => Ok(&mut datastore.tags),
        Some(image_set_id) => datastore
            .image_sets
            .get_mut(image_set_id)
            .map(|image_set| &mut image_set.tags)
            .ok_or(MissingResource::ImageSet),
    }
}
```
`src/ops/tag_resource.rs:194-217` calls it and then inserts the tags, with no state check of its own.

Data store mid-transition — the rule the crate states:

`src/state.rs:266-271`
```
/// The text this service refuses a call that would act on a data store, or on the
/// contents of one, that has not settled into `ACTIVE` with — one still `CREATING` or
/// already `DELETING`. Constant, and stated by rule rather than by which store it was,
pub const DATASTORE_NOT_ACTIVE_MESSAGE: &str =
    "The data store is not in the ACTIVE state, so it cannot serve this request.";
```

`tests/all/datastore_status_guard.rs:5-11`
```
//! RULE: a data store answers work on itself or on its contents only once it has settled
//! into `ACTIVE`; while a transition the service has already promised is mid-flight
//! (`CREATING` on the way to `ACTIVE`, `DELETING` on the way to gone), such a call is
//! refused with the operation's modelled `ConflictException` ...
```

Siblings that honour it: `src/ops/copy_image_set.rs:320-325`, `src/ops/delete_image_set.rs:177-182`, `src/ops/start_dicom_import_job.rs:195-200` — each `if !datastore.is_active() { ... DATASTORE_NOT_ACTIVE_MESSAGE ... }`. `src/state.rs:1406-1410` shows `datastore_in_mut` filters only `!datastore.is_deleted()`, so `CREATING`/`DELETING` pass through.

No test pins tagging against either state: `grep -n 'CREATING\|is_active\|NOT_ACTIVE' tests/all/resource_tagging.rs` → no matches.

## Steps to Reproduce

1. CreateDatastore, wait out DATASTORE_ACTIVATION_DELAY so the store is ACTIVE, and obtain an image set in it (the only minting path today is CopyImageSet from a seeded image set).
2. DeleteImageSet on that image set and wait out IMAGE_SET_DELETION_DELAY, so GetImageSet reports imageSetState=DELETED with a deletedAt.
3. POST /tags/<datastoreArn>/imageset/<imageSetId> with {"tags":{"k":"v"}} — observe HTTP 200 and the tag persisted, while GetImageFrame / GetImageSetMetadata / SearchImageSets all treat the same image set as gone.
4. GET /tags/<same ARN> — the tag written onto the tombstone is reported back.
5. Separately: CreateDatastore, wait for ACTIVE, DeleteDatastore (answers DELETING), then immediately POST /tags/<datastoreArn> with a tag — observe HTTP 200 where CopyImageSet/DeleteImageSet/StartDICOMImportJob against the same store answer 409 DATASTORE_NOT_ACTIVE_MESSAGE; after DATASTORE_DELETION_DELAY the record (and the tag the 200 promised) is gone.
