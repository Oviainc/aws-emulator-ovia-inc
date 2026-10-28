Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 35520d9c06247da719cc1093e5db81eb1d87c7db

# GetAutomationEvent reports an event outside the retrieval window its three sibling event readers treat as absent

_Filed: 2026-10-28T18:17:13Z_
_Operation: GetAutomationEvent | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `35520d9c06247da719cc1093e5db81eb1d87c7db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other reader of the event store applies the crate's `EVENT_RETRIEVAL_WINDOW` floor before it reports anything: `ListAutomationEventSteps` answers the modelled 404 for an event created before the floor, and `ListAutomationEvents` / `ListAutomationEventSummaries` take the later of the caller's start bound and the floor and then drop everything older. `GetAutomationEvent` applies no floor at all — it resolves the id straight out of the store, settles it and reports the whole event.

So one and the same stored event is simultaneously absent (404 from `ListAutomationEventSteps`, invisible to both event listings however far back `startTimeInclusive`/`startDateInclusive` points) and present (200 with every member from `GetAutomationEvent`). That contradicts the rule the crate states on the constant itself — the window is where "an event created before that window is outside every query's reach" — and it contradicts the crate's own test assertion that the two readers agree about one event (`tests/all/automation_event_steps.rs:405-406`: "The event reader and the step reader agree on what the run reached"). `GetAutomationEvent` is the one query of the four that the window is not in force for.

The window itself is not in question here: it is already a crate constant, documented from the model, implemented in shared helpers (`effective_window_start` / `within_window`) and pinned by tests on the three siblings. The defect is that the fourth reader of the same store does not consult it.

## Evidence

The rule the crate states on the constant — `src/state.rs:211-216`:
```
/// How far back an automation event stays reportable. `ListAutomationEvents`
/// documents "You can retrieve events that were created within the past year"
/// (`smithy_query(query="doc",shape="ListAutomationEvents")`), so an event
/// created before that window is outside every query's reach however far back
/// the caller's `startTimeInclusive` points.
pub const EVENT_RETRIEVAL_WINDOW: Duration = Duration::from_secs(365 * 24 * 60 * 60);
```

The handler that consults nothing — `src/ops/get_automation_event.rs:190-213` (the whole body between the lock and the response; there is no `effective_window_start` / `within_window` call anywhere in the file):
```
        let mut guard = state.lock();
        // The service-wide enrollment gate, AHEAD of the event lookup: an
        // account that is not opted in is refused the whole call, so it is never
        // told "no such event" when what it has to do is enroll. See
        // `crate::state::…::opt_in_required`.
        if let Some(refusal) = guard.opt_in_required() {
            return Err(
                GetAutomationEventErrorModeled::OptInRequiredException(refusal).into(),
            );
        }
        let Some(event) = guard.events.get_mut(&input.event_id) else {
            return Err(GetAutomationEventErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::RESOURCE_NOT_FOUND_MESSAGE.into()),
                },
            )
            .into());
        };
        // The only write a read makes: the status transition the clock already
        // owed this event, which was due at its stored instant whether or not
        // anybody called. Nothing here is advanced BECAUSE the event was read.
        event.settle(::std::time::SystemTime::now());

        Ok(GetAutomationEventOutput {
```

The sibling that does, on the same store and the same `eventId` input — `src/ops/list_automation_event_steps.rs:206-227`:
```
        let now = ::std::time::SystemTime::now();
        // "You can only list steps for events created within the past year": an
        // event created before that floor is out of this operation's reach, so
        // it is as absent as one the store never held.
        let window_start = crate::state::effective_window_start(None, now);
...
        let Some(event) = guard.events.get_mut(&input.event_id) else {
            return Err(not_found());
        };
        if !crate::state::within_window(event.created_timestamp, Some(window_start), None) {
            return Err(not_found());
        }
```
and the two listings — `src/ops/list_automation_events.rs:232-238` + `:259-265`:
```
        // Only events created inside the retrieval window are reachable, so the
        // window this call reports over starts at the later of the caller's
        // `startTimeInclusive` and that floor.
        let window_start = crate::state::effective_window_start(
            input.start_time_inclusive,
            ::std::time::SystemTime::now(),
        );
...
                .filter(|event| {
                    crate::state::within_window(
                        event.created_timestamp,
                        Some(window_start),
                        input.end_time_exclusive,
                    )
                })
```
`src/ops/list_automation_event_summaries.rs:292-295`:
```
        // Only events created inside the retrieval window are reachable, so the
        // window this call summarises over starts at the later of the caller's
        // `startDateInclusive` and that floor.
        let window_start = crate::state::effective_window_start(start_date, now);
```

Test coverage: the three siblings' behaviour at this boundary is pinned at the same seam a reproduction uses — `tests/all/automation_event_steps.rs:560-592`:
```
/// A well-formed `eventId` no stored event carries is the modelled
/// `ResourceNotFoundException` (404) — and so is an event created before the
/// retrieval window, whose steps are no longer listable.
...
    let outside = stored_event(6, "UpgradeEbsVolumeType", base - window - 60, step(6));
...
        ("just-outside-the-edge", outside.event_id.clone(), None),
        ("far-outside", long_outside.event_id.clone(), None),
```
(and the equivalents at `tests/all/automation_events.rs:568-643` and `tests/all/automation_event_summaries.rs:878-955`). A grep for `GetAutomationEvent` across `tests/` shows it is only ever called on freshly started events (`tests/all/automation_event_and_tagging.rs:234`, `:293`, `:355`, `tests/all/automation_event_completion.rs:76`, `tests/all/automation_event_rollback.rs:128`, …), so the behaviour reported here is pinned by nothing — while `tests/all/automation_event_steps.rs:405-406` asserts the opposite principle for a reachable event: "The event reader and the step reader agree on what the run reached".

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Seed one event created before the retrieval window, exactly as tests/all/automation_event_steps.rs:573 does: take `state::service_state(&engine)`, lock it and `events.insert(..)` a StoredEvent whose `created_timestamp` is `now - (EVENT_RETRIEVAL_WINDOW + 60s)`, carrying one step.
3. Call ListAutomationEventSteps with {"eventId": <that id>} — 404 ResourceNotFoundException, from src/ops/list_automation_event_steps.rs:225-227 ("it is as absent as one the store never held"); tests/all/automation_event_steps.rs:591 pins this.
4. Call ListAutomationEvents with {"startTimeInclusive": 0} and ListAutomationEventSummaries with {"startDateInclusive": "1970-01-01"} — neither reports or counts the event, because both floor the window at src/ops/list_automation_events.rs:235-238 and src/ops/list_automation_event_summaries.rs:295.
5. Call GetAutomationEvent with {"eventId": <the same id>} — 200 with the full event (eventId, eventType, eventStatus, createdTimestamp, estimatedMonthlySavings …). The one reader that applies no floor hands back an event its three siblings answer as absent.
