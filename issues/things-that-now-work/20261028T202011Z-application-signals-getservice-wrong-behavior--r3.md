Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 31ee5846b7ac7b9895607a192ecc1e91c8df8e5f

# The "has this window begun" test re-reads the wall clock once per stored SLO inside the topology filter, so one answer can admit some entities and reject others on a condition that is a property of the request alone

_Filed: 2026-10-28T20:20:11Z_
_Operation: GetService | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `31ee5846b7ac7b9895607a192ecc1e91c8df8e5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every topology projection decides whether a stored SLO is evidence over the requested window with `monitored_in_window`, which ANDs two tests: the SLO's `CreatedTime` against the window end, and `window_has_begun(window_start)`. The second test does not look at the SLO at all — it is a property of the REQUEST ("has the period the caller asked about started yet?") — yet it is evaluated inside the per-SLO predicate and reads `SystemTime::now()` afresh on every evaluation.

So one `GetService`/`ListServices`/`ListServiceOperations`/`ListServiceDependencies`/`ListServiceDependents`/`ListServiceStates`/`ListAuditFindings` call reads the wall clock once per stored SLO (up to `MAX_SLOS` = 2000 times), and the request-level condition can answer differently for different records of the SAME response: for a window whose `StartTime` the server clock crosses while the filter is walking the store, the SLOs visited before the crossing are reported as unobserved and the ones visited after are reported as discovered. The answer is then not a function of the request and the store, and two reads of one window disagree about which entities exist — `ListServices` can report a strict subset of the services it holds, and `GetService` on one of the omitted identities answers with an undiscovered entity (empty `MetricReferences`, no `ServiceGroups`) while `GetService` on a sibling identity answers with a discovered one.

This is the one place in the crate where the clock is read outside a handler. The crate's own pattern is one sample per request, taken at the top of `handle` and passed down — every other operation does exactly that (19 call sites), and the sibling read that answers the SAME "has the period elapsed for this SLO" question, `BatchGetServiceLevelObjectiveBudgetReport`, passes its single sample in explicitly as `evaluated(interval_start, slo, at, now)`. `ListServiceStates` even takes such a sample (for the expiry pass) and then lets the projection below it read the clock again, per SLO.

## Evidence

THE PER-RECORD CLOCK READ — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:2049-2062, the ONLY `SystemTime::now()` in state.rs:
```
fn monitored_in_window(
    slo: &ServiceLevelObjective,
    window_start: &DateTime,
    window_end: &DateTime,
) -> bool {
    instant_nanos(&slo.created_time) <= instant_nanos(window_end) && window_has_begun(window_start)
}

/// Whether a window has begun: nothing has been observed in one that starts after now,
/// however far ahead it ends. Compared on the same instant key as every other window test
/// here ([`instant_nanos`]).
fn window_has_begun(window_start: &DateTime) -> bool {
    instant_nanos(window_start) <= instant_nanos(&DateTime::from(::std::time::SystemTime::now()))
}
```

THE CLAIM IT BREAKS — the doc immediately above it, src/state.rs:2043-2048:
```
/// Both bounds are compared on the module's one instant key ([`instant_nanos`]), the same
/// one [`time_range_refusal`] orders a requested range by and
/// [`ChangeRecord::inside`] places a recorded change by: a window this admits an SLO over
/// and a window a change read reports that SLO's writes in are therefore decided by one
/// comparison at one resolution, so an SLO created a fraction of a microsecond after the
/// window ended is outside it in every read.
```
It is not "one comparison": the right-hand operand is re-sampled per record, so the comparison is one per record against a different instant each time.

EVALUATED PER RECORD — src/state.rs:2354-2364 (`monitoring`, reached by `GetService` through `observed_service`):
```
        self.slos.values().filter(move |slo| {
            monitored_in_window(slo, window_start, window_end)
                && attributes_contain(entity_of(slo).key_attributes.as_ref(), key_attributes)
        })
```
and src/state.rs:2453-2457 (`observed_services`, reached by `ListServices` and by `ListServiceStates` through `observed_service_states`):
```
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_start, window_end))
```

THE PATTERN THE REST OF THE CRATE USES — one sample per request, passed in. src/ops/batch_get_service_level_objective_budget_report.rs:565 `let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());`, used at line 444 `let evaluated_window = interval.is_some_and(|(start, _)| evaluated(start, slo, at, now));`, whose doc (same file, lines 345-348) states the goal this projection misses: "The three instants are ordered on the module's one instant key (`crate::state::instant_nanos`), the same key those reads apply, so a creation a fraction of a microsecond after the window ends leaves the window empty here exactly as it leaves the entity undiscovered there."

THE SAME REQUEST ALREADY HOLDING A SAMPLE — src/ops/list_service_states.rs:232-239:
```
            let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());
            let mut store = state.lock();
            store.expire_instrumentation_configurations(&now);
            store.observed_service_states(
                &input.start_time,
                &input.end_time,
                input.attribute_filters.as_deref().unwrap_or_default(),
            )
```
The sample is taken and then not used for the window test the projection makes.

HANDLERS THAT TAKE NO SAMPLE AT ALL, so the projection has nothing to be given: src/ops/get_service.rs:177-195, src/ops/list_services.rs:175-202, src/ops/list_service_operations.rs:179-202, src/ops/list_service_dependencies.rs:182-205, src/ops/list_service_dependents.rs:180-203.

TEST COVERAGE: tests/all/time_ranged_reads.rs and tests/all/window_instant_resolution.rs pin the window rule only for windows far from the instant of the call (a wholly future window observes nothing, an inverted one is refused), so the per-record re-sampling is unpinned — hoisting the test to one sample per request leaves those assertions unchanged.

## Steps to Reproduce

1. Start the emulator with this crate mounted and create several SLOs (POST /slo) whose `SliConfig.SliMetricConfig.KeyAttributes` name distinct service entities, e.g. `svc-a` … `svc-z`.
2. Call GET /services?StartTime=<server now + a few milliseconds>&EndTime=<server now + 3600> — a window whose start the server clock crosses during the request (a client whose clock runs slightly ahead of the emulator's sends exactly this).
3. Because `window_has_begun` re-reads `SystemTime::now()` for every SLO the filter visits (src/state.rs:2060-2061), the SLOs walked before the clock crosses `StartTime` are dropped and the ones walked after are kept: the `ServiceSummaries` list is a partial, clock-dependent subset of the stored services, and repeating the same call answers a different subset.
4. POST /service with the `KeyAttributes` of one of the omitted services and the same window: it answers a `Service` with an empty `MetricReferences` list and no `ServiceGroups` (the undiscovered projection, src/state.rs:2405-2412), while the same call for a service that happened to be visited later in the walk answers the discovered projection.
5. Expected: the "has this window begun" test is a property of the request, so it is decided once per request against one clock sample — exactly as `BatchGetServiceLevelObjectiveBudgetReport` decides the same question with the single `now` its handler sampled (batch_get_service_level_objective_budget_report.rs:565, used at line 444) — and every entity of one answer is admitted or rejected by that one decision.
