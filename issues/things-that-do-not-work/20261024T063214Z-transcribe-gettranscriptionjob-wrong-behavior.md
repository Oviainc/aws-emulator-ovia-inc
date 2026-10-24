Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# A completed job started with ContentRedaction never reports Transcript.RedactedTranscriptFileUri

_Filed: 2026-10-24T06:32:14Z_
_Operation: GetTranscriptionJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ContentRedaction` is stored on the job record and echoed back, but the completion path ignores it entirely when it builds the `Transcript`: `TranscriptionJobRecord::advance_clock` always sets `transcript_file_uri: Some(self.output_uri("json"))` and `redacted_transcript_file_uri: None`.

Expected (modeled + documented): `ContentRedaction$RedactionOutput` is `@required` and is one of `redacted | redacted_and_unredacted` — "When you choose `redacted` Amazon Transcribe creates only a redacted transcript. When you choose `redacted_and_unredacted` Amazon Transcribe creates a redacted and an unredacted transcript (as two separate files)." So a COMPLETED job started with `RedactionOutput: redacted` must report `Transcript.RedactedTranscriptFileUri` (and no unredacted `TranscriptFileUri`), and one started with `redacted_and_unredacted` must report BOTH URIs, pointing at two distinct output objects.

Actual: in both cases the emulator reports only `Transcript.TranscriptFileUri` and omits `RedactedTranscriptFileUri`, so `Transcript.RedactedTranscriptFileUri` is unreachable for every job this emulator can complete. A client that follows the documented redaction flow (start with redaction, poll until COMPLETED, read the redacted transcript location) finds the member missing and cannot locate its redacted output; a client that asked for `redacted` only is handed an unredacted-transcript URI the real service would not report. The same projection is read by `ListTranscriptionJobs` summaries, which carry `ContentRedaction` but no transcript URIs, so the only observable surface is Get.

## Evidence

Spec: `smithy_query(query="shape", shape="Transcript,ContentRedaction,RedactionOutput")` — `Transcript` has members `TranscriptFileUri` and `RedactedTranscriptFileUri` ("The Amazon S3 location of your redacted transcript. You can use this URI to access or download your transcript."); `ContentRedaction$RedactionOutput` is required with values `redacted` / `redacted_and_unredacted` and the quoted doc above. Handler/state: `src/state.rs` `TranscriptionJobRecord::advance_clock` (lines ~1027-1046) hard-codes `Transcript { transcript_file_uri: Some(self.output_uri("json")), redacted_transcript_file_uri: None }` with no reference to `self.content_redaction`, and `to_job()` (lines ~1062-1091) just clones that `transcript`. `src/ops/get_transcription_job.rs` lines 190-197 call `advance_clock(now)` and project the record, so the Get answer inherits the omission. Tests: `tests/all/transcription_jobs.rs` lists `ContentRedaction` in `ECHOED_MEMBERS` (line 232) — only the echo is asserted; `a_job_completes_once_its_processing_span_has_elapsed` (line 394 onward) never starts a redacted job and nothing anywhere asserts a redacted transcript URI (grep for `Redacted` in tests/: no hits).

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` and body {"TranscriptionJobName":"redacted-job","Media":{"MediaFileUri":"s3://example-media/call.flac"},"LanguageCode":"en-US","OutputBucketName":"example-output","ContentRedaction":{"RedactionType":"PII","RedactionOutput":"redacted"}}
2. Wait past the completion transition (~500 ms, or 1 s to be safe) and call `Transcribe.GetTranscriptionJob` with {"TranscriptionJobName":"redacted-job"}
3. Emulator: TranscriptionJobStatus COMPLETED with Transcript = {"TranscriptFileUri": "s3://example-output/redacted-job.json"} and no RedactedTranscriptFileUri; AWS reports RedactedTranscriptFileUri (redacted output) and no unredacted TranscriptFileUri for RedactionOutput=redacted
4. Repeat with "RedactionOutput":"redacted_and_unredacted" — AWS reports BOTH TranscriptFileUri and RedactedTranscriptFileUri at two distinct keys; the emulator still reports only TranscriptFileUri
