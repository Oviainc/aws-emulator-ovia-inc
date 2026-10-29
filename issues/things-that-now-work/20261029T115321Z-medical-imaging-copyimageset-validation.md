Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: da4330c5500325c8e5ff90f3211b2833b72817eb

# copiableAttributes accepts a JSON scalar or array and silently copies nothing, where the sibling metadata document is refused

_Filed: 2026-10-29T11:53:21Z_
_Operation: CopyImageSet | Class: validation_

> _Note: this issue was identified against the code base at commit `da4330c5500325c8e5ff90f3211b2833b72817eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CopyImageSet` reads `copyImageSetInformation.sourceImageSet.DICOMCopies.copiableAttributes` with `serde_json::from_str::<Value>` and accepts ANY successfully parsed JSON value — a number, a string, a boolean, `null`, an array — not just a document. For any of those, `selected_instances()` navigates `Study` -> `Series` -> `Instances` on a non-object, finds nothing, and returns an EMPTY selection. The empty selection is then used as the real subset selector: `frames_of_instances(&source.image_frames, selected)` yields no frames and `attributes_of_instances(source.attributes(), selected)` strips every SOP instance, so the call is ADMITTED (HTTP 200, source `COPYING_WITH_READ_ONLY_ACCESS`, destination `COPYING` -> `COPIED`) and the destination it mints ends up holding none of the source's pixel data and none of its instance-level attributes.

That turns a request the service cannot evaluate into a success whose only visible effect is a stripped, empty copy — indistinguishable on the wire from the legitimate case the handler documents (`a document that names none selects none`). This crate's OTHER handler for the very same kind of value draws the line at an object: `UpdateImageSetMetadata::parse_document` refuses a parsed-but-non-object blob with `ValidationException` (400) and the message "Member must be a JSON object of DICOM attributes", keeping "Member must be a JSON document" for text that does not parse at all. Both members carry, by this crate's own description, "a document written in the shape of the aggregated metadata document", so one of the two paths is applying a check the other skips.

## Evidence

The copy handler accepts any parsed JSON value and derives the subset from it:

- `src/ops/copy_image_set.rs:475-492`: `let selected = match &information.source_image_set.dicom_copies { Some(copies) => match ::serde_json::from_str::<Value>(&copies.copiable_attributes) { Ok(document) => Some(selected_instances(&document)), Err(_) => { return Err(CopyImageSetErrorModeled::ValidationException(... "Member must be a JSON document" ...)) } }, None => None };` — only a PARSE failure is refused.
- `src/ops/copy_image_set.rs:185-198` (`selected_instances`): `let series = document.get(METADATA_STUDY_KEY).and_then(|study| study.get(METADATA_SERIES_KEY)).and_then(Value::as_object);` ... `selected` — a non-object document yields an empty set.
- `src/ops/copy_image_set.rs:599-611`: `let copied_image_frames = match &selected { Some(selected) => frames_of_instances(&source.image_frames, selected), None => source.image_frames.clone() };` and `let copied_attributes = match &selected { Some(selected) => attributes_of_instances(source.attributes(), selected), None => source.attributes() };` — the empty set is honoured as a real selector.

The sibling handler in the SAME crate refuses exactly that input, and says why:

- `src/ops/update_image_set_metadata.rs:650-666`: "Both members are documented as \"the DICOM tags\" to update or remove, and a DICOM metadata document is the object `GetImageSetMetadata` serves — so a blob that is not a JSON object names no attribute at all and fails the constraint the service sets." -> `match ::serde_json::from_slice::<Value>(bytes) { Ok(Value::Object(document)) => Ok(Some(document)), Ok(_) => Err(refuse(path, "Member must be a JSON object of DICOM attributes")), Err(_) => Err(refuse(path, "Member must be a JSON document")), }`

The copy handler's own doc states the same premise for `copiableAttributes` — that the value is a DOCUMENT in that shape:

- `src/ops/copy_image_set.rs:43-52`: "`sourceImageSet.DICOMCopies.copiableAttributes` is documented as \"the JSON string used to specify a subset of SOP Instances to copy\", so a value that is not a JSON document fails the constraint the service sets and is refused with `ValidationException` (400) ... A value that IS one SELECTS the content the copy carries: the document is written in the shape of the aggregated metadata document, which places a SOP instance at `Study` -> `Series` -> series UID -> `Instances` -> SOP Instance UID"

Model side (nothing constrains the value's SHAPE, which is why the handler owns the check): `smithy_query(query="shape",shape="MetadataCopies")` -> `copiableAttributes`: `{"required": true, "length": {"min": 1, "max": 260000}, "sensitive": true, "doc": "The JSON string used to specify a subset of SOP Instances to copy from source to destination image set."}`.

Test coverage: `tests/all/image_set_copy_subsets.rs` drives subset copies and `tests/all/image_set_copy.rs` the whole-image-set copy; no case drives a `copiableAttributes` payload that parses as a non-object, so nothing pins today's silent-empty-copy answer.

## Steps to Reproduce

1. CreateDatastore and let it settle into ACTIVE.
2. Seed (or otherwise obtain) an image set in that data store holding image frames whose `sop_instance_uid` is set and some instance-level attributes, so a subset copy has something to select.
3. Call CopyImageSet with `copyImageSetInformation.sourceImageSet.latestVersionId` = the source's latest version and `copyImageSetInformation.sourceImageSet.DICOMCopies.copiableAttributes` = "123" (or "[]", or "\"abc\"") — all valid JSON, none an object.
4. Observe HTTP 200: the copy is admitted, a destination image set is minted and, once the clock lands the copy (IMAGE_SET_COPY_DELAY), GetImageFrame against the destination finds no frame and GetImageSetMetadata reports no instances — the selector the service could not understand was honoured as "select nothing".
5. Contrast with UpdateImageSetMetadata on the same image set with `updateImageSetMetadataUpdates.DICOMUpdates.updatableAttributes` = `123`: HTTP 400 ValidationException, "...updatableAttributes failed to satisfy constraint: Member must be a JSON object of DICOM attributes".
