Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# ListProfileTimes accepts startTime == endTime instead of refusing the non-interval

_Filed: 2026-10-24T06:11:47Z_
_Operation: ListProfileTimes | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListProfileTimes requires `startTime`, `endTime` and `period`. The handler refuses only `endTime < startTime`; an interval whose bounds are equal is accepted and answered 200 with an empty `profileTimes` list (the range is applied half-open: `at >= range_start && at < range_end`).

Expected (real service): the interval rule is "Interval start time (<startTime>) must be before end time (<endTime>)" — equal bounds are not an interval, so they are refused with 400 ValidationException before any lookup, exactly as this emulator already does for GetRecommendations.

Actual (emulator): 200 with an empty page. A caller that mis-computes a zero-width window gets a successful, empty answer instead of the validation error that tells it the window is wrong.

Scope note: this is NOT the already-open cause about this operation's refusal MESSAGE ("endTime must not be before startTime" vs the interval sentence) — that stays as filed; this is the strictness of the check itself (200 vs 400 for equal bounds).

## Evidence

Smithy model: ListProfileTimesInput.startTime/endTime are @required timestamps; ValidationException (400) is modeled. The service's interval sentence ("must be before end time") is recorded for this very operation in the open behavioral issue 20261023T123333Z-codeguruprofiler-..., and the emulator implements the strict `>=` form of the same rule in src/ops/get_recommendations.rs. Handler with the gap: src/ops/list_profile_times.rs, `if range_end < range_start { ... }` — equality falls through. Tests: tests/all/profile_times.rs covers out-of-range rows but nothing asserts the equal-bound case, so this is uncovered.

## Steps to Reproduce

1. Create profiling group N and PostAgentProfile a profile for it.
2. GET /profilingGroups/N/profileTimes?startTime=2026-01-01T00:00:00Z&endTime=2026-01-01T00:00:00Z&period=PT5M
3. Observe HTTP 200 with an empty profileTimes list; the service's interval rule requires 400 ValidationException because startTime is not before endTime.
4. Compare with GET /internal/profilingGroups/N/recommendations?startTime=T&endTime=T, which this emulator refuses with 400.
