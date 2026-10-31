Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: fffc92c5e8ca342c0e1949ba8f9a858d2cea7932

# ListAutomationEvents derives its retrieval-window floor and its status settlement from two different clock readings, taken either side of the store lock

_Filed: 2026-10-28T22:34:18Z_
_Operation: ListAutomationEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fffc92c5e8ca342c0e1949ba8f9a858d2cea7932`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAutomationEvents` reads the wall clock TWICE in one request. The first read (before the store lock is taken) produces the retrieval-window floor that decides which events are reportable at all; the second read (after the lock) is handed to `EventStore::settle_due`, which decides which status transitions the clock has made due. So one response judges "is this event inside the past year" as of instant T1 and "what status does this event have now" as of a later instant T2, with the lock acquisition sitting between the two reads, so the gap between T1 and T2 is unbounded under contention.

Both of its sibling event readers bind ONE `now` per request and use it for exactly these two purposes: `ListAutomationEventSteps` binds `now` once and passes it to `effective_window_start` and to `event.settle`, and `ListAutomationEventSummaries` binds `now` once and passes it to `effective_window_start`, to `settle_due` and to the reported `TimePeriod`'s open end. `ListAutomationEvents` is the only one of the three that skips that normalisation.

The observable consequence is a boundary event: an event created in the sliver `[T1 - 1 year, T2 - 1 year)` is reported by this handler (its window membership was judged with the earlier, more permissive floor) while the two siblings called at the same wall-clock moment treat it as absent, and its status in that same response has been settled against the later instant. A single request should answer from one instant.

## Evidence

The two separate clock reads, with the lock between them — `src/ops/list_automation_events.rs:232-251`:
```
        // Only events created inside the retrieval window are reachable, so the
        // window this call reports over starts at the later of the caller's
        // `startTimeInclusive` and that floor.
        let window_start = crate::state::effective_window_start(
            input.start_time_inclusive,
            ::std::time::SystemTime::now(),
        );

        let mut guard = state.lock();
...
        // Before reporting, materialise the status transitions the clock has
        // already made due, so this reader and `GetAutomationEvent` agree on
        // every event's status and a status filter selects on the current one.
        guard.events.settle_due(::std::time::SystemTime::now());
```

The sibling that binds one `now` for the same two decisions — `src/ops/list_automation_event_steps.rs:206-233`:
```
        let now = ::std::time::SystemTime::now();
        // "You can only list steps for events created within the past year": an
        // event created before that floor is out of this operation's reach, so
        // it is as absent as one the store never held.
        let window_start = crate::state::effective_window_start(None, now);
...
        if !crate::state::within_window(event.created_timestamp, Some(window_start), None) {
            return Err(not_found());
        }
...
        event.settle(now);
```

and the other sibling, over the very same events — `src/ops/list_automation_event_summaries.rs:291-310`:
```
        let now = ::std::time::SystemTime::now();
        // Only events created inside the retrieval window are reachable, so the
        // window this call summarises over starts at the later of the caller's
        // `startDateInclusive` and that floor.
        let window_start = crate::state::effective_window_start(start_date, now);
...
        guard.events.settle_due(now);
```

The floor itself is derived from whatever instant it is handed — `src/state.rs:2799-2809`:
```
pub fn effective_window_start(
    requested: Option<::aws_smithy_types::DateTime>,
    now: SystemTime,
) -> ::aws_smithy_types::DateTime {
    let floor = ::aws_smithy_types::DateTime::from(
        now.checked_sub(EVENT_RETRIEVAL_WINDOW).unwrap_or(UNIX_EPOCH),
    );
```

Test coverage: the window floor is pinned only with events placed far outside it — `tests/all/automation_events.rs:575` (`let window = i64::try_from(EVENT_RETRIEVAL_WINDOW.as_secs()).expect("a representable window");`), `tests/all/automation_event_steps.rs:567` and `tests/all/automation_event_summaries.rs:474,885` — so no test places an event at the boundary or compares the two readers at one instant, and the double read reported here is pinned by nothing.

## Steps to Reproduce

1. Read `src/ops/list_automation_events.rs:235-238`: the retrieval-window floor is computed from `::std::time::SystemTime::now()` BEFORE `let mut guard = state.lock();` on line 240.
2. Read `src/ops/list_automation_events.rs:251`: `guard.events.settle_due(::std::time::SystemTime::now())` reads the clock a second time, after the lock.
3. Compare with `src/ops/list_automation_event_steps.rs:206` and `src/ops/list_automation_event_summaries.rs:291`, each of which binds `let now = ::std::time::SystemTime::now();` once and passes that single instant to both `effective_window_start` and the settle call.
4. Seed the store with an event whose `createdTimestamp` is exactly `EVENT_RETRIEVAL_WINDOW` before the present instant (as `tests/all/automation_events.rs:575` does for the far-outside case) and call ListAutomationEvents: whether that event appears depends on which of the two clock readings wins, and the status it is reported with was settled against the other reading.
5. Call ListAutomationEventSteps (or ListAutomationEventSummaries) for the same event at the same moment: it judges the same boundary against a single instant, so the two readers can disagree about an event neither request changed.
