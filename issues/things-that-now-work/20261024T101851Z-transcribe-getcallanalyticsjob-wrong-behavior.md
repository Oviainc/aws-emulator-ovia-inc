Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# A completed Call Analytics job reports no LanguageCode, MediaFormat, MediaSampleRateHertz or redacted transcript location

_Filed: 2026-10-24T10:18:51Z_
_Operation: GetCallAnalyticsJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartCallAnalyticsJob` takes no language, media-format or sample-rate member — the service derives all three — yet the emulator's projection leaves them absent for every completed job, and never reports a redacted output even when the request asked for redaction.

Expected (AWS) for a COMPLETED Call Analytics job: `CallAnalyticsJob.LanguageCode` carries the language the service identified (with `IdentifiedLanguageScore` when identification ran), `MediaFormat` and `MediaSampleRateHertz` carry what the service detected from the media file, and a job started with `Settings.ContentRedaction` (or `Media.RedactedMediaFileUri`) reports `Transcript.RedactedTranscriptFileUri` in addition to `TranscriptFileUri`. `ListCallAnalyticsJobs` summaries carry the same `LanguageCode`.

Actual (emulator): `CallAnalyticsJobRecord::to_job` / `to_summary` set `media_format: None`, `media_sample_rate_hertz: None`, `identified_language_score: None`, `call_analytics_job_details: None`, and derive `LanguageCode` only when `Settings.LanguageOptions` happens to name exactly one code — so the common request (no LanguageOptions, or two or more) answers a completed job with NO LanguageCode at all. `advance_clock` always sets `redacted_transcript_file_uri: None`. A client that switches on the job's language, or that fetches the redacted transcript it asked for, gets a null where AWS always has a value. This is the Call Analytics counterpart of the already-filed GetTranscriptionJob/StartTranscriptionJob reporting gaps, which do not cover this family.

## Evidence

Spec: `CallAnalyticsJob$LanguageCode` — "The language code used to create your Call Analytics job" — plus `MediaFormat`, `MediaSampleRateHertz`, `IdentifiedLanguageScore` and `CallAnalyticsJobDetails` are output-only members of a shape whose start request carries none of them (smithy_query operation=StartCallAnalyticsJob / shape=CallAnalyticsJob), i.e. the service computes them; `Transcript$RedactedTranscriptFileUri` is documented as the location of the redacted transcript, and `Media$RedactedMediaFileUri` "produces a redacted audio file in addition to a redacted transcript". Handler/state: services/.../src/state.rs `CallAnalyticsJobRecord::language_code` (returns Some only for a single-entry LanguageOptions), `to_job` (media_format/media_sample_rate_hertz/identified_language_score/call_analytics_job_details all hardcoded None), `advance_clock` (redacted_transcript_file_uri: None); `get_call_analytics_job.rs` returns that projection verbatim. Tests: tests/all/call_analytics_jobs.rs lines 215-233 and 300-310 assert the CURRENT (absent) behavior, so no test asserts the AWS behavior.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.StartCallAnalyticsJob and {"CallAnalyticsJobName":"ca-lang","Media":{"MediaFileUri":"s3://b/call.wav"},"DataAccessRoleArn":"arn:aws:iam::000000000000:role/TranscribeAccessRole"} (no Settings.LanguageOptions).
2. Wait past the completion delay and call Transcribe.GetCallAnalyticsJob for "ca-lang": AWS reports CallAnalyticsJobStatus COMPLETED together with LanguageCode, MediaFormat and MediaSampleRateHertz; the emulator reports COMPLETED with all three absent.
3. Call Transcribe.ListCallAnalyticsJobs: the summary also carries no LanguageCode.
4. Start another job with {"Settings":{"ContentRedaction":{"RedactionType":"PII","RedactionOutput":"redacted"}}} and poll it: AWS reports Transcript.RedactedTranscriptFileUri; the emulator reports only TranscriptFileUri.
