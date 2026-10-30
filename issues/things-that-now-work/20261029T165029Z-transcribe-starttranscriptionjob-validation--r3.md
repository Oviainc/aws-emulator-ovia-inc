Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a399f03ccc4f5afa40c0af0ce737308e8fb96c4f

# A per-language LanguageIdSettings entry naming a VocabularyFilterName is accepted with no VocabularyFilterMethod, the one co-requirement that member's own model doc states

_Filed: 2026-10-29T16:50:29Z_
_Operation: StartTranscriptionJob | Class: validation_

> _Note: this issue was identified against the code base at commit `a399f03ccc4f5afa40c0af0ce737308e8fb96c4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`LanguageIdSettings$VocabularyFilterName` carries exactly the same documented co-requirement as `Settings$VocabularyFilterName`: "Note that if you include VocabularyFilterName in your request, you must also include VocabularyFilterMethod." The handler enforces that rule for the TOP-LEVEL settings member only. The per-language `LanguageIdSettings` map — which this crate already walks for the dangling-reference check, through the single-homed `crate::state::extend_language_id_references` — is never read for the co-requirement, so a start whose only vocabulary-filter reference sits inside a `LanguageIdSettings` entry and that carries no `VocabularyFilterMethod` anywhere is accepted 200 and stored.

The result is exactly what the handler's own comment says the co-requirement block exists to prevent: "A request that names only one side is refused rather than stored, so no stored job reports a vocabulary filter with no method" (start_transcription_job.rs:392-394) — yet `GetTranscriptionJob` then reports `LanguageIdSettings[<lang>].VocabularyFilterName` with no method in force anywhere on the job.

The same omission is in the sibling start that models the same map: `StartCallAnalyticsJob` checks only `Settings.VocabularyFilterName`/`Settings.VocabularyFilterMethod` and never the entries of `Settings.LanguageIdSettings`, although it too expands that map for the reference check. Both paths are symptoms of the same missing application of the crate's existing `crate::state::missing_companion_error` rule.

An existing test pins the current (accepting) behaviour, and its stated reason is the thing the model contradicts: the test's own field doc says the request is "qualified by the vocabulary filter `method` where the member it uses documents one", and the `LanguageIdSettings` row therefore drops the method — but that member DOES document one.

## Evidence

Smithy model (`smithy_query(query="shape", shape="com.amazonaws.transcribe#LanguageIdSettings")`), member `VocabularyFilterName`:
  "Note that if you include <code>VocabularyFilterName</code> in your request, you must also include <code>VocabularyFilterMethod</code>."
(the identical sentence on `Settings$VocabularyFilterName` is the one the handler DOES enforce)

Handler — the co-requirement block reads the top-level settings only:
services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs:402-407
```
            (
                "Settings.VocabularyFilterName",
                settings.is_some_and(|settings| settings.vocabulary_filter_name.is_some()),
                "Settings.VocabularyFilterMethod",
                settings.is_some_and(|settings| settings.vocabulary_filter_method.is_some()),
            ),
```
and its own stated purpose, start_transcription_job.rs:392-394:
```
        // A request that names only one side is refused rather than stored, so no
        // stored job reports a vocabulary filter with no method, queueing with no role
```

The same handler DOES walk the per-language map, for the other rule — start_transcription_job.rs:496-499:
```
        crate::state::extend_language_id_references(
            &mut references,
            input.language_id_settings.as_ref(),
        );
```
and the helper's own doc says that expansion exists so the two operations "cannot end up checking different members of it" — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1330-1336:
```
/// `LanguageIdSettings` names resources of the very same three families a request's
/// top-level settings do (`VocabularyName`, `VocabularyFilterName`,
/// `LanguageModelName` — `smithy_query(query="shape", shape="LanguageIdSettings")`),
/// so the expansion is single-homed here: the operations that accept the map cannot
/// end up checking different members of it.
```

Sibling start, same omission — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_call_analytics_job.rs:285-292 (only `Settings.*`) beside start_call_analytics_job.rs:366-369 (`extend_language_id_references` over `settings.language_id_settings`).

Test coverage: the co-requirement suite covers top-level `Settings` rows only — services/service-com-amazonaws-transcribe-transcribe/tests/all/transcription_job_request_rules.rs:366 `a_start_naming_one_side_of_a_co_requirement_is_refused`, whose accepted rows at lines 312-326 are all `json!({"Settings": {"VocabularyFilterName": HELD_FILTER, "VocabularyFilterMethod": method}})`; no row names a `LanguageIdSettings` entry.
The opposite behaviour is pinned, on a reason the model contradicts — services/service-com-amazonaws-transcribe-transcribe/tests/all/vocabulary_filter_file_uri.rs:615-617:
```
    /// A start request for the job `job` applying the filter `filter`, qualified by the
    /// vocabulary filter `method` where the member it uses documents one.
    request: fn(job: &str, filter: &str, method: &str) -> Value,
```
and the row that drops it, vocabulary_filter_file_uri.rs:651-670:
```
            case: "StartTranscriptionJob LanguageIdSettings[en-US].VocabularyFilterName",
            ...
            request: |job, filter, _| {
                json!({
                    ...
                    "LanguageIdSettings": {"en-US": {"VocabularyFilterName": filter}}
                })
            },
```

## Steps to Reproduce

1. Create a custom vocabulary filter: POST / with `X-Amz-Target: Transcribe.CreateVocabularyFilter` and {"VocabularyFilterName":"f1","LanguageCode":"en-US","Words":["foo"]}
2. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` and {"TranscriptionJobName":"lid-filter-no-method","IdentifyLanguage":true,"LanguageOptions":["en-US"],"Media":{"MediaFileUri":"s3://example-media/audio.flac"},"LanguageIdSettings":{"en-US":{"VocabularyFilterName":"f1"}}} — no `VocabularyFilterMethod` appears anywhere in the request
3. Observe HTTP 200 and a stored job, where the modelled rule quoted on `LanguageIdSettings$VocabularyFilterName` makes this the same one-sided co-requirement the handler refuses 400 BadRequestException for `Settings.VocabularyFilterName`
4. POST / with `X-Amz-Target: Transcribe.GetTranscriptionJob` and {"TranscriptionJobName":"lid-filter-no-method"} — the job reports `LanguageIdSettings["en-US"]["VocabularyFilterName"] = "f1"` with no method in force
5. Repeat against the sibling: POST / with `X-Amz-Target: Transcribe.StartCallAnalyticsJob` and {"CallAnalyticsJobName":"ca-lid-filter","Media":{"MediaFileUri":"s3://example-media/call.flac"},"Settings":{"LanguageIdSettings":{"en-US":{"VocabularyFilterName":"f1"}}}} — also accepted 200
6. Contrast with the enforced path: the same request with {"Settings":{"VocabularyFilterName":"f1"}} is refused 400 with "Settings.VocabularyFilterMethod is required when Settings.VocabularyFilterName is specified."
