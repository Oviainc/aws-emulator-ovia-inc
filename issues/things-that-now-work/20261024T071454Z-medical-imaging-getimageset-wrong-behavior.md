Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# GetImageSet never reports lastAccessedAt: the field is hardcoded absent and no image set ever carries one

_Filed: 2026-10-24T07:14:54Z_
_Operation: GetImageSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetImageSetOutput.lastAccessedAt` (modeled `Date`, "When the image set was last accessed") is hardcoded to `None` in the handler, and the `ImageSet` state record has no last-accessed field at all, so the value can never be produced by any path.

Expected: an image set reports a `lastAccessedAt` instant — AWS HealthImaging returns it alongside `storageTier`, and the service maintains it on data access (GetImageFrame / GetImageSetMetadata) with the image set's creation/import/copy instant as its initial value.
Actual: `lastAccessedAt` is omitted from every GetImageSet response, so a client that reads it (e.g. to reason about storage tiering) always sees nothing, and there is no sequence of calls that makes it appear.

The handler's comment justifies not having a *reader* write the field, but that only argues against updating it on GetImageSet itself — it leaves the member permanently unpopulated even for the instants the spec clearly owes it (when the image set was created/imported/copied, and when its pixel data or metadata was actually fetched, which the service documents as access and is therefore service-mandated state, not a read inventing a write).

## Evidence

Spec: `smithy_query(query="operation", operation="GetImageSet")` — output member `lastAccessedAt` (target `Date`), documented "When the image set was last accessed."; `storageTier` is modeled beside it, and the tiering feature these two members belong to is driven by access time.
Handler: src/ops/get_image_set.rs lines 211-214 — `last_accessed_at: None` with the comment "Absent, deliberately".
State: src/state.rs `ImageSet` (lines 861-887) and `ImageSetVersion` carry `created_at`, `deleted_at`, `updated_at` but no last-accessed instant; `GetImageFrame` (src/ops/get_image_frame.rs) records nothing when it serves pixel data.
Tests: no test in tests/all/ mentions `lastAccessedAt` (grep over tests/ finds no occurrence), so no assertion pins the field absent.

## Steps to Reproduce

1. Create a data store and obtain an image set in it (today only possible by seeding, see the StartDICOMImportJob issue)
2. POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSet — response contains no lastAccessedAt member
3. POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageFrame for a frame the image set holds, then repeat the getImageSet call — lastAccessedAt is still absent instead of reporting the access instant
