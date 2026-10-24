Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# A job started with IdentifyLanguage never reports an identified LanguageCode, LanguageCodes or IdentifiedLanguageScore

_Filed: 2026-10-24T09:08:05Z_
_Operation: StartTranscriptionJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartTranscriptionJob` accepts and stores `IdentifyLanguage` / `IdentifyMultipleLanguages` (and `LanguageOptions`), and the handler even requires exactly one language specifier. But the completion path never produces a language-identification RESULT: `TranscriptionJobRecord::to_job` / `to_summary` hard-code `identified_language_score: None` and `language_codes: None`, and `record.language_code` stays `None` for the whole life of a job started with `IdentifyLanguage: true` (it is only ever set from the request's own `LanguageCode`).

Expected (real service): a job started with `IdentifyLanguage: true` reaches `COMPLETED` carrying the identified `LanguageCode` and an `IdentifiedLanguageScore` ("The confidence score associated with the language identified in your media file"); a job started with `IdentifyMultipleLanguages: true` carries `LanguageCodes` (code + duration per language). `GetTranscriptionJob` / `ListTranscriptionJobs` therefore let a caller read which language was identified. Actual: the emulator's COMPLETED job reports no `LanguageCode` at all, no `IdentifiedLanguageScore` and no `LanguageCodes`, so the entire automatic-language-identification feature yields nothing observable, and a client that reads `TranscriptionJob.LanguageCode` after completion (the documented way to learn the result) gets an absent member.

A faithful emulation does not need audio: when `LanguageOptions` is supplied the identified code can be taken from it (the service is documented to restrict identification to those options), otherwise a deterministic default such as `en-US` with a score is far closer to the wire AWS produces than three absent members. This is distinct from the open `GetTranscriptionJob` redaction issue, which is about `Transcript.RedactedTranscriptFileUri`.

## Evidence

Spec: `smithy_query(query="doc", shape="TranscriptionJob", member="IdentifiedLanguageScore")` — "The confidence score associated with the language identified in your media file"; `TranscriptionJob$LanguageCodes` is documented for multi-language identification; `StartTranscriptionJobRequest$IdentifyLanguage` — "If you include IdentifyLanguage and want to apply custom language models ... Note that you must include LanguageOptions" and the identified language is reported back on the job. Handler/state: `src/ops/start_transcription_job.rs` lines 396-437 store `identify_language` / `identify_multiple_languages` / `language_options` and set no language result; `src/state.rs` `TranscriptionJobRecord::advance_clock` (lines ~1325-1356) sets only status, completion time, transcript and subtitle URIs, and `to_job` (lines ~1370-1399) / `to_summary` (lines ~1403-1420) pass `identified_language_score: None`, `language_codes: None` with the comment "Language identification is not performed by this emulator". Tests: `tests/all/transcription_jobs.rs` ECHOED_MEMBERS (lines 223-237) assert only that the request members are echoed; no test asserts a language-identification result on a completed job, so nothing covers this.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` and body {"TranscriptionJobName":"idlang","Media":{"MediaFileUri":"s3://example-media/audio.flac"},"IdentifyLanguage":true,"LanguageOptions":["en-US","fr-FR"]} — 200, job IN_PROGRESS
2. Wait past the completion transition (~1 s) and call `Transcribe.GetTranscriptionJob` with {"TranscriptionJobName":"idlang"}
3. Emulator answers TranscriptionJobStatus COMPLETED with no LanguageCode, no IdentifiedLanguageScore; AWS answers COMPLETED with the identified LanguageCode (one of the LanguageOptions) and an IdentifiedLanguageScore between 0 and 1
4. Repeat with {"IdentifyMultipleLanguages":true,"LanguageOptions":["en-US","es-US"]} and read the completed job — emulator reports no LanguageCodes member, AWS reports one entry per identified language
5. Call `Transcribe.ListTranscriptionJobs` — the summary has the same three members absent
