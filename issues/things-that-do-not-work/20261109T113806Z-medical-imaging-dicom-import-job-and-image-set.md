Issue-Source: trace-sweep-divergence
Issue-Trace: medical-imaging-dicom-import-job-yields-searchable-image-set
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medical-imaging-dicom-import-job-yields-searchable-image-set
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging|StartDICOMImportJob|2xx->2xx|$.submittedAt
Issue-Key: medical-imaging|GetDICOMImportJob|2xx->2xx|$.jobProperties.endedAt
Issue-Key: medical-imaging|GetDICOMImportJob|2xx->2xx|$.jobProperties.outputS3Uri
Issue-Key: medical-imaging|GetDICOMImportJob|2xx->2xx|$.jobProperties.submittedAt
Issue-Key: medical-imaging|SearchImageSets|2xx->2xx|$.imageSetsMetadataSummaries[*].DICOMTags.DICOMNumberOfStudyRelatedSeries
Issue-Key: medical-imaging|SearchImageSets|2xx->2xx|$.imageSetsMetadataSummaries[*].createdAt
Issue-Key: medical-imaging|SearchImageSets|2xx->2xx|$.imageSetsMetadataSummaries[*].lastAccessedAt
Issue-Key: medical-imaging|SearchImageSets|2xx->2xx|$.imageSetsMetadataSummaries[*].updatedAt
Issue-Key: medical-imaging|GetImageSet|2xx->2xx|$.createdAt
Issue-Key: medical-imaging|GetImageSet|2xx->2xx|$.lastAccessedAt
Issue-Key: medical-imaging|GetImageSet|2xx->2xx|$.overrides
Issue-Key: medical-imaging|GetImageSet|2xx->2xx|$.updatedAt
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.DICOM.SeriesNumber
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.ChecksumType
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.BitsAllocated
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.BitsStored
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.Columns
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.FileMetaInformationGroupLength
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.FileMetaInformationVersion
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.HighBit
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.ImplementationClassUID
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.InstanceNumber
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.MediaStorageSOPClassUID
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.MediaStorageSOPInstanceUID
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.PhotometricInterpretation
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.PixelRepresentation
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.Rows
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.SOPClassUID
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.SamplesPerPixel
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOM.TransferSyntaxUID
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.DICOMVRs
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.ImageFrames[*].FrameSizeInBytes
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.ImageFrames[*].MaxPixelValue
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.ImageFrames[*].MinPixelValue
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.ImageFrames[*].PixelDataChecksumFromBaseToFullResolution
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$.Study.Series.1.2.826.0.1.3680043.10.511.1.3.Instances.1.2.826.0.1.3680043.10.511.1.1.StoredTransferSyntaxUID
Issue-Key: medical-imaging|GetImageSetMetadata|2xx->2xx|$headers.content-type
Issue-Key: medical-imaging|GetImageSet|4xx->2xx|$
Issue-Key: medical-imaging|GetImageSet|4xx->2xx|status
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# medical-imaging DICOM import job and image set lifecycle responses diverge from AWS on six points

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImageSet

## Description
Replaying the DICOM-import-to-searchable-image-set fixture against the emulator leaves 6 of its steps diverging, all on medical-imaging. The import job itself now runs and does index an image set, but every response along the lifecycle (job submit, job get, search, image set get, metadata get, post-delete get) carries a different shape or state than AWS. This issue bundles SIX distinct causes; all six must be closed before submitting.

Cause 1 of 6 — every modelled `Date` member is serialized in a form that does not match AWS's timestamp wire shape (StartDICOMImportJob.submittedAt, GetDICOMImportJob.jobProperties.submittedAt/endedAt, SearchImageSets summary createdAt/updatedAt/lastAccessedAt, GetImageSet createdAt/updatedAt/lastAccessedAt).
  AWS: Smithy: StartDICOMImportJobResponse.submittedAt, DICOMImportJobProperties.submittedAt/endedAt, ImageSetsMetadataSummary.createdAt/updatedAt/lastAccessedAt and GetImageSetResponse.createdAt/updatedAt/deletedAt/lastAccessedAt all target com.amazonaws.medicalimaging#Date (type timestamp), protocol restJson1. The model documents the instant form it serves for these members: ImageSetsMetadataSummary.createdAt — "The time an image set is created. Sample creation date is provided in 1985-04-12T23:20:50.52Z format", i.e. an instant carrying at most millisecond precision.
  Emulator: emits a bare f64 epoch value at full float precision for every one of those members (e.g. submittedAt 1794223902.9356477, endedAt/updatedAt/lastAccessedAt 1794223906.9356477). The replay comparator asserts these members by SHAPE, not value (expected '<shaped-not-asserted>'), and the shape assertion fails on all of them, so the divergence is the serialized form of the timestamp and not the particular clock reading.
  Why: a client that round-trips a HealthImaging timestamp (deserializing it, re-sending it in a filter, logging or diffing it) gets a value of the wrong shape/precision from the emulator, so timestamp-typed comparisons and any assertion on a job's or image set's time fields behave differently than against AWS.
  Hypothesis: the handlers hand the state clock's raw f64 to the JSON writer instead of routing Date members through the service's timestamp serializer.
  Fix: the medical-imaging response serializer for the `Date` shape, applied uniformly across the four operations (not patched per operation).
  Traces: task#7 (StartDICOMImportJob.submittedAt), task#8 (jobProperties.submittedAt, jobProperties.endedAt), task#10 (imageSetsMetadataSummaries[0].createdAt/updatedAt/lastAccessedAt), task#11 (createdAt/updatedAt/lastAccessedAt).

Cause 2 of 6 — GetDICOMImportJob echoes the request's outputS3Uri instead of the job's own output prefix.
  AWS: Smithy: DICOMImportJobProperties.outputS3Uri (S3Uri, required) — "The output prefix of the S3 bucket to upload the results of the DICOM import job." Recorded behavior: AWS returns the caller-supplied output prefix with the job-specific sub-prefix appended, formed as `<datastoreId>-DicomImport-<jobId>/` under that prefix; the recorded response for this step is `s3://<bucket>/out/<datastoreId>-DicomImport-<jobId>/`. The bucket, the `out/` prefix and the two ids are values taken from the request and from the job's own identity — the requirement is the appended `<datastoreId>-DicomImport-<jobId>/` component, not the recorded literal.
  Emulator: returns the request's outputS3Uri verbatim (`s3://<bucket>/out/`), with no per-job component.
  Why: a client uses jobProperties.outputS3Uri to locate that job's SUCCESS/FAILURE/WARNING manifests. Against the emulator every job reports the same prefix, so a client reads another job's results or finds none, and cannot distinguish two jobs writing under one output prefix.
  Fix: the GetDICOMImportJob handler / the import job state that records a job's output prefix at submit time.
  Traces: task#8. Related open issue 20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md covers the job not reading/writing S3 at all; it does not cover this field's value.

Cause 3 of 6 — SearchImageSets reports DICOMNumberOfStudyRelatedSeries from the indexed series count instead of the study attribute.
  AWS: Smithy: DICOMTags.DICOMNumberOfStudyRelatedSeries (integer, @default 0, @range min 0 max 1000000) — "The total number of series in the DICOM study." Recorded behavior only: for an image set imported by this job AWS reports 0 in the search summary, i.e. the modelled default, even though the image set contains one series.
  Emulator: reports 1 — it counts the series it indexed for the image set.
  Why: a client that filters or displays study-related series counts reads a value the real service does not produce, so search-result dashboards and any equality check on the tag diverge.
  Hypothesis: AWS populates the study-related counts from the Study-level DICOM attributes present in the imported P10 (absent here, hence the default 0) rather than by counting indexed series.
  Fix: the SearchImageSets summary projection / the DICOMTags derivation in the import indexer.
  Traces: task#10. Related open issue 20261108T075138Z-medical-imaging-startdicomimportjob-validation--r2.md concerns @length/@pattern/@range enforcement on DICOMTags, not how this count is derived.

Cause 4 of 6 — GetImageSet omits the `overrides` structure.
  AWS: Smithy: GetImageSetResponse.overrides targets Overrides { forced: Boolean } — "Specifies the overrides used in image set modification calls to CopyImageSet and UpdateImageSetMetadata." Recorded behavior: AWS returns `overrides` present with `forced` false for an image set that was never force-modified.
  Emulator: the member is absent from the response body entirely.
  Why: a client reading imageSet.overrides.forced to decide whether an image set was force-completed gets a null/absent structure from the emulator and either faults or silently treats the flag as unknown.
  Fix: the GetImageSet handler/serializer plus the image set state that must carry the overrides record from import and from CopyImageSet/UpdateImageSetMetadata.
  Traces: task#11.

Cause 5 of 6 — the GetImageSetMetadata blob stops at the Series level: instance-level metadata, image-frame records and VR map are missing, SeriesNumber has the wrong type, and the response's Content-Type diverges.
  AWS: Smithy: GetImageSetMetadataResponse.imageSetMetadataBlob (@httpPayload, @required, streaming blob) — "The blob containing the aggregated metadata information for the image set" — with contentType bound to the Content-Type header and contentEncoding bound to Content-Encoding. Recorded behavior: the aggregated document nests Study > Series > Instances, and each instance carries its DICOM header group (FileMetaInformationGroupLength, FileMetaInformationVersion, MediaStorageSOPClassUID, MediaStorageSOPInstanceUID, TransferSyntaxUID, ImplementationClassUID, SOPClassUID, InstanceNumber, SamplesPerPixel, PhotometricInterpretation, Rows, Columns, BitsAllocated, BitsStored, HighBit, PixelRepresentation), plus DICOMVRs, StoredTransferSyntaxUID and an ImageFrames entry (FrameSizeInBytes, MinPixelValue, MaxPixelValue, PixelDataChecksumFromBaseToFullResolution); Series-level SeriesNumber is typed as in the imported header.
  Emulator: serves a document whose Instances entries lack all of the above members (every instance-level DICOM member, DICOMVRs, StoredTransferSyntaxUID and ImageFrames are reported missing), types Series-level DICOM.SeriesNumber differently from the recording, and answers with a Content-Type other than the recorded one (the cluster key lists $headers.content-type among the diverging keys).
  Why: metadata consumers (viewers, de-identification and frame-fetch flows) read instance-level tags and ImageFrames out of this blob to issue GetImageFrame; against the emulator there is nothing to read, so the whole pixel-retrieval path is unusable even though the call returns 200.
  Hypothesis: the import indexer stores only Patient/Study/Series attributes, and the metadata writer does not set the modelled contentType/contentEncoding header bindings.
  Fix: the import-job metadata indexer (instance/frame level state) and the GetImageSetMetadata handler's payload serializer plus its Content-Type/Content-Encoding header bindings.
  Traces: task#12.

Cause 6 of 6 — a deleted image set is never reaped: GetImageSet keeps answering 200 instead of ResourceNotFoundException.
  AWS: Smithy: GetImageSet errors include ResourceNotFoundException (errorKind client, httpError 404, required `message`) — "The request references a resource which does not exist." Recorded behavior: after DeleteImageSet the image set first reports imageSetState DELETED and then becomes unretrievable, so GetImageSet answers HTTP 404 with error code ResourceNotFoundException; the fixture polls for exactly that code. The requirement is status 404 and code ResourceNotFoundException with AWS's not-found wording for an image set, whose variable parts are the requested imageSetId/datastoreId from the request; the recorded step asserts the code, not a message literal.
  Emulator: answers HTTP 200 forever with the DELETED image set body (imageSetState/imageSetWorkflowStatus DELETED, deletedAt set) — the poll ran 37 attempts and was capped at 180s without ever seeing an error code.
  Why: any client that waits for a deletion to finish by polling GetImageSet until ResourceNotFoundException hangs until its own timeout against the emulator, and teardown code that treats 200 as "still present" never completes.
  Fix: the image set state transition for DeleteImageSet (DELETED entries must stop being readable) and the GetImageSet handler's not-found path.
  Traces: teardown#14.

## Evidence
Fixture medical-imaging-dicom-import-job-yields-searchable-image-set, 38 steps, 31 passed / 7 diverged (score 81.6%); 6 of the 7 are medical-imaging (the ListObjectsV2 cluster at task#9 is already owned by 20261029T192819Z-s3-listobjectsv2-echoes-the-percent-encoded-prefix-instead.md).

task#7 StartDICOMImportJob, 200 -> 200: $.submittedAt value_mismatch (expected '<shaped-not-asserted>', got 1794223902.9356477).

task#8 GetDICOMImportJob, 200 -> 200: $.jobProperties.submittedAt and $.jobProperties.endedAt value_mismatch (expected '<shaped-not-asserted>', got 1794223902.9356477 / 1794223906.9356477); $.jobProperties.outputS3Uri value_mismatch — expected 's3://<bucket>/out/<datastoreId>-DicomImport-<jobId>/', got 's3://<bucket>/out/'.

task#10 SearchImageSets, 200 -> 200: $.imageSetsMetadataSummaries[0].DICOMTags.DICOMNumberOfStudyRelatedSeries expected 0, got 1; createdAt/updatedAt/lastAccessedAt value_mismatch against the shape assertion (got raw f64 epochs).

task#11 GetImageSet, 200 -> 200: $.overrides missing (expected {'forced': False}, got None); createdAt/updatedAt/lastAccessedAt value_mismatch against the shape assertion.

task#12 GetImageSetMetadata, 200 -> 200: $.Study.Series.<seriesUID>.DICOM.SeriesNumber type_mismatch; and under Instances.<instanceUID>: DICOM.FileMetaInformationGroupLength, FileMetaInformationVersion, MediaStorageSOPClassUID, MediaStorageSOPInstanceUID, TransferSyntaxUID, ImplementationClassUID, SOPClassUID, InstanceNumber, SamplesPerPixel, PhotometricInterpretation, Rows, Columns, BitsAllocated, BitsStored, HighBit, PixelRepresentation all missing, plus DICOMVRs, StoredTransferSyntaxUID, ChecksumType and ImageFrames[0].{FrameSizeInBytes,MinPixelValue,MaxPixelValue,PixelDataChecksumFromBaseToFullResolution} missing; the cluster key also lists $headers.content-type as diverging.

teardown#14 GetImageSet after DeleteImageSet: status expected 404, got 200; 'poll: error code None not in [ResourceNotFoundException] after 37 attempt(s)', poll capped at 180s, body returned each time was the image set with imageSetState DELETED, imageSetWorkflowStatus DELETED and deletedAt set.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture medical-imaging-dicom-import-job-yields-searchable-image-set (trace set simparity-wire-aws-v1).
2. Step task#7: StartDICOMImportJob against an ACTIVE datastore with inputS3Uri/outputS3Uri prefixes — inspect the serialized form of submittedAt.
3. Step task#8: GetDICOMImportJob for that jobId — compare jobProperties.outputS3Uri with the request's outputS3Uri (AWS appends <datastoreId>-DicomImport-<jobId>/) and the form of submittedAt/endedAt.
4. Step task#10: SearchImageSets on the datastore — read imageSetsMetadataSummaries[0].DICOMTags.DICOMNumberOfStudyRelatedSeries and the three timestamps.
5. Step task#11: GetImageSet for the indexed imageSetId — check that `overrides` is present with forced=false.
6. Step task#12: GetImageSetMetadata for the same image set — walk Study > Series > Instances and check instance-level DICOM members, DICOMVRs, StoredTransferSyntaxUID and ImageFrames, plus the response Content-Type/Content-Encoding headers.
7. Step teardown#14: DeleteImageSet, then poll GetImageSet — AWS reaches HTTP 404 ResourceNotFoundException; the emulator keeps returning 200 with the DELETED body until the 180s cap.
