Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-transcription-job-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-transcription-job-workflow
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|StartTranscriptionJob|4xx->2xx|$.Message
Issue-Key: transcribe|StartTranscriptionJob|4xx->2xx|$.__type
Issue-Key: transcribe|StartTranscriptionJob|4xx->2xx|status
Issue-Key: transcribe|ListTranscriptionJobs|2xx->2xx|$.TranscriptionJobSummaries[*]
Issue-Key: transcribe|DeleteTranscriptionJob|4xx->4xx|$.Message
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# StartTranscriptionJob accepts a Media.MediaFileUri AWS rejects, creating a phantom job

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTranscriptionJob

## Description
This issue bundles THREE causes: one unvalidated accept and the two state divergences it leaves behind. A fixer must close all three before submitting.

Cause 1 of 3: StartTranscriptionJob does not validate that Media.MediaFileUri points at an S3 object.
AWS: HTTP 400, __type BadRequestException, message "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again." (recorded response). Smithy: StartTranscriptionJob input Media is @required, MediaFileUri is the S3 location of the media file, and the operation models BadRequestException @httpError 400 — a URI that names no object is refused at the control plane, so NO job record is created.
Emulator: HTTP 200 with a TranscriptionJob body and a stored record. services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs lines 308-318 only reject a Media.MediaFileUri that is absent or whitespace; any non-empty string is accepted and inserted into state (line 448).
Why: the single most common failure mode a client must handle when wiring up transcription — a bad or missing media object — is reported as success, so error-handling paths are never exercised and the client waits for a transcript of a file that does not exist.
Fix: StartTranscriptionJob's request validation (start_transcription_job.rs, the Media.MediaFileUri check) — refuse a URI that is not an s3:// URI naming an object the emulator can resolve, with BadRequestException/400, before any record is stored.
Traces: transcribe-transcription-job-workflow#task#2.

Cause 2 of 3: ListTranscriptionJobs reports the job that should never have been created.
AWS: HTTP 200 with no TranscriptionJobSummaries entry for the rejected job (recorded: the member is absent).
Emulator: HTTP 200 with TranscriptionJobSummaries[0] present for the job name, TranscriptionJobStatus IN_PROGRESS, OutputLocationType SERVICE_BUCKET.
Why: a listing that invents a job makes a caller's reconciliation logic (list, then act on each job) operate on a resource AWS does not have; the extra element also shifts every index and NextToken page boundary.
Fix: follows from cause 1 — the listing projection in src/state.rs is correct, the record must not exist. Re-verify this step after the validation lands rather than closing it unrun.
Traces: transcribe-transcription-job-workflow#task#3.

Cause 3 of 3: DeleteTranscriptionJob refuses the delete for a job state AWS never reached.
AWS: HTTP 400, __type BadRequestException, message "The requested job couldn't be found. Check the job name and try your request again." (recorded) — there is no such job.
Emulator: HTTP 400 with __type agreeing but the message "The transcription job <name> is in the IN_PROGRESS state and can't be deleted. Wait until it is no longer being processed and try your request again." This is not a wording difference: the two responses report DIFFERENT facts (absent vs. non-terminal), and the emulator's is a consequence of the phantom record from cause 1. Smithy: DeleteTranscriptionJob models BadRequestException @httpError 400 only; its doc covers both a nonexistent entity and a non-terminal state, so the status and __type alone cannot distinguish them — the message is the only signal, and it reports the wrong cause.
Why: a teardown or retry routine that distinguishes 'gone' from 'still running' (delete, else wait and delete again) loops forever against the emulator on a job AWS says does not exist.
Fix: follows from cause 1 — with the start refused, this delete must take the not-found branch. Re-verify this step after the validation lands.
Traces: transcribe-transcription-job-workflow#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture transcribe-transcription-job-workflow, 3 divergence clusters:
- task#2 StartTranscriptionJob: expected 400 with $.__type BadRequestException and $.Message "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again."; got 200 with neither member present.
- task#3 ListTranscriptionJobs: expected 200 with no $.TranscriptionJobSummaries[0]; got that element present ({'TranscriptionJobName': '<probe-name>', 'LanguageCode': 'en-US', 'TranscriptionJobStatus': 'IN_PROGRESS', 'OutputLocationType': 'SERVICE_BUCKET', ...}).
- task#4 DeleteTranscriptionJob: expected 400 $.Message "The requested job couldn't be found. Check the job name and try your request again."; got 400 $.Message "The transcription job <name> is in the IN_PROGRESS state and can't be deleted. ...".
Code: start_transcription_job.rs:308-318 validates only emptiness of Media.MediaFileUri; :448 inserts the record unconditionally thereafter.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture transcribe-transcription-job-workflow step task#2: call StartTranscriptionJob with a fresh TranscriptionJobName, LanguageCode en-US and a Media.MediaFileUri that names no existing S3 object. The emulator answers HTTP 200 and stores the job; AWS answers HTTP 400 BadRequestException and stores nothing.
3. Step task#3: call ListTranscriptionJobs — the emulator returns a TranscriptionJobSummaries entry for that job (IN_PROGRESS); the recording has none.
4. Step task#4: call DeleteTranscriptionJob for that name — the emulator reports it as IN_PROGRESS and undeletable; AWS reports the job as not found.
