Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# A COMPLETED DICOM import job creates no image sets, so no image set can ever exist over the HTTP surface

_Filed: 2026-10-24T07:14:54Z_
_Operation: StartDICOMImportJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartDICOMImportJob` persists a job record and the clock marches it SUBMITTED -> IN_PROGRESS -> COMPLETED, but a completed job never materialises the resources an import produces: no image set is inserted into the data store, no image frames (pixel data) are stored, and no DICOM metadata is recorded.

In AWS HealthImaging a DICOM import is the ONLY way image sets come into existence: a job that reaches COMPLETED leaves one or more image sets in the data store (workflow status IMPORTED/CREATED, version 1), each holding its image frames and its DICOM metadata, discoverable through SearchImageSets / GetImageSet / GetImageSetMetadata / GetImageFrame.

Consequence for an emulator client: after CreateDatastore -> StartDICOMImportJob -> GetDICOMImportJob reports COMPLETED, `GetImageSet` and `GetImageFrame` answer 404 for every identifier and `CopyImageSet` can never be called because it requires an existing source image set. The entire image-set half of this service is unreachable through the HTTP front door — the only reason the crate's own test suites can exercise `GetImageFrame` and `CopyImageSet` is that they reach into `crate::state` in-process and insert `ImageSet` / `ImageFrame` records by hand.

Expected: when the import job's completion falls due, the data store gains at least one image set (with image frames readable by GetImageFrame and a stable set of image-set identifiers a client can discover), derived from what the job was asked to import.
Actual: the job's only observable effect is its own status/endedAt; `datastore.image_sets` stays empty forever.

This is distinct from the already-open event-propagation issue on this operation (`20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation`), which asks for S3 input reading, the output manifest, and a reachable FAILED status. This issue is about the data store gaining its primary resource — the image set — on a successful import, which is what every downstream image-set operation depends on.

## Evidence

Spec: `smithy_query(query="operation", operation="StartDICOMImportJob")` — the operation's doc (reproduced in the handler header, src/ops/start_dicom_import_job.rs line 27) is "Start importing bulk data into an ACTIVE data store. The import job imports DICOM P10 files found in the S3 prefix specified by the inputS3Uri parameter"; `ImageSetWorkflowStatus` declares IMPORTING/IMPORTED, statuses only an import can produce, and `ImageSetProperties`/`GetImageSet` model an image set that an import must have created. Well-established HealthImaging behavior: a completed import job yields image sets in the target data store.
Handler: src/ops/start_dicom_import_job.rs lines 225-258 builds an `ImportJob` and inserts it into `datastore.import_jobs`; nothing touches `datastore.image_sets`.
State: src/state.rs `ImportJob::advance_to` (lines 625-637) only moves the job status and sets `ended_at`; `Datastore::advance_to` (lines 1039-1067) advances jobs and image sets but never creates an image set. The only `image_sets.insert` in the whole crate is in src/ops/copy_image_set.rs line 401, which needs a source image set that already exists.
Tests: tests/all/datastore_lifecycle.rs lines 71-75 state it outright — the shared engine handle is held so a suite can "seed a resource this service's implemented operations cannot yet mint themselves (an image set, which only a DICOM import creates)"; tests/all/image_frames.rs and tests/all/image_set_copy.rs both seed `ImageSet::new` directly through `crate::state`. No test asserts that an import produces an image set, because none does.

## Steps to Reproduce

1. POST /datastore with a clientToken to create a data store and wait until GetDatastore reports ACTIVE
2. POST /startDICOMImportJob/datastore/{datastoreId} with dataAccessRoleArn, inputS3Uri and outputS3Uri
3. Poll GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId} until jobStatus is COMPLETED
4. POST /datastore/{datastoreId}/imageSet/{anyImageSetId}/getImageSet — every identifier answers 404 ResourceNotFoundException, and there is no API call that can enumerate an image set the import should have created
5. POST /datastore/{datastoreId}/imageSet/{anyImageSetId}/copyImageSet — fails 404 for lack of a source image set, so the copy workflow is unreachable too
