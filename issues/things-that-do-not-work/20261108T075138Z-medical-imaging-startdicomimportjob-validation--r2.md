Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0ce8acb2d50b38f212ecb6be47b8b7720e8c1b4

# The DICOM import writes DICOMTags straight from a P10 file without the modelled @length/@pattern/@range the sibling metadata-update writer applies

_Filed: 2026-11-08T07:51:38Z_
_Operation: StartDICOMImportJob | Class: validation_

> _Note: this issue was identified against the code base at commit `d0ce8acb2d50b38f212ecb6be47b8b7720e8c1b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly two writers of the modelled `DICOMTags` members: `UpdateImageSetMetadata` (through `ModelledAttribute::unmet_requirement`) and the DICOM import (`crate::state::dicom_import::read_part10`, reached from the `StartDICOMImportJob` handler).

The update writer states and enforces the rule that a value may only be persisted into one of those members when it satisfies every constraint the model declares on that member — kind, `@length`, `@pattern`, `@range` — because the document arrives in a blob the boundary validator does not walk, and its crate test spells out why: "without them a search summary and the served metadata document would report values the model forbids."

The import path skips that rule entirely. `read_part10` assigns each `DICOMTags` member from whatever the file carried (`text(&object, tags::STUDY_INSTANCE_UID)`, `text(&object, tags::STUDY_DESCRIPTION)`, `number(&object, tags::SERIES_NUMBER)`, …), `imported_image_sets` packs them into `ImageSetAttributes`, and the `StartDICOMImportJob` handler hands them to the clock as `delivered_attributes`, which merges them onto the image set verbatim (`ImageSet::merge_attributes`). No length, pattern or range check runs anywhere on that path, and neither `merge_dicom_tags` nor `set_attributes` applies one.

So an S3 object that is a readable Part 10 file but carries a value outside a member's modelled constraint — e.g. a `StudyInstanceUID` (0020,000D) holding `1.2.3.ABC` against `@pattern ^[0-9.]+$`, or a `StudyDescription` (0008,1030) longer than 512 characters against `@length max 512` — is persisted as that image set's modelled attribute, and from then on `SearchImageSets` (`ImageSetsMetadataSummary.DICOMTags`) and `GetImageSetMetadata` report a value the model forbids: exactly the outcome the crate's own rule for the other writer exists to prevent. The two writers of one stored member disagree about what may be stored in it.

## Evidence

Smithy (the constraints the stored members carry, `smithy_query(query="shape",shape="DICOMTags")`):
  "DICOMStudyInstanceUID": {"length": {"min": 0, "max": 512}, "pattern": "^[0-9.]+$"}
  "DICOMStudyDescription": {"length": {"min": 0, "max": 512}}
  "DICOMSeriesNumber": {"range": {"min": -2147483648, "max": 2147483647}}
and `ImageSetsMetadataSummary` carries `"DICOMTags": {"target": "DICOMTags", "doc": "<p>The DICOM tags associated with the image set.</p>"}` — i.e. these are the members the search result reports.

Side A — the rule, as the sibling writer states and enforces it:
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/update_image_set_metadata.rs:32-36
  "//!   member, so what `SearchImageSets` filters on and what the metadata document reports
   //!   never disagree. Only a value that member can legally carry is written: the document
   //!   arrives in a blob the boundary validator does not walk, so every constraint the model
   //!   declares on the member it is destined for — its kind, its `@length`, its `@pattern`,
   //!   its `@range` ([`ModelledAttribute::unmet_requirement`]) — is applied here instead, and
   //!   a value that fails one is refused with `ValidationException` (400) rather than persisted."
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/update_image_set_metadata.rs:852-859
  "Some(modelled) => {
       if let Some(requirement) = modelled.unmet_requirement(&value) {
           let keyword = at.last().cloned().unwrap_or_default();
           return Err(refuse(path, &format!(\"{requirement} for {keyword}\")));
       }
       modelled.write(&mut attributes.dicom_tags, Some(&value));"
and the test that pins it, services/service-com-amazonaws-medicalimaging-ahigatewayservice/tests/all/modelled_attribute_constraints.rs:5-11
  "//! RULE: an attribute an update document names for a member the model declares is written
   //! only when its value satisfies every constraint that member's shape declares — the kind
   //! it holds, its `@length`, its `@pattern`, its `@range` — and a value failing one is
   //! refused with `ValidationException` (HTTP 400) that persists nothing. The document
   //! arrives in a blob, which the boundary validator does not walk, so the handler is where
   //! those constraints reach it; without them a search summary and the served metadata
   //! document would report values the model forbids."

Side B — the import writer, with no such check:
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state/dicom_import.rs:259-273
  "let tags = DICOMTags {
       dicom_patient_id: text(&object, tags::PATIENT_ID),
       ...
       dicom_study_instance_uid: text(&object, tags::STUDY_INSTANCE_UID),
       dicom_study_id: text(&object, tags::STUDY_ID),
       dicom_study_description: text(&object, tags::STUDY_DESCRIPTION),
       ...
       dicom_series_number: number(&object, tags::SERIES_NUMBER),"
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state/dicom_import.rs:298-300 (the whole of what a value passes through)
  "fn text(object: &::dicom_object::DefaultDicomObject, tag: ::dicom_core::Tag) -> Option<String> {
       trimmed(&object.get(tag)?.value().to_str().ok()?)
   }"
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/start_dicom_import_job.rs:355-358 and 450-455 (the handler persisting them unchecked)
  "let imported = { let mut mint = || store.mint_id(); crate::state::imported_image_sets(&instances, &mut mint) };"
  "image_set.pending = Some(PendingImageSetTransition { ... delivered_attributes: Some(one.attributes), ... });"
and services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:1708-1712 merges them verbatim (`merge_attributes` → `set_attributes`), with src/state.rs:1338 `merge_dicom_tags` doing a plain element-by-element assignment — no constraint is consulted on this path.

Test coverage: tests/all/dicom_part10_reading.rs covers only WHETHER bytes are a readable Part 10 file and how instances group into image sets (its header lists the rows as "HOW the bytes fail to be a Part 10 file" / "how the instances are spread over studies and series"); no row covers a readable file whose attribute value breaks a modelled constraint.

## Steps to Reproduce

1. POST /datastore with a clientToken and poll GET /datastore/{datastoreId} until datastoreStatus is ACTIVE.
2. Create an S3 bucket and PUT one readable DICOM Part 10 object under in/ whose StudyInstanceUID (0020,000D) is `1.2.3.ABC` (or whose StudyDescription (0008,1030) is 600 characters long), with a SeriesInstanceUID so the instance groups into an image set.
3. Create an IAM role medical-imaging.amazonaws.com can assume, then POST /startDICOMImportJob/datastore/{datastoreId} with inputS3Uri=s3://<bucket>/in/, outputS3Uri=s3://<bucket>/out/.
4. Poll GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId} until jobStatus is COMPLETED.
5. POST /datastore/{datastoreId}/searchImageSets with no filters and read the summary's DICOMTags; also POST .../getImageSetMetadata and inspect Study.DICOM.StudyInstanceUID / StudyDescription.
6. Observed: the stored and reported value is the file's, violating DICOMStudyInstanceUID's `@pattern ^[0-9.]+$` (resp. DICOMStudyDescription's `@length max 512`) — while the same value sent through POST .../updateImageSetMetadata is refused with ValidationException by ModelledAttribute::unmet_requirement and persists nothing.
