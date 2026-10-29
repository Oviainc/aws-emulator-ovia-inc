Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 527cd3726300db8090bc233c982f66d05e13948e

# GetRecommendations reports a profile span over profiles whose aggregation is not due, skipping the is_aggregated gate every sibling profile read applies

_Filed: 2026-10-28T15:35:57Z_
_Operation: GetRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `527cd3726300db8090bc233c982f66d05e13948e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRecommendations` derives `profileStartTime`/`profileEndTime` with `ProfilingGroup::profile_span_in`, the one profile-reading helper in this crate that does NOT gate on `AgentProfile::is_aggregated` — it is not even handed an instant to judge aggregation against. Every other read of stored agent profiles in the same crate does gate on it: `GetProfile` through `latest_profile_in(start, end, now())`, `ListProfileTimes` through `aggregated_profile_starts(period_seconds, now())`, and `BatchGetFrameMetricData` by explicitly splitting aggregated profiles from pending ones (a pending step becomes an `unprocessedEndTimes` entry instead of a value).

The consequence is observable and inconsistent: post an agent profile and immediately call `GetRecommendations` over a window containing that receipt instant. The analysis is reported as being about `[receipt, receipt]` — a profile that `GetProfile` simultaneously answers 404 `No profiles found for the requested time range!` for and that `ListProfileTimes` reports no profile time for. The same read, issued a moment later once the aggregation falls due (`AGGREGATION_PASS_MILLIS` after the finest window closes), is the one that should first expose that profile.

The fix is local and needs no new concept: pass `crate::state::now()` into the span computation and skip a profile that `is_aggregated` says is not collected yet, exactly as the three sibling reads do. The existing `GetRecommendations` suite stamps its profiles' receipt times to 2020 (`tests/all/recommendations_analysis.rs:271-281`), so every row there is long aggregated and stays green.

## Evidence

The skipped gate — `src/state.rs:723-731`:
```
    /// Whether, as of `at`, this posted profile is part of an aggregated profile.
    ///
    /// The operations that report AGGREGATED profiles (`GetProfile`, `ListProfileTimes`)
    /// count a posted profile only once this holds: until then the group has received
    /// the profile — `profilingStatus.latestAgentProfileReportedAt` says so — and no
    /// aggregated profile covers it yet.
    pub fn is_aggregated(&self, at: DateTime) -> bool {
        instant_key(self.aggregated_at()) <= instant_key(at)
    }
```

The path that skips it — `src/state.rs:1201-1208` (`profile_span_in`, no `at` parameter, no `is_aggregated` test):
```
    pub fn profile_span_in(&self, start: DateTime, end: DateTime) -> Option<(DateTime, DateTime)> {
        let (lower, upper) = (instant_key(start), instant_key(end));
        let mut span: Option<(DateTime, DateTime)> = None;
        for profile in &self.profiles {
            let at = instant_key(profile.received_at);
            if at < lower || at > upper {
                continue;
            }
```
and its one caller — `src/ops/get_recommendations.rs:247-252`:
```
        // The span the analysed profiles cover: the earliest and latest receipt times the
        // group retains inside the requested window. A window holding no profile covers
        // nothing, so the requested bounds stand as the empty analysed span.
        let (profile_start_time, profile_end_time) = group
            .profile_span_in(input.start_time, input.end_time)
            .unwrap_or((input.start_time, input.end_time));
```

The siblings that apply the gate:
- `src/ops/get_profile.rs:737-743`: `group\n                .latest_profile_in(start, end, crate::state::now())\n                .ok_or_else(|| ... NO_PROFILES_IN_TIME_RANGE ...)` — and `src/state.rs:1293-1296`: `self.profiles.iter().rfind(|profile| { ... profile.is_aggregated(at) ... })`.
- `src/ops/list_profile_times.rs:255-256`: `let mut starts: Vec<::aws_smithy_types::DateTime> = group\n            .aggregated_profile_starts(period_seconds, crate::state::now())` — and `src/state.rs:1151`: `.filter(|profile| profile.is_aggregated(at))`.
- `src/ops/batch_get_frame_metric_data.rs:639-643`: `if profile.is_aggregated(now) { aggregated.push((step, profile)); } else { pending[step] = true; }`.

The crate's own stated rule for this state — `tests/all/pending_aggregation.rs:7-16`:
```
//! The rule every case here asserts: `GetProfile` and `ListProfileTimes` report
//! AGGREGATED profiles, and a posted agent profile is not one at the instant it is
//! received — ... Until then the window is
//! covered by no aggregated profile, so `GetProfile` answers its 404
//! `ResourceNotFoundException` with the fixed no-data text and `ListProfileTimes` reports
//! no profile time for it
```

Test coverage of the affected behaviour: `tests/all/recommendations_analysis.rs::the_reported_profile_span_is_the_retained_profiles_inside_the_window` pins the span rule only for profiles stamped to 2020 (`tests/all/recommendations_analysis.rs:271-281`, `let base: i64 = 1_590_969_600; ... harness.stamp_receipt_times(name, &received_at);`), so the pending-aggregation case is pinned by no test; `tests/all/pending_aggregation.rs` pins it for `GetProfile`/`ListProfileTimes` only.

## Steps to Reproduce

1. Create a profiling group: POST /profilingGroups?clientToken=tok with {"profilingGroupName":"pending-span"}.
2. Post a valid agent profile: POST /profilingGroups/pending-span/agentProfile with Content-Type: application/json and body {"start":1600000000000}. It answers 204, and the profile's aggregation is not due until the finest (PT5M) window closes plus AGGREGATION_PASS_MILLIS.
3. Immediately GET /internal/profilingGroups/pending-span/recommendations?startTime=<now-1m>&endTime=<now+1m>: profileStartTime and profileEndTime report the pending profile's receipt instant.
4. Immediately GET /profilingGroups/pending-span/profile?startTime=<now-1m>&endTime=<now+1m>: 404 ResourceNotFoundException with "No profiles found for the requested time range!", and GET /profilingGroups/pending-span/profileTimes?startTime=<now-1m>&endTime=<now+1m>&period=PT5M reports an empty profileTimes — the same profile, invisible to both.
5. Compare src/ops/get_recommendations.rs:250-252 (no instant, no is_aggregated) with src/ops/get_profile.rs:738, src/ops/list_profile_times.rs:256 and src/ops/batch_get_frame_metric_data.rs:639, all of which pass crate::state::now() and gate on AgentProfile::is_aggregated.
