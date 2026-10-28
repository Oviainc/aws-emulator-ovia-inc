Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# CopyImageSet answers ConflictException for a DELETED source or destination image set instead of the crate's 404 tombstone answer

_Filed: 2026-10-28T05:27:40Z_
_Operation: CopyImageSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CopyImageSet` looks its source and destination image sets up with a plain `image_sets.get(..)` and never asks `ImageSet::is_deleted()`. A retired image set therefore falls through to the state check and is reported as 409 `ConflictException` (IMAGE_SET_NOT_ACTIVE_MESSAGE) — i.e. as a resource that exists but is busy — while every other handler in this crate that touches an image set's CONTENT answers 404 `ResourceNotFoundException` worded by the image-set kind for exactly the same record, each one citing the others as the reason. `CopyImageSet` reads the source's pixel data and DICOM attributes (copy_image_set.rs:550-566), so it is such a content path; the file never mentions a deleted image set at all (no occurrence of `deleted`/`DELETED`/`tombstone` in src/ops/copy_image_set.rs).

## Evidence

The copy's lookups, with no tombstone rule — `src/ops/copy_image_set.rs:525-544`:
```
        let source = datastore
            .image_sets
            .get(&input.source_image_set_id)
            .ok_or_else(|| { ... IMAGE_SET_NOT_FOUND_MESSAGE ... })?;
...
        if source.latest_version.image_set_state != ImageSetState::Active {
            return Err(CopyImageSetErrorModeled::ConflictException(ConflictException {
                message: crate::state::IMAGE_SET_NOT_ACTIVE_MESSAGE.to_string(),
```
and the same for the destination at `src/ops/copy_image_set.rs:588-593`.
The rule the siblings apply — `src/ops/get_image_frame.rs:187-198`:
```
        // A DELETED image set's pixel data went with it, so what is absent is the image
        // set itself, not one frame of it — decided by the one helper that owns the rule
        // ([`crate::state::ImageSet::is_deleted`]), as the sibling `GetImageSetMetadata`
        // decides the identical question.
        if image_set.is_deleted() {
            return Err(GetImageFrameErrorModeled::ResourceNotFoundException(
```
`src/ops/get_image_set_metadata.rs:401-410` and `src/ops/update_image_set_metadata.rs:790-799` repeat it verbatim ("the same answer `GetImageSetMetadata` gives").
The crate states it as a shared rule in its own test suite — `tests/all/content_reads_shared_rules.rs:9-11`:
```
//! * an image set whose latest version is `DELETED` holds no content at all, so either
//!   read answers 404 `ResourceNotFoundException` worded by the IMAGE SET kind — the
//!   whole resource is what is absent, not the frame or the attribute asked for;
```
No test pins the copy's answer for a deleted source or destination: `tests/all/image_set_copy.rs` contains no `deleted`/`Deleted` occurrence.

## Steps to Reproduce

1. Create a data store, let it settle to ACTIVE, and seed an image set with DICOM content as the crate's tests do.
2. `DeleteImageSet` that image set and wait for IMAGE_SET_DELETION_DELAY so its latest version is DELETED.
3. `POST /datastore/{id}/imageSet/{deleted}/copyImageSet` with `sourceImageSet.latestVersionId` equal to the version `GetImageSet` reports.
4. Observe 409 `ConflictException` ("image set is not ACTIVE"); `GetImageFrame` and `GetImageSetMetadata` on the same record answer 404 `ResourceNotFoundException` with IMAGE_SET_NOT_FOUND_MESSAGE.
5. Repeat naming the deleted image set as `copyImageSetInformation.destinationImageSet` — also 409 instead of 404.
