Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# GetProfile's window filter compares stored receipt times as f64 epoch seconds, the comparison crate::state::instant_key exists to prevent

_Filed: 2026-10-25T11:23:36Z_
_Operation: GetProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::instant_key` is this crate's stated way to compare stored instants, and its own doc says why: an `f64` of current epoch seconds cannot represent nanosecond resolution, so two instants a fraction of a microsecond apart compare EQUAL. The doc then states the rule — ordering and inclusive-bound tests on stored timestamps go through `instant_key`.

`ProfilingGroup::latest_profile_in` — the single lookup `GetProfile` serves its 200 from — breaks that rule: it compares each stored `received_at` against the requested `start`/`end` through `as_secs_f64()`. The sibling helper immediately above it in the same file (`profile_span_in`, used by `GetRecommendations`) performs the same kind of inclusive-bound test and routes it through `instant_key`, and so does the sibling HANDLER `ListProfileTimes` for its own range filter.

Consequence on input the model allows (`startTime`/`endTime` are modelled `Timestamp`s with `date-time` format, i.e. sub-second precision is on the wire): a profile received up to ~240 ns outside the requested `[startTime, endTime]` compares equal to the bound and is served as though it were inside the window, and a profile that is inside by less than that is indistinguishable from one that is not. The same `f64` shortcut appears twice more in the same module on the same kind of test — `FindingsReport::overlaps` (the range filter behind `ListFindingsReports`) and the ordering in `ordered_findings_reports` / `oldest_evictable_profile` — so the stated rule is broken on every stored-timestamp comparison except the two that use `instant_key`.

## Evidence

The rule, src/state.rs:1723-1731:
```
/// One instant as an EXACTLY comparable key: whole seconds and sub-second nanoseconds.
///
/// `DateTime` carries nanosecond resolution, which an `f64` of current epoch seconds
/// cannot represent (its granularity there is coarser than a microsecond), so two
/// instants a fraction of a microsecond apart would compare equal. Ordering and
/// inclusive-bound tests on stored timestamps go through this instead.
pub fn instant_key(at: DateTime) -> (i64, u32) {
```

The code under it, src/state.rs:1069-1081 (`latest_profile_in`, the only lookup `GetProfile` uses):
```
        self.profiles.iter().rfind(|profile| {
            let received = profile.received_at.as_secs_f64();
            profile.is_aggregated(at)
                && start.is_none_or(|s| received >= s.as_secs_f64())
                && end.is_none_or(|e| received <= e.as_secs_f64())
        })
```

The sibling that obeys the rule, src/state.rs:988-991 (`profile_span_in`):
```
    /// Instants are compared through [`instant_key`], so two receipt times a fraction of
    /// a microsecond apart still order exactly.
    pub fn profile_span_in(&self, start: DateTime, end: DateTime) -> Option<(DateTime, DateTime)> {
        let (lower, upper) = (instant_key(start), instant_key(end));
```

The sibling HANDLER that obeys it, src/ops/list_profile_times.rs:223-228:
```
        // Instants are compared through `instant_key`, which is exact at nanosecond
        // resolution, rather than through `f64` epoch seconds.
        let (range_start, range_end) = (
            crate::state::instant_key(input.start_time),
            crate::state::instant_key(input.end_time),
        );
```

The same lapse twice more in the same module: src/state.rs:678-681 (`FindingsReport::overlaps`, `self.profile_end_time.as_secs_f64() >= start.as_secs_f64()`) and src/state.rs:1030-1035 (`ordered_findings_reports` sorts with `.as_secs_f64().total_cmp(..)`).

Caller: src/ops/get_profile.rs:291-297 — `group.latest_profile_in(start, end, crate::state::now())`. Test coverage: tests/all/profiling_group_lifecycle.rs (window rows) and tests/all/pending_aggregation.rs exercise whole-second bounds only, so nothing pins the sub-microsecond behaviour either way.

## Steps to Reproduce

1. Read src/state.rs:1723-1731 — `instant_key`'s doc states that ordering and inclusive-bound tests on stored timestamps go through it, because `f64` epoch seconds collapse instants less than a microsecond apart.
2. Read src/state.rs:1069-1081 — `latest_profile_in`, the lookup behind GetProfile's 200, compares `profile.received_at.as_secs_f64()` against `start`/`end` as `f64`.
3. Store a profile whose `received_at` is a few hundred nanoseconds AFTER an `endTime` bound (e.g. via the store, as tests/all/pending_aggregation.rs seeds receipts), then GET /profilingGroups/N/profile?startTime=<T0>&endTime=<T1> with T1 just before that receipt time.
4. The profile is served (the two f64 values compare equal), although its receipt time is outside the requested window; routing the comparison through `instant_key`, as `profile_span_in` and ListProfileTimes already do, excludes it.
