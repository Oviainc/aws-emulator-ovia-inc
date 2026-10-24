Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# StartMedicalTranscriptionJob accepts a Media.MediaFileUri that names no object, creating a job that always COMPLETES

_Filed: 2026-10-24T17:10:04Z_
_Operation: StartMedicalTranscriptionJob | Class: validation_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler's media check (lines 271-281) refuses only an absent or whitespace-only `Media.MediaFileUri`; any non-empty string is accepted, the record is stored, and `MedicalTranscriptionJobRecord::advance_clock` then moves it unconditionally `IN_PROGRESS -> COMPLETED` and reports a `Transcript.TranscriptFileUri`. Nothing resolves the URI against the companion object store, so no medical transcription job this emulator accepts can ever be reported as `FAILED`.

Expected (real service): a `MediaFileUri` that does not name an existing S3 object is refused at the control plane with HTTP 400 `BadRequestException` — the recorded AWS response for the sibling operation is "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." (see open issue 20261022T175151Z for StartTranscriptionJob, filed from a wire recording) — and NO job record is created, so a following `GetMedicalTranscriptionJob` reports the job as unknown and `ListMedicalTranscriptionJobs` does not list it. Where the media is reachable but unusable, the job reaches `FAILED` with one of the `FailureReason` values the model enumerates ("Unsupported media format", "The media format provided does not match the detected media format", "Invalid sample rate for audio file", "Invalid file size: file size too large", ...).

Actual: HTTP 200 with a `MedicalTranscriptionJob` body, a stored record, and a job that reaches `COMPLETED` with a transcript location for a media file that does not exist. The whole failure half of this family's lifecycle is unreachable, so a client's error handling (and any poll-until-FAILED path) is never exercised. Note the handler already performs the cross-resource lookup for `Settings.VocabularyName` (lines 336-346), so the pattern of consulting state before storing a job is established in this very handler — only the media object is unchecked.

## Evidence

Spec: `smithy_query(query="operation", operation="StartMedicalTranscriptionJob")` — `Media` is @required and `MediaFileUri` is "The Amazon S3 location of the media file you want to transcribe"; the operation models `BadRequestException` (@httpError 400, "Your request didn't pass one or more validation tests"); `smithy_query(query="shape", shape="MedicalTranscriptionJob")` documents `FailureReason` with the concrete per-media failure values listed above and `TranscriptionJobStatus` with FAILED. Recorded AWS behaviour for the identical check on the sibling operation: open issue 20261022T175151Z-transcribe-starttranscriptionjob-accepts-a-media-mediafileuri-aws-rejects-creating (trace set simparity-wire-aws-v1, fixture transcribe-transcription-job-workflow task#2) — 400 BadRequestException, no job stored. Handler: src/ops/start_medical_transcription_job.rs lines 271-281 (presence/whitespace only), record inserted at lines 388-426; src/state.rs `MedicalTranscriptionJobRecord::advance_clock` (lines ~1756-1777) sets `STATUS_COMPLETED` unconditionally and never sets `failure_reason`. Platform capability: other crates resolve S3 objects in-process through the shared emulator (services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs `s3_client`/`read_plugin_file`). Tests: tests/all/medical_transcription_jobs.rs and tests/all/medical_transcription_job_listing.rs start every job with a fabricated `s3://example-media/...` URI and assert COMPLETED, so no test asserts the AWS refusal or any FAILED outcome.

## Steps to Reproduce

1. Boot the emulator with an empty companion object store and create a medical vocabulary is not needed for this path
2. POST / with `X-Amz-Target: Transcribe.StartMedicalTranscriptionJob` and body {"MedicalTranscriptionJobName":"phantom-med","LanguageCode":"en-US","Specialty":"PRIMARYCARE","Type":"DICTATION","OutputBucketName":"example-output","Media":{"MediaFileUri":"s3://no-such-bucket/no-such-object.flac"}}
3. Emulator answers HTTP 200 and stores the job; AWS answers HTTP 400 BadRequestException ("The URI that you provided doesn't point to an S3 object...") and stores nothing
4. Wait past the completion span (~500 ms) and call `Transcribe.GetMedicalTranscriptionJob` with {"MedicalTranscriptionJobName":"phantom-med"} — the emulator reports TranscriptionJobStatus COMPLETED with a Transcript.TranscriptFileUri; AWS reports the job as not found
5. Call `Transcribe.ListMedicalTranscriptionJobs` with {"Status":"FAILED"} — the emulator can never return a row, because no job of this family ever fails
