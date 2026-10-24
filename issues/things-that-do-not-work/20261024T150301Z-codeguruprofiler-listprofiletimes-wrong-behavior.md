Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# ListProfileTimes reports aggregated profile times immediately for profiles AWS has not aggregated yet

_Filed: 2026-10-24T15:03:01Z_
_Operation: ListProfileTimes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: ListProfileTimes lists the AGGREGATED profiles a group holds for the requested aggregation period. Aggregation in CodeGuru Profiler is asynchronous and lags well past a single request (the GetProfile doc: "Amazon CodeGuru Profiler collects posted agent profiles for a profiling group into aggregated profiles", at periods "aligned to UTC"), so right after a successful PostAgentProfile the window containing that post has no aggregated profile yet and ListProfileTimes answers with an empty `profileTimes` list (and no nextToken), while DescribeProfilingGroup already shows profilingStatus.latestAgentProfileReportedAt set.

Actual: the emulator derives one ProfileTime per posted agent profile by flooring the profile's receipt time to the requested period and reports it on the very next call, so a single PostAgentProfile instantly materialises an "aggregated profile" for PT5M/PT1H/P1D. A client that polls ListProfileTimes until an aggregate appears (the normal pattern, because aggregation is pending for minutes) sees it immediately and then fetches a profile AWS would not have, so the pending-aggregation code path is never exercised and a 0-entry page becomes a 1-entry page.

This is the ListProfileTimes face of the same state-model problem the already-open GetProfile issue describes (posted agent profiles are not kept separate from aggregated profiles); a fixer should close both together, but the observable divergence here is on a different operation and is not covered by that issue's causes (which are GetProfile's 200-instead-of-404 and ListProfileTimes' interval-message wording).

## Evidence

Spec/established behavior: ListProfileTimes' `period` is the AggregationPeriod "during which an aggregation profile collects posted agent profiles for a profiling group" (`smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#ListProfileTimesRequest")`), i.e. the listed entries are aggregates, not posts. Recorded AWS behavior cited in the open sibling issue 20261023T123333Z-codeguruprofiler-codeguru-profiler-getpolicy-404s-on-a-policy-less (Cause 3, traces codeguruprofiler.get-profile-no-aggregated-data-yet / get-profile-without-data-not-found) shows aggregation still unresolved after ~200s of polling real AWS following a successful PostAgentProfile.

Handler/state: services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/list_profile_times.rs:239-286 filters `group.aggregated_profile_starts(period_seconds)`; src/state.rs `ProfilingGroup::aggregated_profile_starts` maps EVERY stored posted profile's `received_at` through `aggregation_window_start` and treats "every window holding at least one posted profile" as one available aggregated profile — there is no aggregation delay or separate aggregate store.

Tests: tests/all/profile_times.rs asserts the current immediate-visibility behavior (posted profile → ProfileTime in the same test), so the divergent behavior is pinned by the emulator's own suite and no test covers the pending-aggregation (empty page) case.

## Steps to Reproduce

1. Boot the emulator and point a codeguruprofiler client at it.
2. CreateProfilingGroup(profilingGroupName="pg-aggr", clientToken=<uuid>, agentOrchestrationConfig={"profilingEnabled": true}).
3. PostAgentProfile(profilingGroupName="pg-aggr", agentProfile=<valid profile JSON>, contentType="application/json") — 204.
4. ListProfileTimes(profilingGroupName="pg-aggr", startTime=now-5m, endTime=now+5m, period="PT5M") — the emulator returns one profileTimes entry for the window containing the post; AWS returns an empty profileTimes list until aggregation completes.
5. Repeat with period="PT1H" and period="P1D" — each also reports an entry immediately.
