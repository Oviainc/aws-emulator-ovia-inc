Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# A completed transcription job writes no transcript object to S3, so Transcript.TranscriptFileUri serves nothing

_Filed: 2026-10-24T10:18:51Z_
_Operation: StartTranscriptionJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A job that reaches COMPLETED reports `Transcript.TranscriptFileUri` (and `Subtitles.SubtitleFileUris` when subtitles were requested) but no object is ever created at those locations, so the canonical client workflow — StartTranscriptionJob -> poll GetTranscriptionJob -> GetObject the transcript JSON — cannot complete against the emulator.

Expected (AWS): when a job completes, Amazon Transcribe writes the transcript JSON into the caller's `OutputBucketName`/`OutputKey` (or into the service-managed location it reports), and a client that follows `Transcript.TranscriptFileUri` with S3 GetObject receives that document; the same holds for each requested subtitle format at `Subtitles.SubtitleFileUris`.

Actual (emulator): `TranscriptionJobRecord::advance_clock` only *computes URI strings* from the stored record; nothing is written into the S3 emulator. `GetObject` on the reported URI answers NoSuchKey/404 while the job says COMPLETED. The service's own state header states this limitation explicitly ("no object is ever written at the reported Transcript.TranscriptFileUri / SubtitlesOutput.SubtitleFileUris").

The same unwritten-output gap exists for the two other job families that share this design — `StartCallAnalyticsJob` (`call_analytics_output_uri`) and `StartMedicalScribeJob` (`medical_scribe_output_uri`, transcript.json / summary.json) — so a fix should cover the shared output helpers, not just the transcription family.

The cross-service seam this needs now exists: the S3 object data plane (PutObject/GetObject/HeadObject) is implemented, and other service crates already drive it in process through `ctx.callback().http_client()` + an `aws_sdk_s3::Client` (see `services/service-com-amazonaws-comprehend-comprehend-20171127/src/state.rs` `s3_client()` / `.put_object()`, and the same pattern in b2bi, codepipeline and elasticbeanstalk). The objection recorded when this was deferred ("naming the S3 crate from the Transcribe crate would form a cross-crate edge") no longer applies.

## Evidence

Well-established service behavior + spec: `Transcript$TranscriptFileUri` is documented as "The Amazon S3 location of your transcript" that the caller downloads, and `StartTranscriptionJobRequest$OutputBucketName`/`OutputKey` exist precisely to name where the service *writes* that object (smithy_query operation=StartTranscriptionJob). Handler/state: services/.../src/state.rs `TranscriptionJobRecord::advance_clock` (sets `transcript = Some(Transcript { transcript_file_uri: Some(self.output_uri("json")) .. })` and the subtitle URIs) performs no write; the module header documents "a completed job means 'the control plane finished with this record', never 'a transcript is fetchable from that URI'". Corpus: the behavioral check id `transcribe.transcription-job-output-written-to-custom-s3-bucket` (listed in the now-fixed issue 20261022T044732Z, whose resolution note explicitly defers this cross-service write and files no issue for it). Tests: tests/all/transcription_jobs.rs lines 454/575/818 assert only the URI *string* the record projects; no test fetches the object, so nothing asserts the correct behavior today.

## Steps to Reproduce

1. Create an S3 bucket in the emulator (`CreateBucket` on `my-output`) and PutObject a media file at `s3://my-input/sample.flac`.
2. POST / with X-Amz-Target: Transcribe.StartTranscriptionJob and {"TranscriptionJobName":"job-1","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://my-input/sample.flac"},"OutputBucketName":"my-output","OutputKey":"transcripts/job-1.json"}.
3. Poll Transcribe.GetTranscriptionJob until TranscriptionJobStatus is COMPLETED and read Transcript.TranscriptFileUri.
4. S3 GetObject that URI: AWS returns the transcript JSON document; the emulator answers 404/NoSuchKey because nothing was written.
5. Repeat with {"Subtitles":{"Formats":["vtt"]}} and GetObject each entry of Subtitles.SubtitleFileUris — same result.
6. Repeat the equivalent flow for StartMedicalScribeJob (MedicalScribeOutput.TranscriptFileUri / ClinicalDocumentUri) and StartCallAnalyticsJob (Transcript.TranscriptFileUri).
