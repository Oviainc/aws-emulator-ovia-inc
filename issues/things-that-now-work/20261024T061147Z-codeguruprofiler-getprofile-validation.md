Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# GetProfile accepts a reversed (or empty) startTime/endTime window instead of refusing it with ValidationException

_Filed: 2026-10-24T06:11:47Z_
_Operation: GetProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetProfile takes an optional `startTime`/`endTime` pair (both modeled timestamps, @httpQuery). The handler never checks that the window it was given is a window at all: `requested_window` in src/ops/get_profile.rs only refuses the "all 3 parameters" combination and an unoffered `period`; when both bounds are present it passes them straight to `ProfilingGroup::latest_profile_in(start, end)`.

Expected (real service): CodeGuru Profiler validates a time interval before looking anything up and refuses a non-interval with 400 ValidationException whose message names both bounds — "Interval start time (<startTime>) must be before end time (<endTime>)" — the same refusal the emulator already implements for GetRecommendations (src/ops/get_recommendations.rs rejects `start >= end` with exactly that sentence) and which the behavioral corpus records for ListProfileTimes.

Actual (emulator): `GetProfile?startTime=2026-01-02T00:00:00Z&endTime=2026-01-01T00:00:00Z` is accepted; the filter simply matches no stored profile, so the caller gets 404 ResourceNotFoundException "No profiles found for the requested time range!" instead of 400 ValidationException. A window whose bounds are equal is likewise accepted. A client that keys on the error class cannot tell a mis-ordered request (its own bug, not retryable) from a group that genuinely holds no profile in the window (retryable while aggregation catches up).

## Evidence

Spec/well-established behavior: the service's interval validator message is already encoded in this crate for GetRecommendations (src/ops/get_recommendations.rs, 'Interval start time (...) must be before end time (...)', asserted by tests/all/recommendations_analysis.rs rows including "a start equal to the end" -> 400), and the open behavioral issue 20261023T123333Z-... records the same sentence for ListProfileTimes. GetProfile models ValidationException (400) in the Smithy model. Handler: src/ops/get_profile.rs `requested_window` + the `latest_profile_in(start, end)` call in `handle` — no ordering check anywhere. Test coverage: tests/all/profiling_group_lifecycle.rs exercises many startTime/endTime rows for GetProfile but no reversed-window row, so nothing asserts this today. Distinct from the already-open GetProfile cause (serving a 200 for data AWS has not aggregated) — that is about the success path, this is the pre-lookup input check.

## Steps to Reproduce

1. Boot the emulator and create a profiling group N (CreateProfilingGroup).
2. Optionally PostAgentProfile for N so the group holds a profile.
3. GET /profilingGroups/N/profile?startTime=2026-01-02T00:00:00Z&endTime=2026-01-01T00:00:00Z
4. Observe 404 ResourceNotFoundException ("No profiles found for the requested time range!"); real AWS answers 400 ValidationException naming both bounds.
5. Repeat with startTime == endTime: also accepted, where the interval rule requires start strictly before end.
