Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# profilingStatus.latestAggregatedProfile is hardcoded absent, so no read reports the aggregated profile the store already derives

_Filed: 2026-10-25T21:25:21Z_
_Operation: DescribeProfilingGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ProfilingGroup::profiling_status()` is the single builder of the `profilingStatus` container every read of a group answers with (`DescribeProfilingGroup`, `UpdateProfilingGroup`, the described form of `ListProfilingGroups`, and a replayed `CreateProfilingGroup`). It fills two of the three modelled members from stored state and hardcodes the third, `latestAggregatedProfile`, to `None` on every path — so no request can ever observe it, even for a group whose posted profiles the SAME crate already reports as aggregated profiles through `ListProfileTimes`.

The crate is not missing the computation: `ProfilingGroup::aggregated_profile_starts(period_seconds, at)` derives exactly the aggregated-profile windows (start instants) from the stored profiles, and `crate::state::AGGREGATION_PERIODS` holds the three modelled `AggregationPeriod` values with their lengths — the two members `AggregatedProfileTime` consists of (`start`, `period`). `ListProfileTimes` calls that helper; `profiling_status()` skips it and reports nothing.

The hardcoded `None` also makes a branch of the crate's own create-payload rule dead: `has_profiling_activity()` — the predicate that decides whether a create's payload carries `profilingStatus` at all — tests `status.latest_aggregated_profile.is_some()`, a disjunct that can never be true, so the rule it encodes ("an aggregated profile counts as profiling activity") is unreachable code rather than behaviour.

Observable effect: create a group, post a profile, let the aggregation fall due (the state the crate models with `aggregated_at`), then `ListProfileTimes` reports the aggregated window while `DescribeProfilingGroup` still omits `latestAggregatedProfile` from `profilingStatus` — a client reading the group's status can never learn the aggregation period and start time of its most recent aggregated profile.

## Evidence

Model, `ProfilingStatus` (smithy_query query="shape", shape="ProfilingStatus"): "Profiling status includes information about the last time a profile agent pinged back, the last time a profile was received, and the aggregation period and start time for the most recent aggregated profile." and its member `latestAggregatedProfile`: "An AggregatedProfileTime object that contains the aggregation period and start time for an aggregated profile."

The code that never reports it — services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/state.rs:789-804:
```
    /// The group's `profilingStatus`: each member present only once the activity it names
    /// has happened.
    fn profiling_status(&self) -> ProfilingStatus {
        ProfilingStatus {
            latest_agent_profile_reported_at: self.latest_agent_profile_reported_at,
            // `latestAggregatedProfile` is ONE `AggregatedProfileTime`, i.e. one
            // aggregation period, while the service aggregates a group's posted
            // profiles at all three periods at once (see
            // [`ProfilingGroup::aggregated_profile_starts`], which derives the
            // aggregated profiles for the period a caller asks about). There is no
            // stored fact saying which period to report here, so this stays absent
            // rather than picking one arbitrarily.
            latest_aggregated_profile: None,
            latest_agent_orchestrated_at: self.latest_agent_orchestrated_at,
        }
    }
```
(the comment itself admits the member is never reported, and names the helper that computes what it would carry)

The dead disjunct it creates — src/state.rs:809-814:
```
    fn has_profiling_activity(&self) -> bool {
        let status = self.profiling_status();
        status.latest_agent_profile_reported_at.is_some()
            || status.latest_agent_orchestrated_at.is_some()
            || status.latest_aggregated_profile.is_some()
    }
```

The helper the crate already has — src/state.rs:950-970:
```
    /// The start instants of the aggregated profiles this group holds for an
    /// aggregation period `period_seconds` long, as of `at`, ASCENDING and distinct.
    ...
    /// every window holding at least one AGGREGATED
    /// posted profile is exactly one available aggregated profile, reported by its
    /// window start.
    pub fn aggregated_profile_starts(&self, period_seconds: i64, at: DateTime) -> Vec<DateTime> {
```
and the sibling handler that calls it — src/ops/list_profile_times.rs:255-256:
```
        let mut starts: Vec<::aws_smithy_types::DateTime> = group
            .aggregated_profile_starts(period_seconds, crate::state::now())
```
plus the period table — src/state.rs:34-38 `pub const AGGREGATION_PERIODS: &[(&str, i64)] = &[("PT5M", 5 * 60), ("PT1H", 60 * 60), ("P1D", 24 * 60 * 60)];`

The read path that serves the hollow container — src/ops/describe_profiling_group.rs:159-161 (`profiling_group: group.description()`) via src/state.rs:759 (`profiling_status: Some(self.profiling_status())`).

Test coverage: tests/all/wire_response_shape.rs:665-755 counts `profilingStatus` members for rows where the posted profile's aggregation is NOT yet due, and tests/all/profiling_group_lifecycle.rs:488-506 covers groups with no activity at all — no test states a reason for the member being absent once an aggregation IS due (tests/all/pending_aggregation.rs:320-333 completes the aggregation and only asserts GetProfile).

## Steps to Reproduce

1. CreateProfilingGroup N, then PostAgentProfile for N.
2. Let the aggregation fall due (as tests/all/pending_aggregation.rs:320 does with `harness.complete_aggregation(name)`).
3. GET /profilingGroups/N/profileTimes?...&period=PT5M — the aggregated window start is reported, so the store holds an aggregated profile.
4. GET /profilingGroups/N — `profilingStatus` carries only `latestAgentProfileReportedAt` (and `latestAgentOrchestratedAt` if an agent pinged); `latestAggregatedProfile` is absent, because src/state.rs:801 hardcodes `None`.
