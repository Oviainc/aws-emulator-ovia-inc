Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# CreateCallAnalyticsCategory stores rules without checking the documented time-range co-requirements

_Filed: 2026-10-24T17:10:04Z_
_Operation: CreateCallAnalyticsCategory | Class: validation_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler validates only the category NAME (conflict) and the account quota before storing `rules: input.rules` verbatim. It never inspects the contents of the rules, so a category whose rule carries a half-specified time range is accepted with HTTP 200 and read back by GetCallAnalyticsCategory / ListCallAnalyticsCategories.

The model documents two co-requirements inside the time-range shapes every rule filter uses:
- `AbsoluteTimeRange`: "If you include StartTime in your request, you must also include EndTime" / "If you include EndTime in your request, you must also include StartTime" (the shape doc repeats it: "You must use StartTime and EndTime as a set; that is, if you include one, you must include both").
- `RelativeTimeRange`: "If you include StartPercentage in your request, you must also include EndPercentage" and the converse.

Expected (real service): a rule whose `AbsoluteTimeRange` sets only `StartTime` (or only `EndTime`), or whose `RelativeTimeRange` sets only `StartPercentage` (or only `EndPercentage`), is refused with HTTP 400 `BadRequestException` ("Your request didn't pass one or more validation tests") and NO category is created. Actual: HTTP 200, the category is stored with the malformed rule, and it is later reported back as a valid category and silently "applied" to Call Analytics jobs.

Smithy cannot express a conditional co-requirement, so the boundary validator cannot refuse this — it has to be in the handler, exactly as this crate already does for the analogous pairs on the settings shapes (`crate::state::co_required_setting_error`, used by StartTranscriptionJob / StartMedicalTranscriptionJob / StartMedicalScribeJob for ShowSpeakerLabels+MaxSpeakerLabels and ShowAlternatives+MaxAlternatives).

## Evidence

Spec: `smithy_query(query="shape", shape="AbsoluteTimeRange,RelativeTimeRange")` — AbsoluteTimeRange$StartTime "If you include StartTime in your request, you must also include EndTime", AbsoluteTimeRange$EndTime the converse, plus the shape doc "You must use StartTime and EndTime as a set"; RelativeTimeRange$StartPercentage "If you include StartPercentage in your request, you must also include EndPercentage", RelativeTimeRange$EndPercentage the converse. `CreateCallAnalyticsCategory` models `BadRequestException` (@httpError 400) whose doc is "Your request didn't pass one or more validation tests". Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/create_call_analytics_category.rs — the whole handle body (lines 176-262) performs exactly two checks (name conflict, lines 198-213; category quota, lines 215-227) and then stores `rules: input.rules` (line 231); nothing in the file reads a Rule's members. Existing in-crate precedent for this class of check: `crate::state::co_required_setting_error` (src/state.rs) called from start_transcription_job.rs lines 337-361. Tests: tests/all/call_analytics_categories.rs (lines 49, 62, 87) and tests/all/call_analytics_category_updates.rs only ever send well-formed ranges (a bounded pair, or First/Last alone), so no test asserts either the AWS refusal or the current lenient accept.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.CreateCallAnalyticsCategory` and body {"CategoryName":"half-range","Rules":[{"InterruptionFilter":{"AbsoluteTimeRange":{"StartTime":10000}}}]}
2. Emulator answers HTTP 200 with CategoryProperties for the stored category; AWS answers HTTP 400 BadRequestException because EndTime is missing
3. Call `Transcribe.GetCallAnalyticsCategory` with {"CategoryName":"half-range"} — the emulator reports the malformed category; AWS has no such category
4. Repeat with {"Rules":[{"SentimentFilter":{"Sentiments":["POSITIVE"],"RelativeTimeRange":{"StartPercentage":10}}}]} — same divergence (EndPercentage missing)
