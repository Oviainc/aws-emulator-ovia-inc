Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c57727bdfc46ee809d2c1cc52508769e6ed5546

# A PT1H/P1D window is reported as an available aggregated profile ~5 minutes after a post, while that window is still open

_Filed: 2026-10-29T10:53:39Z_
_Operation: ListProfileTimes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c57727bdfc46ee809d2c1cc52508769e6ed5546`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListProfileTimes` reports one profile time per aligned window of the REQUESTED `period` that holds an "aggregated" posted profile. Whether a posted profile counts as aggregated is decided by `AgentProfile::is_aggregated`, which is computed from the FINEST aggregation period only (`aggregated_at` calls `finest_aggregation_period_seconds()`, i.e. PT5M, + `AGGREGATION_PASS_MILLIS`). `ProfilingGroup::aggregated_profile_starts(period_seconds, at)` then applies that period-independent gate and floors the receipt time to the period the request asked about.

Consequence: for `period=PT1H` or `period=P1D` the window is announced as an existing aggregated profile as soon as the five-minute window containing the post has closed (plus the pass), even though the hourly/daily window is still collecting. With `period=P1D`, a single post at 00:02 makes ListProfileTimes report today's 00:00 as an available daily aggregated profile from ~00:05 until midnight — roughly 24 hours of reporting an aggregate whose own window has not closed. That contradicts the rule this crate states for itself in two places (`aggregated_at`'s doc: "A window still open cannot have been aggregated"; the suite's own rule: "a window still open is covered by no aggregated profile and contributes no profile time — however many profiles it holds, whatever period"), and the period-general claim in `aggregated_profile_starts`' own doc that "every window holding at least one AGGREGATED posted profile is exactly one available aggregated profile".

This is NOT a request to remove the aggregation delay that the earlier fixed issue `20261024T150301Z-codeguruprofiler-listprofiletimes-wrong-behavior` introduced (that issue's step 5 named `PT1H` and `P1D` explicitly); the delay exists now, but it is derived from the finest window instead of from the window being reported, so it is the residual for the two coarser periods. The crate already holds everything the per-period instant needs (`aggregation_window_start(received_at, period_seconds)`, the period's own length in `AGGREGATION_PERIODS`, and `AGGREGATION_PASS_MILLIS`); the gate simply never receives the period it is answering about. The same period-independent gate is used for `BatchGetFrameMetricData`'s steps (`targetResolution=P1D` marks a step for the current, still-open day as processed), but that operation already carries an open `wrong-behavior` issue, so it is named here only as the shared root cause.

The existing pending-aggregation test does not catch this because it places its receipts relative to the FINEST window boundary (`pending_window_start()` = `window_start(now, finest) + finest`), so its PT1H rows are pending for the finest reason and never reach the hour boundary.

## Evidence

The gate is finest-period only — src/state.rs:231-238:
```
/// When the aggregated profile covering a profile received at `received_at` becomes
/// available: the end of the finest aggregation window containing it, plus
/// [`AGGREGATION_PASS_MILLIS`].
///
/// A window still open cannot have been aggregated — the service is still collecting
/// profiles into it — so this is never earlier than the window's end.
pub fn aggregated_at(received_at: DateTime) -> DateTime {
    let period = finest_aggregation_period_seconds();
```
and src/state.rs:751-753:
```
    pub fn is_aggregated(&self, at: DateTime) -> bool {
        instant_key(self.aggregated_at()) <= instant_key(at)
    }
```

...yet it is applied to the window of the REQUESTED period — src/state.rs:1266-1276:
```
    /// ... every window holding at least one AGGREGATED
    /// posted profile is exactly one available aggregated profile, reported by its
    /// window start. A profile whose aggregation is not due yet
    /// ([`AgentProfile::is_aggregated`]) is in no aggregated profile, so it contributes
    /// no profile time.
    pub fn aggregated_profile_starts(&self, period_seconds: i64, at: DateTime) -> Vec<DateTime> {
        let mut starts: Vec<i64> = self
            .profiles
            .iter()
            .filter(|profile| profile.is_aggregated(at))
            .map(|profile| aggregation_window_start(profile.received_at, period_seconds).secs())
```

The handler, which asks only about the period the request named — src/ops/list_profile_times.rs:249-256:
```
        // The aggregated profiles the group holds for this period AS OF NOW, keeping
        // those whose start falls in the requested range. A profile the group has
        // received but whose aggregation is not due yet is in no aggregated profile, so
        // it contributes no profile time. ...
        let mut starts: Vec<::aws_smithy_types::DateTime> = group
            .aggregated_profile_starts(period_seconds, crate::state::now())
```

The rule the suite states for this operation, period-generally — tests/all/profile_times.rs:960-965:
```
/// `ListProfileTimes` lists AGGREGATED profiles, and a posted agent profile is not one
/// at the instant it is received: the service collects posted profiles into aggregated
/// profiles over windows aligned to UTC, so a window still open is covered by no
/// aggregated profile and contributes no profile time — however many profiles it holds,
/// whatever period, order or page size the request names.
```
and why that test cannot see the PT1H/P1D case — tests/all/profile_times.rs:816-826:
```
/// The start of the finest aggregation window that has NOT OPENED yet.
...
fn pending_window_start() -> i64 {
    let finest = service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::finest_aggregation_period_seconds();
    let now = DateTime::from(std::time::SystemTime::now()).secs();
    window_start(now, finest) + finest
}
```
Its rows include `period: "PT1H"` (tests/all/profile_times.rs:935-941, :950-956), but the receipts sit at the next FIVE-MINUTE boundary, so the assertion is satisfied by the finest gate and the hour boundary is never crossed.

Test coverage of the reported windows: tests/all/profile_times.rs::profile_times_are_the_aggregation_windows_of_the_stored_profiles (case table at :255-434, which places receipts a day or more in the past so every period's window has closed) and ::a_window_is_reported_only_once_the_aggregation_it_owes_is_due (:966) — no row posts into the current hour or the current day and asks for PT1H/P1D.

## Steps to Reproduce

1. Boot the emulator and point a codeguruprofiler client at it.
2. CreateProfilingGroup(profilingGroupName="pg-open-window", clientToken=<uuid>).
3. PostAgentProfile(profilingGroupName="pg-open-window", agentProfile=<valid profile document>, contentType="application/json") — 204. Note the receipt instant R (it is inside today's P1D window and inside the current PT1H window).
4. Wait until the five-minute window containing R has closed plus AGGREGATION_PASS_MILLIS (src/state.rs:229) — i.e. at most ~5 minutes — or seed the receipt ~6 minutes in the past the way tests/all/profile_times.rs::place_receipts does.
5. ListProfileTimes(profilingGroupName="pg-open-window", startTime=<start of today>, endTime=<start of tomorrow>, period="P1D"): the response carries one profileTimes entry whose start is today 00:00, although that P1D window is still open and still collecting — the state the crate's own `aggregated_at` doc (src/state.rs:235-236) and tests/all/profile_times.rs:960-965 say contributes no profile time.
6. Repeat with period="PT1H" and a range covering the current hour: the current hour's start is reported the same way, minutes into an hour that has not ended.
7. Contrast with period="PT5M": there the gate and the reported window coincide, which is the only case tests/all/profile_times.rs::a_window_is_reported_only_once_the_aggregation_it_owes_is_due exercises, because pending_window_start() (tests/all/profile_times.rs:822-826) is built from finest_aggregation_period_seconds().
