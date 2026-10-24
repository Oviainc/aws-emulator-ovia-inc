Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CopyImageSet discards DICOMCopies.copiableAttributes: the copied content and the requested subset are never recorded

_Filed: 2026-10-24T05:11:11Z_
_Operation: CopyImageSet | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CopyImageSet` is modeled as copying one image set's DICOM content into a destination image set, and `copyImageSetInformation.sourceImageSet.DICOMCopies.copiableAttributes` is documented as "the JSON string used to specify a subset of SOP Instances to copy from source to destination image set".

Expected: the destination version records what was copied — derived from the source image set and, when `DICOMCopies.copiableAttributes` is supplied, restricted to the subset it names — so a subsequent read of the destination (`GetImageSetMetadata`, and the instance/frame-bearing reads) reflects the copied instances and differs between a full copy and a subset copy.

Actual: the handler parses `copiableAttributes` only to assert it is a JSON document and then drops it; the value is never stored anywhere in state. The destination version that the copy pushes/mints carries nothing but `imageSetState`, `imageSetWorkflowStatus`, `Overrides.forced` and `updatedAt`, and no reference to the source image set or its content. Consequences a client can observe:

1. A subset copy and a full copy of the same source produce byte-identical observable state — `copiableAttributes` has no effect on anything the emulator will ever report.
2. An image set minted by `CopyImageSet` has no copied content and no linkage to its source, so once `GetImageSetMetadata` is implemented it has nothing to report for a copied image set.
3. The `force` flag's modeled purpose ("force completion … even if there are inconsistent Patient, Study, and/or Series level metadata elements between the sourceImageSet and destinationImageSet") can never make a difference either, because no Patient/Study/Series metadata exists to be inconsistent; the flag is only echoed back as `Overrides.forced`.

## Evidence

Spec: `smithy_query(query="shape", shape="MetadataCopies")` — `copiableAttributes` is `@required` inside `MetadataCopies`, length 1..260000, `@sensitive`, documented as the JSON string selecting a subset of SOP Instances to copy; `smithy_query(query="shape", shape="CopySourceImageSetInformation")` reaches it via `DICOMCopies`. Well-known HealthImaging behavior: a copy materialises the source's DICOM metadata/instances (optionally a subset) into the destination image set.
Handler: `src/ops/copy_image_set.rs` lines 229-240 — `serde_json::from_str::<serde_json::Value>(&copies.copiable_attributes)` is the ONLY use of the member, used purely for the 400 refusal; lines 377-414 then mint/`push_version` the destination from `ImageSet::new` / `ImageSet::push_version` with only state, workflow status, `forced` and `now`. `src/state.rs` `ImageSetVersion` (lines ~591-607) has no member for copied content, a source reference, or the copy's attribute selection, so there is nowhere for it to be persisted.
Tests: `tests/all/image_set_copy.rs` (the `copiableAttributes` table around lines 836-901) asserts only that a non-JSON value is refused and that every JSON shape is accepted; the success cases assert ids, versions, state, workflow status, `overrides.forced`, `isPrimary`, `storageTier` — no case asserts anything about copied content or about a subset copy differing from a full copy. Uncovered.

## Steps to Reproduce

1. CreateDatastore and wait for ACTIVE
2. Obtain a source image set in that data store (today only possible by seeding `state`, since the import job creates none)
3. POST /datastore/{datastoreId}/imageSet/{sourceImageSetId}/copyImageSet with body {"sourceImageSet": {"latestVersionId": "1", "DICOMCopies": {"copiableAttributes": "{\"SchemaVersion\":\"1.1\",\"Study\":{\"Series\":{\"<seriesUid>\":{\"Instances\":{\"<sopUid>\":{}}}}}}"}}} → 200
4. Repeat the same copy without `DICOMCopies` (full copy) against a second destination
5. Read both destinations back (GetImageSet, and GetImageSetMetadata once implemented) and observe the two destinations are indistinguishable: the subset selection was never stored and no content was copied
