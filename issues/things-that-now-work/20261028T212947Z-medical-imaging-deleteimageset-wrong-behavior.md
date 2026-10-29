Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 83b2b15993ea2e3b2b9e4b142ed7a7a8695c4dfd

# A retired image set keeps the DICOM attributes the crate's own rules say the deletion took with it (only the pixel data is released)

_Filed: 2026-10-28T21:29:47Z_
_Operation: DeleteImageSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `83b2b15993ea2e3b2b9e4b142ed7a7a8695c4dfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the clock lands a deletion, `ImageSet::advance_to` releases only the pixel data: it clears `image_frames` and leaves `dicom_tags` and `extra_attributes` — and every version's `superseded_attributes` snapshot — exactly as they were. So a tombstone goes on holding the patient, study and series attributes (patient id, patient name, birth date, accession number, plus up to `MAX_IMAGE_SET_EXTRA_ATTRIBUTES` = 512 caller-written attributes per record) of content the service reports as deleted.

Three rules this crate states about a retired image set say the opposite: the transition that retires it is documented as taking the content with it, `held_image_sets` is documented as resting on a tombstone having "given up the content it held", and `CopyImageSet` justifies its 404 for a DELETED source or destination with "the deletion took the pixel data and the DICOM attributes with it". The code implements the first half of that sentence and not the second.

No reader reports a tombstone's attributes today (every content read — `GetImageSetMetadata`, `GetImageFrame`, `UpdateImageSetMetadata`, `CopyImageSet` — answers 404 on `ImageSet::is_deleted`, and `SearchImageSets` walks `held_image_sets`), so the defect is not visible in a response. What it does mean is that a clinical service holds a deleted image set's patient identifiers in process memory for the life of the record, and that any future reader of `ImageSet::dicom_tags` that does not first consult `is_deleted` will report content the crate's own documentation says is gone.

## Evidence

What the retirement releases — services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:1337-1349:
```
// A deletion the clock owed lands the same way, at the DUE instant rather than at
// `now`, so `deletedAt` does not move between reads — and the pixel data this
// image set held goes with it, because a deleted image set holds none.
if pending.retires_image_set {
    self.deleted_at = Some(pending.due_at);
    self.image_frames.clear();
```
There is no `set_attributes(ImageSetAttributes::default())` (or equivalent) anywhere in that branch, and `dicom_tags` / `extra_attributes` are written nowhere else on the delete path (`src/ops/delete_image_set.rs:211-231` only sets state, workflow status, `updated_at` and `pending`).

The rules the crate states about the same records:
* state.rs:1442-1448 (`Datastore::held_image_sets`): "A tombstone occupies no place: an image set whose deletion has landed **has given up the content it held**, so it neither takes a place among the image sets the store holds nor can be worked on".
* src/ops/copy_image_set.rs:73-77: "Because the copy carries that content, a `DELETED` image set named as its source or as its destination is what is ABSENT rather than what is busy: **the deletion took the pixel data and the DICOM attributes with it**, so the copy answers 404 `ResourceNotFoundException` …".
* state.rs:1104-1109 says `dicom_tags` is "The DICOM attributes of the patient, study and series this image set holds", and state.rs:1110-1114 the same for `extra_attributes` — i.e. held CONTENT, which the sentences above say a tombstone has given up.

Test coverage: the deletion suite asserts the pixel-data half only — tests/all/image_set_deletion.rs:218 `a_delete_locks_the_image_set_and_the_clock_retires_it` and :412 `the_clock_alone_lands_the_retirement_the_delete_promised` check the state/workflow pair, `deletedAt`, and that `GetImageFrame` no longer serves the frames; tests/all/image_set_capacity.rs:341 `the_record_cap_counts_every_image_set_record_including_the_tombstones` pins that the RECORD survives. No test asserts anything about what a tombstone still holds, so clearing the attributes changes no existing assertion.

## Steps to Reproduce

1. Seed an image set holding DICOM attributes (patient id/name, study instance UID, plus attributes outside the modelled ones written through UpdateImageSetMetadata) in an ACTIVE data store.
2. Call POST /datastore/{datastoreId}/imageSet/{imageSetId}/deleteImageSet and wait out IMAGE_SET_DELETION_DELAY so the clock lands the retirement (GetImageSet then reports DELETED/DELETED with a deletedAt).
3. Inspect the surviving record through `crate::state::service_state` (the records are reachable for the DATASTORE_DELETED_RETENTION-independent life of the image set record): `image_frames` is empty, but `dicom_tags` still carries the patient and study attributes and `extra_attributes` still carries every attribute written into the image set, as does each version's `superseded_attributes`.
4. Compare with copy_image_set.rs:73-77 and state.rs:1442-1448, which state that a retired image set has given up the content it held — pixel data AND DICOM attributes.
