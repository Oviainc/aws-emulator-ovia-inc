Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# UpdateCallAnalyticsCategory replaces a category's rules without checking the documented time-range co-requirements

_Filed: 2026-10-24T17:10:04Z_
_Operation: UpdateCallAnalyticsCategory | Class: validation_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler checks that the category exists and that the request's optional `InputType` matches the stored one, then overwrites `record.rules = input.rules` with no inspection of the rules themselves. A replacement rule whose `AbsoluteTimeRange` sets only `StartTime`/`EndTime`, or whose `RelativeTimeRange` sets only `StartPercentage`/`EndPercentage`, is accepted with HTTP 200 and becomes the category's live rule set.

Expected (real service): HTTP 400 `BadRequestException` ("Your request didn't pass one or more validation tests"), with the STORED rules left untouched — the model states the pairing requirement in both directions for both time-range shapes. Actual: HTTP 200, the stored rules are replaced by the malformed set, and GetCallAnalyticsCategory reports it.

This is the same unchecked path as on CreateCallAnalyticsCategory (the two handlers each build/replace `rules` independently), and the fix belongs in one shared helper alongside `crate::state::co_required_setting_error`. It matters slightly more here because the overwrite is destructive: the category's previously valid rules are lost to a request AWS would have rejected.

## Evidence

Spec: `smithy_query(query="shape", shape="AbsoluteTimeRange,RelativeTimeRange")` — "If you include StartTime in your request, you must also include EndTime" / "You must use StartTime and EndTime as a set"; "If you include StartPercentage in your request, you must also include EndPercentage" (and both converses). UpdateCallAnalyticsCategory models BadRequestException (@httpError 400). Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/update_call_analytics_category.rs lines 181-212 — lookup (183-188), InputType-match refusal (196-207), then `record.rules = input.rules` (line 211) with no inspection of the rules; the file contains no Rule-member access at all. Tests: tests/all/call_analytics_category_updates.rs (lines 61-79) only sends well-formed ranges (First/Last alone, or StartPercentage+EndPercentage, or StartTime+EndTime), so neither the AWS refusal nor the current accept is asserted.

## Steps to Reproduce

1. Create a category: `Transcribe.CreateCallAnalyticsCategory` {"CategoryName":"cat","Rules":[{"NonTalkTimeFilter":{"Threshold":5000}}]} — 200
2. POST / with `X-Amz-Target: Transcribe.UpdateCallAnalyticsCategory` and body {"CategoryName":"cat","Rules":[{"InterruptionFilter":{"AbsoluteTimeRange":{"EndTime":50000}}}]}
3. Emulator answers HTTP 200 and the category's rules are now the malformed one; AWS answers HTTP 400 BadRequestException (StartTime missing) and leaves the stored rules as they were
4. Call `Transcribe.GetCallAnalyticsCategory` with {"CategoryName":"cat"} — the emulator reports the half-specified range as the category's current rule; AWS still reports the original NonTalkTimeFilter rule
