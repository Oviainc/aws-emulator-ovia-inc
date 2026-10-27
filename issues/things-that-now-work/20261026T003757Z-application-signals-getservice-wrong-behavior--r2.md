Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# The topology reads' window comparison uses as_secs_f64, contradicting the module's stated rule that every window comparison uses instant_nanos

_Filed: 2026-10-26T00:37:57Z_
_Operation: GetService | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::monitored_in_window` is the window test every topology read applies to decide whether a stored SLO is evidence of its entity over the requested period (`GetService`, `ListServices`, `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents`, `ListServiceStates`, `ListAuditFindings`). It compares instants through `DateTime::as_secs_f64`.

The module documents that it does not do that: `instant_nanos` exists precisely because `as_secs_f64` cannot tell two nearby instants apart, and `time_range_refusal` asserts that `instant_nanos` is "the same instant key every window comparison in this module uses" so that "a pair this refuses and a pair a projection reports over are ordered by one rule". `ChangeRecord::inside` and `latest_change_in_window` do use `instant_nanos`; `monitored_in_window` and `window_has_begun` do not.

The consequence is two different window precisions inside ONE read: `ListServiceStates` selects its discovered set with the float comparison and then picks the latest change inside the same window with the nanosecond comparison, so an SLO created a few hundred nanoseconds after `EndTime` can be counted as discovered by `ListServices`/`GetService` while `ListEntityEvents` correctly reports no change in that window.

## Evidence

The rule the module states — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:4105-4112`:

    /// One instant as a count of nanoseconds since the epoch, which is the precision a
    /// [`DateTime`] holds.
    ///
    /// Instants are compared through this rather than through `DateTime::as_secs_f64`, whose
    /// resolution at present epoch values is around a hundred nanoseconds: the window bounds are
    /// INCLUSIVE, so a comparison that cannot tell two instants apart would make the bound
    /// approximate, and two distinct instants would order as equal.

and restated as covering every such comparison — `src/state.rs:1816-1820`:

    /// The comparison is STRICT, because that is the rule the message states: ... It is made on the same instant
    /// key every window comparison in this module uses ([`instant_nanos`]), so a pair this
    /// refuses and a pair a projection reports over are ordered by one rule.

The projection's own window comparison breaks it — `src/state.rs:1889-1901`:

    fn monitored_in_window(
        slo: &ServiceLevelObjective,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> bool {
        slo.created_time.as_secs_f64() <= window_end.as_secs_f64() && window_has_begun(window_start)
    }

    fn window_has_begun(window_start: &DateTime) -> bool {
        window_start.as_secs_f64() <= DateTime::from(::std::time::SystemTime::now()).as_secs_f64()
    }

The change-event side of the same window does follow the rule — `src/state.rs:4193-4198`:

    /// Whether this write's instant falls inside the window, given as the pair of
    /// nanosecond bounds ([`instant_nanos`]), INCLUSIVE at both ends.
    fn inside(&self, (window_start, window_end): (i128, i128)) -> bool {
        let at = instant_nanos(&self.at);
        window_start <= at && at <= window_end
    }

…and so does `latest_change_in_window` (`src/state.rs:2510`: `let window = (instant_nanos(window_start), instant_nanos(window_end));`), which `observed_service_states` (`src/state.rs:2547-2559`) applies to the set `observed_services` selected with the float comparison.

Test coverage: `tests/all/time_ranged_reads.rs` and `tests/all/list_services.rs` exercise these windows at whole-second boundaries only, so the two precisions are never contrasted.

## Steps to Reproduce

1. Create an SLO whose SLI config names `KeyAttributes` `{"Type":"Service","Name":"checkout","Environment":"prod"}` (`POST /slo`); note its `CreatedTime` as seconds S with sub-second nanos N (the stored instant carries full `DateTime` precision).
2. Call `GET /services?StartTime=<S-3600>&EndTime=<S.(N-200ns)>` — an `EndTime` a few hundred nanoseconds BEFORE the SLO existed.
3. Expected (by the module's stated rule): the SLO is not evidence of the entity in that window, so `ServiceSummaries` is empty — the same answer `POST /events` gives, since `ChangeRecord::inside` compares with `instant_nanos`.
4. Actual: `monitored_in_window` compares the two instants through `as_secs_f64`, which cannot separate them, so the service is reported as discovered while `ListEntityEvents` over the same window reports no change for it.
5. Repeat with `POST /service/states` to see both comparisons inside one read: the discovered-set filter admits the entity, the latest-change filter (nanosecond) rejects it.
