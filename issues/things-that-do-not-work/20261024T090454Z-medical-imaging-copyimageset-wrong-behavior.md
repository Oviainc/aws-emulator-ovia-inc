Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# promoteToPrimary is applied as a flag on the named destination instead of producing a new primary image set

_Filed: 2026-10-24T09:04:54Z_
_Operation: CopyImageSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`promoteToPrimary` is documented as: "Providing this parameter will configure the CopyImageSet operation to promote the given image set to the primary DICOM hierarchy. If successful, a NEW primary image set ID will be returned as the destination image set." The emulator does not implement that: when the request carries both `copyImageSetInformation.destinationImageSet` and `promoteToPrimary=true`, the handler pushes a new version onto the caller-named destination image set and merely flips that record's `isPrimary` boolean, so the response's `destinationImageSetProperties.imageSetId` is the id the caller supplied — never a newly minted primary image set id, and `latestVersionId` of the caller's image set is bumped instead.

The flag is also the ONLY way an image set in this emulator can ever become primary: `ImageSet::new` is called with `is_primary = promote_to_primary`, so a plain full copy (no `promoteToPrimary`) mints a destination with `isPrimary=false`, and `GetImageSet` then reports `isPrimary: false` for a full copy of a primary image set. In AWS, image sets in the primary DICOM hierarchy are the norm (an import produces primary image sets, and only subset copies produce non-primary ones), so defaulting every minted copy to non-primary inverts the common case.

Expected: with `promoteToPrimary=true` the operation answers with a newly minted destination image set id that is primary (and the combination with an explicit `destinationImageSet` is resolved the way the service does, not by versioning the caller's image set); a full copy's destination inherits the source's primary-hierarchy membership. Actual: `promoteToPrimary` is a boolean stored on whichever record the `destinationImageSet` member named, and `isPrimary` is false unless the caller passed the flag.

## Evidence

Smithy model: `smithy_query(query="doc",shape="CopyImageSetRequest",member="promoteToPrimary")` → "...a new primary image set ID will be returned as the destination image set." Handler: `src/ops/copy_image_set.rs` — `let promote_to_primary = input.promote_to_primary.unwrap_or(false);` then (a) `DestinationPlan::Mint` passes `promote_to_primary` straight into `ImageSet::new(..., promote_to_primary, forced, now)` as `is_primary`, and (b) `DestinationPlan::Version` does `image_set.push_version(...)` followed by `if promote_to_primary { image_set.is_primary = true; }` with the comment "promoteToPrimary promotes the DESTINATION into the primary DICOM hierarchy". `src/ops/get_image_set.rs` reports `is_primary: Some(image_set.is_primary)`. Tests: `tests/all/image_set_copy.rs` exercises `force`/`promoteToPrimary` only as flags on the destination record (see the `copy_image_set` helper and the destination assertions); no test asserts a new primary image set id is returned, so the documented behavior is uncovered.

## Steps to Reproduce

1. Create a data store (POST /datastore) and make two image sets exist in it (today only reachable by seeding state, since a COMPLETED import creates none — see the open StartDICOMImportJob issue).
2. POST /datastore/{datastoreId}/imageSet/{sourceImageSetId}/copyImageSet?promoteToPrimary=true with a body naming `destinationImageSet` = {imageSetId: <second image set>, latestVersionId: <its latest>}.
3. Observe `destinationImageSetProperties.imageSetId` equals the caller-supplied destination id (its `latestVersionId` incremented) rather than a newly minted primary image set id.
4. Repeat the copy WITHOUT `promoteToPrimary` and no `destinationImageSet`, then POST .../getImageSet on the minted destination: `isPrimary` is reported `false` for a copy of a primary image set.
