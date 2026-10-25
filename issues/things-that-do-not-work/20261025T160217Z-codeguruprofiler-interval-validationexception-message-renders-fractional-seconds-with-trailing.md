Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeguruprofiler.get-recommendations-start-after-end-rejected
Issue-Shape-Id: com.amazonaws.codeguruprofiler#CodeGuruProfiler
Issue-Key: codeguruprofiler.get-recommendations-start-after-end-rejected|run#12|AssertionError|assert message == expected_message, (message, expected_message)
Issue-Commit: 2efbe0db58fc669c0e9ee58e639ae2f2fc196447

# Interval ValidationException message renders fractional seconds with trailing zeros stripped (".66Z" instead of ".660Z")

> _Note: this issue was identified against the code base at commit `2efbe0db58fc669c0e9ee58e639ae2f2fc196447`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
Cause 1 of 1: the shared interval-refusal message renders its timestamp bounds with a variable-length fractional-second field instead of AWS's fixed 3-digit millisecond field, so any bound whose millisecond value ends in a zero is reported with the trailing zero(s) dropped. Failing check: codeguruprofiler.get-recommendations-start-after-end-rejected.

AWS: GetRecommendations takes required `startTime`/`endTime` timestamps (Smithy: GetRecommendationsRequest.startTime/endTime, @required, @timestampFormat date-time, bound @httpQuery) and models ValidationException (HTTP 400) for a range that is not strictly increasing. The service's refusal message is the fixed text "Interval start time (" <startTime> ") must be before end time (" <endTime> ")", where the two variable parts are the bounds AS THE REQUEST CARRIED THEM, rendered in the ISO-8601 date-time form this service's own model documents for every one of its time members: "This is specified using the ISO 8601 format. For example, 2020-06-01T13:15:02.001Z represents 1 millisecond past June 1, 2020 1:15:02 PM UTC" (smithy doc, GetRecommendationsRequest.startTime / .endTime / GetRecommendationsResponse.profileStartTime). That form has a FIXED-WIDTH, zero-padded 3-digit millisecond field — ".001Z", not ".1Z" — so a bound at 660 ms renders ".660Z".

Emulator: the message's bounds are rendered by crate::state::iso8601, which delegates to aws_smithy_types DateTime::fmt(Format::DateTime). That RFC-3339 writer emits the MINIMAL fractional field, stripping trailing zeros, so the same instant renders ".66Z". With a request whose bounds are whole, non-zero milliseconds ending in zero, the emulator answered the right status (400) and the right code (ValidationException) but the message
  "Interval start time (2026-10-25T15:59:59.66Z) must be before end time (2026-10-25T14:59:59.66Z)"
where AWS words it
  "Interval start time (2026-10-25T15:59:59.660Z) must be before end time (2026-10-25T14:59:59.660Z)".
Only the fractional-second rendering diverges; the fixed text is correct and the hour/minute values shown here are this run's request values, not part of the requirement. The same defect reaches the fractional-second field generally: a bound at an exact second renders with no fractional field at all, and a bound at 500 ms renders ".5Z", where AWS renders ".000Z" / ".500Z" respectively.

Why: a client that parses or compares the echoed bound — to map the refusal back to the request it sent, to assert on the message in a test, or to re-serialize the bound — gets a string that does not match what it sent and does not match what AWS returns. Because the renderer is the single service-wide helper, every operation that refuses a time range through crate::state::validate_interval (GetRecommendations, ListProfileTimes, and any other interval taker) is wrong in the same way, so the divergence is service-wide rather than specific to one operation.

Fix: the timestamp rendering helper used to build interval refusal messages — crate::state::iso8601 — must emit a fixed-width, zero-padded 3-digit millisecond fractional field for every instant, rather than the minimal RFC-3339 fractional form. The validation rule itself (strictly-increasing bounds compared at nanosecond resolution) and the fixed message text are already correct and should not change. Related but distinct open issue: 20261024T061147Z-codeguruprofiler-listfindingsreports-validation.md reports that ListFindingsReports does not USE this shared validator/message at all; it does not cover the millisecond rendering defect in the message itself, so fixing that one does not fix this.

Traces: codeguruprofiler.get-recommendations-start-after-end-rejected.

## Evidence
Check codeguruprofiler.get-recommendations-start-after-end-rejected, assertion on the ValidationException message:
  actual   = 'Interval start time (2026-10-25T15:59:59.66Z) must be before end time (2026-10-25T14:59:59.66Z)'
  expected = 'Interval start time (2026-10-25T15:59:59.660Z) must be before end time (2026-10-25T14:59:59.660Z)'
The check deliberately normalizes `now` to whole, non-zero milliseconds so the expected message is the one AWS renders. Status (400) and error code (ValidationException) matched; the positive control (validly-ordered range -> 200 with empty anomalies/recommendations) is unaffected.

Fixed text of the message (requirement): 'Interval start time (' ... ') must be before end time (' ... ')'. Variable parts (from the request): the two bounds, in ISO-8601 date-time with a zero-padded 3-digit millisecond field, as the model documents ('2020-06-01T13:15:02.001Z represents 1 millisecond past ...'). The concrete date/time values above are this run's request values and are not part of the requirement.

Code: services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/state.rs lines 1741-1744 (`iso8601` -> `DateTime::fmt(Format::DateTime)`, the trailing-zero-stripping renderer) and lines 1759-1768 (`validate_interval`, which formats both bounds through it); called from services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/get_recommendations.rs lines 211-216.

## Steps to Reproduce
1. CreateProfilingGroup with name N and agentOrchestrationConfig={profilingEnabled:true}.
2. Pick an instant T whose millisecond component is non-zero and ends in a zero digit (e.g. xx:xx:xx.660Z).
3. GET /internal/profilingGroups/N/recommendations?startTime=T&endTime=T-1h (start after end).
4. Observe 400 ValidationException whose message renders the bounds as '...59.66Z'; AWS renders '...59.660Z' (fixed 3-digit millisecond field).
5. Repeat with a bound at an exact second (.000) — the emulator emits no fractional field at all where AWS emits '.000Z'.
6. Same divergence via any other interval taker that goes through crate::state::validate_interval (e.g. ListProfileTimes with a reversed range).
