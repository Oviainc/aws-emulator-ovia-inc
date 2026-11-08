Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata medical-imaging.dicom-import-job-success-creates-active-image-set medical-imaging.update-image-set-metadata-concurrent-conflict medical-imaging.update-image-set-metadata-updates-patient-name medical-imaging.delete-datastore-conflicts-with-active-import-job medical-imaging.dicom-import-job-invalid-role-reaches-failed medical-imaging.get-datastore-malformed-id-validation-rejected medical-imaging.start-dicom-import-job-on-creating-datastore-conflict-rejected
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata|_find_image_set_id#17|Timeout|time.sleep(3)
Issue-Key: medical-imaging.dicom-import-job-success-creates-active-image-set|_find_image_set_id#17|Timeout|time.sleep(3)
Issue-Key: medical-imaging.update-image-set-metadata-concurrent-conflict|_find_image_set_id#17|Timeout|time.sleep(3)
Issue-Key: medical-imaging.update-image-set-metadata-updates-patient-name|_find_image_set_id#17|Timeout|time.sleep(3)
Issue-Key: medical-imaging.delete-datastore-conflicts-with-active-import-job|_client_error#10|AssertionError|raise AssertionError(f"expected DeleteDatastore to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: medical-imaging.dicom-import-job-invalid-role-reaches-failed|run#14|AssertionError|raise AssertionError("StartDICOMImportJob unexpectedly succeeded with a nonexistent role")
Issue-Key: medical-imaging.get-datastore-malformed-id-validation-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: medical-imaging.start-dicom-import-job-on-creating-datastore-conflict-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: e0f3b342c2995e24bb81f3397837b3776f1c553b

# medical-imaging: imported image sets never materialise, DeleteDatastore/StartDICOMImportJob miss their refusals, and two refusal texts diverge (5 causes)

> _Note: this issue was identified against the code base at commit `e0f3b342c2995e24bb81f3397837b3776f1c553b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDICOMImportJob

## Description
Behavioral replay of AWS HealthImaging (medical-imaging) left 8 of 18 checks failing. They cluster into 5 distinct root causes, all in this one service. This issue bundles all 5; a fixer must close every one of them, and should re-replay the full trace_ids list.

=== Cause 1 of 5: a DICOM import that reaches COMPLETED creates no image set, so the whole image-set data plane is unreachable over HTTP ===
Failing checks: medical-imaging.dicom-import-job-success-creates-active-image-set, medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata, medical-imaging.update-image-set-metadata-updates-patient-name, medical-imaging.update-image-set-metadata-concurrent-conflict.
AWS: Smithy: StartDICOMImportJob is documented "Start importing bulk data into an ACTIVE data store. The import job imports DICOM P10 files ... found in the S3 prefix specified by the inputS3Uri parameter"; ImageSetWorkflowStatus declares IMPORTING/IMPORTED and GetImageSet/SearchImageSets/GetImageSetMetadata/GetImageFrame model the image set an import produces. A job that reaches jobStatus COMPLETED therefore leaves at least one image set in the target data store: imageSetState ACTIVE, imageSetWorkflowStatus IMPORTED, discoverable by SearchImageSets on the study's DICOMStudyInstanceUID, carrying the normalized Patient/Study metadata of the imported P10 file and its image frames (GetImageFrame serves them with a contentType of the image/* family).
Emulator: the job record is created and the clock marches it SUBMITTED -> IN_PROGRESS -> COMPLETED, but nothing is ever inserted into the data store's image sets. SearchImageSets keeps answering an empty imageSetsMetadataSummaries for the imported study, so each of the four checks sits in its image-set discovery loop until the harness's 60s per-check budget expires ("Check timed out after 60.0s"). Because the first imported image set can never exist, CopyImageSet, UpdateImageSetMetadata (including its optimistic-concurrency/ConflictException behavior) and GetImageFrame are all unreachable through the front door, not just unverified.
Why: a client cannot do anything an imaging data store exists for — import, then find, read, copy or update an image set. Every data-plane workflow dead-ends after GetDICOMImportJob reports COMPLETED.
Fix: the import job's completion path in the state module that owns the clock transition (ImportJob/Datastore advance) plus the StartDICOMImportJob handler, which currently records only the job; the completion must materialise the image set(s), their DICOM metadata and their image frames from what the job was asked to import.
Traces: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/start_dicom_import_job.rs lines 245-277 build an ImportJob and insert it into datastore.import_jobs and touch nothing else; src/state.rs IMPORT_JOB_IN_PROGRESS_DELAY/IMPORT_JOB_COMPLETION_DELAY (lines 89-93) only move the status. This is the same root cause the OPEN issue 20261024T071454Z-medical-imaging-startdicomimportjob-wrong-behavior.md describes from the source side; the four check ids above are the replay evidence for it and are what must pass. The related OPEN issue 20261024T002353Z-medical-imaging-startdicomimportjob-event-propagation.md covers reading the input prefix / writing the output manifest / reaching FAILED, which cause 3 below also touches.

=== Cause 2 of 5: DeleteDatastore is accepted while a DICOM import job on that data store is still running ===
Failing check: medical-imaging.delete-datastore-conflicts-with-active-import-job.
AWS: Smithy: DeleteDatastore models ConflictException (HTTP 409), documented "Updating or deleting a resource can cause an inconsistent state". Recorded behavior: with an import job in flight on the data store, DeleteDatastore is refused 409 ConflictException with message "Cannot delete a datastore with running import jobs" — that sentence is entirely fixed text, with no value taken from the request or state — and the data store is left untouched (GetDatastore still reports ACTIVE). Once the job resolves to COMPLETED/FAILED the identical call succeeds and answers datastoreStatus DELETING.
Emulator: the call is accepted: HTTP 200, {"datastoreId": "<id>", "datastoreStatus": "DELETING"}. The handler has no running-import-job rule at all: it refuses only a creation still in flight and a store that still holds image sets. Compounding it, IMPORT_JOB_COMPLETION_DELAY is 600ms, so even with the rule added there is no in-flight window a polling client can observe; the job window must stay open long enough for a client that submits a job and immediately calls DeleteDatastore to see the conflict.
Why: a client's data store (and the import writing into it) can be destroyed mid-import, and code that relies on the 409 to serialise teardown against in-flight work never sees it — the guard silently does nothing.
Fix: DeleteDatastore's handler, which must consult the data store's non-terminal import jobs before marking it DELETING, plus the import-job clock span in the state module.
Traces: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/delete_datastore.rs lines 168-202 (only creation_in_flight() and held_image_sets() are checked); src/state.rs line 93 IMPORT_JOB_COMPLETION_DELAY = 600ms.

=== Cause 3 of 5: StartDICOMImportJob accepts a dataAccessRoleArn naming a role that does not exist ===
Failing check: medical-imaging.dicom-import-job-invalid-role-reaches-failed.
AWS: Smithy: dataAccessRoleArn is required, "The Amazon Resource Name (ARN) of the IAM role that grants permission to access medical imaging resources", and the operation models AccessDeniedException (HTTP 403). Recorded behavior: a well-formed ARN naming a role the account does not hold is refused synchronously with 403 AccessDeniedException and message "Amazon Medical Imaging is not authorized to assume role: <dataAccessRoleArn>. Please update the role's trust policy." — the two sentences are fixed text and the only variable part is the role ARN copied verbatim from the request. The rejected call creates nothing: ListDICOMImportJobs afterwards still shows only the jobs submitted before it, with an unchanged count.
Emulator: the call succeeds (HTTP 200, jobStatus SUBMITTED) and the job is persisted, so the check fails with "StartDICOMImportJob unexpectedly succeeded with a nonexistent role". The handler never resolves dataAccessRoleArn against IAM: it only validates the modelled pattern, then copies the ARN into the job record.
Why: the main misconfiguration an import can have — an unassumable or absent role — is laundered into a job that reports success, so a client never learns its role/trust policy is wrong and the emulator cannot be used to exercise that failure path at all.
Fix: StartDICOMImportJob's validation, resolving the role against the emulated IAM state (the same cross-crate state seam other crates use) before the job is minted, and refusing with the modelled AccessDeniedException before anything is written.
Traces: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/start_dicom_import_job.rs lines 177-277 — no AccessDeniedException is ever constructed although the handler declares the variant at line 104.

=== Cause 4 of 5: a ValidationException about a resource identifier echoes the rejected value, which AWS withholds ===
Failing check: medical-imaging.get-datastore-malformed-id-validation-rejected.
AWS: Smithy: GetDatastore.datastoreId is an @httpLabel with pattern ^[0-9a-z]{32}$ and the operation models ValidationException (HTTP 400). Recorded behavior for a datastoreId that fails that pattern: HTTP 400, code ValidationException, message "1 validation error detected: datastoreId failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]{32}". The envelope ("1 validation error detected: "), the clause separator (" failed to satisfy constraint: ") and the requirement wording are fixed text; the variable parts are the member path (datastoreId) and the unanchored pattern taken from the model. The rejected VALUE is withheld — this member's clause carries no "Value '<x>' at '<path>'" prefix.
Emulator: the status and code are right but the message is "1 validation error detected: Value 'not-a-valid-datastore-id!!' at 'datastoreId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]{32}" — it echoes the caller's value in the prefixed form. The crate already owns this exact rule (withholds_value / VALUE_WITHHOLDING_MEMBERS) but the member list covers only datastorename, jobname, kmskeyarn and the three attribute documents, so the resource-identifier members are not in it.
Why: a client (or test) matching the service's validation text does not match, and the emulator reproduces a caller-supplied value in an error body where AWS deliberately does not.
Fix: the service's validation-message rule in the state module — the value-withholding member set must cover the resource-identifier members (datastoreId, and its siblings bound the same way such as imageSetId and jobId) so every clause about one is worded "<path> failed to satisfy constraint: ...".
Traces: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs lines 501-537 (VALUE_WITHHOLDING_MEMBERS / withholds_value) and lines 460-479 (constraint_clause). Related, already [fixed]: 20261022T065704Z-medical-imaging-error-response-messages-do-not-follow.md — the same text contract, a member class its fix did not cover.

=== Cause 5 of 5: the not-ACTIVE data store conflict is worded differently from AWS ===
Failing check: medical-imaging.start-dicom-import-job-on-creating-datastore-conflict-rejected.
AWS: Smithy: StartDICOMImportJob is documented "Start importing bulk data into an ACTIVE data store" and models ConflictException (HTTP 409). Recorded behavior for a data store still CREATING: HTTP 409, code ConflictException, message "The datastore is not active." — the whole sentence is fixed text, with no identifier or status interpolated.
Emulator: the status and the code match, but the message is "The data store is not in the ACTIVE state, so it cannot serve this request.", a sentence chosen by the crate rather than AWS's.
Why: a client that branches on, logs or asserts the refusal text sees a message AWS never emits; the divergence is served for every operation that refuses on this rule, not only StartDICOMImportJob, because one shared constant words them all.
Fix: the shared refusal text in the state module (and any sibling constant worded by analogy to it), so this conflict is reported with AWS's sentence.
Traces: services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs lines 392-397 (DATASTORE_NOT_ACTIVE_MESSAGE), used by src/ops/start_dicom_import_job.rs lines 195-200. The refusal itself was introduced by the already [fixed] issue 20261024T013850Z-medical-imaging-startdicomimportjob-validation.md, whose fix chose this wording; only the text is wrong, the refusal is correct.

## Evidence
Suite: 10/18 passed for medical-imaging; 8 mismatches, all clustered above.

Cause 1 — four checks killed by the harness while waiting for an image set the import never creates:
  [mismatch] medical-imaging.dicom-import-job-success-creates-active-image-set — "Check timed out after 60.0s". The check creates a data store, waits ACTIVE, PUTs a minimal DICOM P10 to s3://<bucket>/in/, starts the import, waits for a terminal job status, then loops SearchImageSets (EQUAL on DICOMStudyInstanceUID) for up to 60s (_find_image_set_id) before it would raise "import COMPLETED but no image set was ever indexed for the study".
  [mismatch] medical-imaging.copy-image-set-creates-active-copy-with-matching-metadata — "Check timed out after 60.0s" (same discovery loop, before CopyImageSet).
  [mismatch] medical-imaging.update-image-set-metadata-updates-patient-name — "Check timed out after 60.0s".
  [mismatch] medical-imaging.update-image-set-metadata-concurrent-conflict — "Check timed out after 60.0s".
  That the timeout is in the post-import loops and not in setup is proved by the delete-datastore check below, which performed the identical setup (CreateDatastore, wait ACTIVE, S3 bucket + IAM role + policy, StartDICOMImportJob) and reached its assertion inside the same 60s budget. Job completion itself is fast (IMPORT_JOB_COMPLETION_DELAY = 600ms), so the stall is the image set that is never indexed.

Cause 2 — expected 409 ConflictException "Cannot delete a datastore with running import jobs"; got:
  AssertionError: expected DeleteDatastore to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'datastoreId': '910a2dec89025cc13c987d390ddfbc54', 'datastoreStatus': 'DELETING'} (the datastoreId is a per-run value, not part of the requirement).

Cause 3 — expected 403 AccessDeniedException "Amazon Medical Imaging is not authorized to assume role: arn:aws:iam::<account>:role/<role>. Please update the role's trust policy."; got:
  AssertionError: StartDICOMImportJob unexpectedly succeeded with a nonexistent role (the call returned 200 with a jobId).

Cause 4 — GetDatastore(datastoreId="not-a-valid-datastore-id!!"):
  expected message '1 validation error detected: datastoreId failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]{32}'
  got "1 validation error detected: Value 'not-a-valid-datastore-id!!' at 'datastoreId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]{32}"
  (code ValidationException and HTTP 400 were correct; only the clause form diverges, and the echoed value is whatever the caller sent.)

Cause 5 — StartDICOMImportJob against a data store still CREATING:
  expected message 'The datastore is not active.'
  got 'The data store is not in the ACTIVE state, so it cannot serve this request.'
  (ConflictException/409 were correct.)

## Steps to Reproduce
Mount the medical-imaging, s3, iam and sts emulators on one router and replay the 8 checks in trace_ids. Per cause:

Cause 1: POST /datastore (clientToken) and poll GET /datastore/{datastoreId} until ACTIVE. Create a bucket, PUT a DICOM P10 object under in/, create a role HealthImaging can assume. POST /startDICOMImportJob/datastore/{datastoreId} with dataAccessRoleArn, inputS3Uri=s3://<bucket>/in/, outputS3Uri=s3://<bucket>/out/. Poll GET /getDICOMImportJob/datastore/{datastoreId}/job/{jobId} until COMPLETED. Then POST /datastore/{datastoreId}/searchImageSets with filters [{operator: EQUAL, values: [{DICOMStudyInstanceUID: <the imported study UID>}]}] — imageSetsMetadataSummaries stays empty forever, and GetImageSet / GetImageSetMetadata / GetImageFrame / CopyImageSet / UpdateImageSetMetadata have no image set to address.

Cause 2: same setup; immediately after StartDICOMImportJob returns SUBMITTED, send DELETE /datastore/{datastoreId}. Observe 200 with datastoreStatus DELETING instead of 409 ConflictException "Cannot delete a datastore with running import jobs", and observe the data store does leave ACTIVE.

Cause 3: with an ACTIVE data store, POST /startDICOMImportJob/datastore/{datastoreId} with dataAccessRoleArn=arn:aws:iam::<account>:role/<a role that was never created> and an inputS3Uri naming a bucket that does not exist. Observe 200 + jobId (and the job then appears in GET /listDICOMImportJobs/datastore/{datastoreId}) instead of 403 AccessDeniedException and no job created.

Cause 4: GET /datastore/not-a-valid-datastore-id!! — compare the ValidationException message against the AWS clause form for this member (no "Value '<x>' at '<path>'" prefix).

Cause 5: POST /datastore to create a data store and, while GetDatastore still reports CREATING, POST /startDICOMImportJob/datastore/{datastoreId} with otherwise valid parameters — compare the ConflictException message with 'The datastore is not active.'
