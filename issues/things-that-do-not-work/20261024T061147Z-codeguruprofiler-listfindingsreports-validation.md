Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# ListFindingsReports interval check is laxer and worded differently than the service's interval validator

_Filed: 2026-10-24T06:11:47Z_
_Operation: ListFindingsReports | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListFindingsReports requires `startTime` and `endTime`. The handler refuses only `startTime > endTime`, and does so with its own sentence "endTime must not be earlier than startTime".

Expected (real service): the shared interval validator — refuse a range that is not strictly increasing with 400 ValidationException and the message "Interval start time (<startTime>) must be before end time (<endTime>)", built from the request's own bounds. That is the sentence the corpus records for this service's interval refusals (ListProfileTimes) and the one the emulator already emits for GetRecommendations, where `startTime == endTime` is correctly a 400.

Actual (emulator): `startTime == endTime` is accepted and answered 200 (a test even asserts "an instant range (startTime == endTime) lists every window over it"), and the reversed range is refused with a message that names neither bound. So the same input is a 400 on GetRecommendations and a 200 here, which cannot both match one service-wide validator.

## Evidence

Smithy model: ListFindingsReportsInput.startTime/endTime are both @required timestamps and the operation models ValidationException (400). Emulator's own encoding of the rule for the sibling operation: src/ops/get_recommendations.rs refuses `instant_key(start) >= instant_key(end)` with "Interval start time ({}) must be before end time ({})" (tests/all/recommendations_analysis.rs row "a start equal to the end" -> 400). Handler with the gap: src/ops/list_findings_reports.rs lines ~185-192 (`if input.start_time.as_secs_f64() > input.end_time.as_secs_f64()` + fixed message). Existing coverage asserts the current, laxer behavior (tests/all/findings_reports_listing.rs: "an instant range (startTime == endTime) lists every window over it"; reversed range -> 400), so no test asserts the AWS rule. Not covered by any open issue (the open ListProfileTimes message cause names a different operation).

## Steps to Reproduce

1. Create profiling group N.
2. GET /internal/profilingGroups/N/findingsReports?startTime=2026-01-01T00:00:00Z&endTime=2026-01-01T00:00:00Z -> 200 with an empty/overlapping listing; the service's interval rule requires 400 ValidationException (start must be BEFORE end).
3. GET the same path with startTime after endTime -> 400 ValidationException whose message is "endTime must not be earlier than startTime" instead of "Interval start time (<startTime>) must be before end time (<endTime>)".
4. Compare with GET /internal/profilingGroups/N/recommendations?startTime=T&endTime=T, which this emulator already refuses with the interval sentence.
