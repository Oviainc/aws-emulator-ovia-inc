Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.e2e-ota-update-package-job-delivery
Issue-Key: iot.e2e-ota-update-package-job-delivery|run#36|KeyError|document = iot.get_job_document(jobId=job_id)["document"]
Issue-Commit: 0826f0e1d122bb3494a016eac0d0efbac2d6dd15

# CreateOTAUpdate deploys a job with no job document, so GetJobDocument on an OTA job answers 200 with no `document`

> _Note: this issue was identified against the code base at commit `0826f0e1d122bb3494a016eac0d0efbac2d6dd15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobDocument

## Description
One cause.

Cause 1 of 1: an OTA update's underlying `AFR_OTA-<otaUpdateId>` job is persisted with an empty job document, so the document a device would fetch never exists. Failing check: iot.e2e-ota-update-package-job-delivery.

AWS: `CreateOTAUpdate` is the only way to create an OTA job, and the OTA service GENERATES that job's document itself from the request's `files`/`protocols`/`awsJobPresignedUrlConfig` — the caller never supplies one (Smithy: CreateOTAUpdate input has `files` (required, len 1..), `protocols`, `awsJobPresignedUrlConfig`, `roleArn` (required), and NO `document`/`documentSource`/`documentParameters` member; model path services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json). Once the update reaches `otaUpdateStatus=CREATE_COMPLETE`, `GET /jobs/{jobId}/job-document` for the job named by the create response's `awsIotJobId` returns HTTP 200 with the output member `document` present and non-empty (Smithy: GetJobDocument output `document` -> JobDocument, @length 0..32768; errors InvalidRequestException 400, ResourceNotFoundException 404, ServiceUnavailableException 503, ThrottlingException 400). Recorded behavior only for the document's inner shape: it is a JSON object (the `afr_ota` OTA job document) whose file entry carries the targeted file — the file name/path and the download location derived from the request's `fileLocation.s3Location`, so the S3 object key the request named appears inside it. The same document is what the job's per-thing executions hand the device through the Jobs data plane.

Emulator: `GetJobDocument` for the OTA job returns HTTP 200 whose body has no `document` member at all — botocore raises `KeyError: 'document'` on `iot.get_job_document(jobId=job_id)["document"]`. This is by construction, not a lookup miss: `CreateOTAUpdate` builds its `RegisteredJob` with `document: None` / `document_source: None` and an empty `document_parameters`, and `GetJobDocument` then takes the `stored.is_none()` branch and returns `GetJobDocumentOutput { document: None }`. Because the same `None` is passed into `queue_job_executions(...)`, the per-thing executions the Jobs data plane serves carry no document either, so a device driven through `iot-jobs-data` is handed an execution with nothing to download. Everything else on the path behaves: the package version publishes, the update reaches `CREATE_COMPLETE`, the execution becomes queryable and drives to `SUCCEEDED`.

Why: a client relying on AWS's behavior cannot complete an OTA at all. Any OTA flow reads the generated document to learn which file to fetch and from where (`document["afr_ota"]["files"][...]`, the presigned `update_data_url`); against the emulator every such read fails with a missing key or an empty document, and a device agent polling the Jobs data plane receives a job execution with no payload — so a success report like the check's `UpdateJobExecution(SUCCEEDED)` is reported for an update that transferred nothing. A CREATE_COMPLETE OTA update whose job has no document is a state real AWS never produces.

Fix: `CreateOTAUpdate`'s handler plus the job-document construction it should call in this service's state module. The handler must synthesize the OTA job document from the request it already validates (its `files` entries with `fileName`/`fileVersion`/`fileLocation`, the requested `protocols`, and the presigned-URL config) and persist it on the `RegisteredJob` it inserts, passing that same document into the execution queueing so the data plane serves it; `GetJobDocument` itself needs no change once the record carries a document, and its "a job an OTA update deploys has no document here" module comment (which currently documents the defect as intended behavior) must go with it. `DeleteOTAUpdate` must keep removing the document with the job.

Traces: iot.e2e-ota-update-package-job-delivery — fails at `document = iot.get_job_document(jobId=job_id)["document"]` with `KeyError: 'document'` immediately after asserting `otaUpdateStatus == CREATE_COMPLETE`. Code citations: services/service-com-amazonaws-iot-awsiotservice/src/ops/create_ota_update.rs:400-416 (`document: None`, empty `document_parameters`) and :418-428 (`job.document.as_deref()` into `queue_job_executions`); services/service-com-amazonaws-iot-awsiotservice/src/ops/get_job_document.rs:168-184 (the `None` record yields `GetJobDocumentOutput { document: None }`) and :16-19 (the comment asserting an OTA-deployed job legitimately has no document).

## Evidence
Expected (real AWS): after CreateOTAUpdate(files=[{fileName: <key>, fileVersion: <version>, fileLocation: {s3Location: {bucket: <bucket>, key: <key>}}}], protocols=["HTTP"], targets=[<thingArn>], roleArn=<role>) reaches otaUpdateStatus=CREATE_COMPLETE, GetJobDocument(jobId=<awsIotJobId, i.e. AFR_OTA-<otaUpdateId>>) -> HTTP 200 with a `document` member holding a non-empty JSON object whose file entry references the requested S3 object key (the key is a value taken from the request, not a fixed string).

Actual (emulator): GetJobDocument(jobId=AFR_OTA-<otaUpdateId>) -> HTTP 200 with NO `document` member in the body. botocore:
  File "check_iot_e2e_ota_update_package_job_delivery.py", line 93, in run
    document = iot.get_job_document(jobId=job_id)["document"]
  KeyError: 'document'
The assertions that would have followed (`json.loads(document)` is a non-empty dict; the S3 key appears in the document) are therefore never reached. The stored job is created document-less on purpose: create_ota_update.rs:407 `document: None`, so the executions fanned out at create_ota_update.rs:422 also carry no document.

## Steps to Reproduce
1. Boot the emulator and point an `iot` client at it.
2. Create an S3 bucket and put an object (key `firmware.bin`); create an IAM role for `iot.amazonaws.com` with the OTA policy.
3. Enable `$package` named-shadow fleet indexing, then CreatePackage / CreatePackageVersion / UpdatePackageVersion(action=PUBLISH) and CreateThing.
4. CreateOTAUpdate(otaUpdateId=<id>, targets=[<thingArn>], targetSelection=SNAPSHOT, protocols=["HTTP"], files=[{fileName: "firmware.bin", fileVersion: "1.0.0", fileLocation: {s3Location: {bucket: <bucket>, key: "firmware.bin"}}}], roleArn=<roleArn>).
5. Poll GetOTAUpdate until otaUpdateStatus == CREATE_COMPLETE and read `awsIotJobId`.
6. Call GetJobDocument(jobId=<awsIotJobId>). Observed: a 200 response with no `document` key (KeyError). Expected: a non-empty JSON job document naming the file/key from step 4. Same with beforeSubstitution=true.
