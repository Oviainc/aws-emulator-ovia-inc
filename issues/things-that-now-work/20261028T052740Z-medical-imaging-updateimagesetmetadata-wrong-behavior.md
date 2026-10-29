Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# includeStudyImageSets counts DELETED image sets as members of the study, so one deleted image set blocks every study-wide update forever

_Filed: 2026-10-28T05:27:40Z_
_Operation: UpdateImageSetMetadata | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

With `includeStudyImageSets` set, the handler collects every other image set of the data store carrying the requested image set's `StudyInstanceUID` and then refuses the WHOLE request with `ConflictException` unless every one of them `is_updatable()` (i.e. `ACTIVE` with nothing pending). The collection step is the only image-set walk in this crate that does NOT filter out tombstones, and a retired image set keeps its `dicom_tags` (the retiring transition in `ImageSet::advance_to` clears `image_frames` and sets the state, never the DICOM attributes), so a DELETED image set is still counted as a member of the study and can never be `ACTIVE` again. Image-set records are never removed from `Datastore::image_sets` (nothing in the crate calls `image_sets.remove`/`retain`), so after any image set of a study is deleted, `UpdateImageSetMetadata` with `includeStudyImageSets=true` on a healthy sibling answers 409 `ConflictException` forever instead of applying the update. The handler itself applies the opposite rule one screen earlier for the image set the request NAMES (a tombstone is 404, "takes part in no further work"), and `SearchImageSets` / `DeleteDatastore` both filter tombstones out of their walks.

## Evidence

The walk that includes tombstones, and the refusal it feeds — `src/ops/update_image_set_metadata.rs:878-901`:
```
                if include_study_image_sets {
                    if let Some(study_instance_uid) = study_instance_uid {
                        let mut study: Vec<&ImageSet> = datastore
                            .image_sets
                            .values()
                            .filter(|other| {
                                other.image_set_id != input.image_set_id
                                    && other.dicom_tags.dicom_study_instance_uid.as_deref()
                                        == Some(study_instance_uid.as_str())
                            })
...
                        for other in study {
                            if !is_updatable(other) {
                                return Err(UpdateImageSetMetadataErrorModeled::ConflictException(
```
`is_updatable` can never hold for a tombstone — `src/ops/update_image_set_metadata.rs:596-598`:
```
fn is_updatable(image_set: &ImageSet) -> bool {
    image_set.latest_version.image_set_state == ImageSetState::Active
        && image_set.pending.is_none()
}
```
The SAME handler's rule for a tombstone, 80 lines above — `src/ops/update_image_set_metadata.rs:790-799`:
```
        // A deleted image set holds no metadata: its content went with the deletion, so
        // what is absent is the image set itself — the same answer `GetImageSetMetadata`
        // gives for the document it can no longer aggregate.
        if image_set.is_deleted() {
            return Err(UpdateImageSetMetadataErrorModeled::ResourceNotFoundException(
```
The crate-wide rule the walk breaks — `src/state.rs:1171-1176`:
```
    /// Whether this image set has been deleted — the terminal state a delete lands on,
    /// after which the record is a tombstone: it still reports its properties and its
    /// `deletedAt`, but holds no pixel data and takes part in no further work.
    pub fn is_deleted(&self) -> bool {
```
and `src/state.rs:1624-1626`: "an image set the ARN names has to be one that is not retired ([`ImageSet::is_deleted`]). A tombstone still REPORTS what it held, but it takes part in no further work, so nothing more is written onto it."
Sibling walks that DO filter — `src/ops/search_image_sets.rs:250-253` (`.filter(|image_set| !image_set.is_deleted())`) and `src/ops/delete_datastore.rs:184-188` (`.filter(|image_set| !image_set.is_deleted())`).
Retiring never clears the study attributes — `src/state.rs:1320-1328`: `if pending.retires_image_set { self.deleted_at = Some(pending.due_at); self.image_frames.clear(); ... self.set_state_on_every_version(pending.image_set_state); }`.
No test pins the deleted-sibling case: `tests/all/image_set_metadata_updates.rs:1144-1250` (`the_study_flag_decides_which_image_sets_one_update_reaches`) seeds only ACTIVE siblings, and the file contains no `Deleted` case for the study flag.

## Steps to Reproduce

1. Create a data store and let it settle to ACTIVE.
2. Seed/obtain two image sets in that data store carrying the same `StudyInstanceUID` (e.g. seed one as `tests/all/image_set_metadata_updates.rs` does and `CopyImageSet` it, which carries the source's DICOM attributes to the new image set).
3. `DeleteImageSet` one of the two and wait for `IMAGE_SET_DELETION_DELAY`, so its latest version is `DELETED` (`GetImageSet` reports DELETED + deletedAt).
4. `POST /datastore/{id}/imageSet/{survivor}/updateImageSetMetadata?latestVersion=1&includeStudyImageSets=true` with a valid `DICOMUpdates` document.
5. Observe 409 `ConflictException` with IMAGE_SET_NOT_UPDATABLE_MESSAGE; the same request with `includeStudyImageSets` unset answers 200, and no live image set of the study is in flight. Repeating the call always fails, because the tombstone is never reaped.
