Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 00afbba9508d973a7de56baf49cf83dffec26bea

# stated_instant hand-rolls the java.util.Date refusal rendering (weekday table, month table, RFC-3339 string surgery) that chrono — already a direct dependency of this crate — formats

_Filed: 2026-10-27T10:50:13Z_
_Operation: GetService | Class: maintainability_

> _Note: this issue was identified against the code base at commit `00afbba9508d973a7de56baf49cf83dffec26bea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::stated_instant` is the single renderer every time-ranged read's `ValidationException` message goes through (`time_range_refusal`, used by GetService, ListServices, ListServiceStates, ListServiceOperations, ListServiceDependencies, ListServiceDependents, ListEntityEvents and ListAuditFindings). It produces the `EEE MMM dd HH:mm:ss UTC yyyy` form by hand: it formats the instant as RFC-3339 with `aws_smithy_types`, splits the result on 'T' and '-', trims the trailing 'Z' and any fractional part, indexes a 12-entry month-abbreviation table with the parsed month number, and derives the weekday from a 7-entry table indexed by `secs.div_euclid(86_400).rem_euclid(7)`.

`chrono` is already a direct dependency of THIS crate (declared in its own Cargo.toml) and is already used in it for calendar arithmetic in the budget-report handler, so the whole span is `chrono::DateTime::from_timestamp(at.secs(), 0)` formatted with `%a %b %d %H:%M:%S UTC %Y`, which emits exactly the same string (`Sun Sep 06 02:00:00 UTC 2026`). The hand-rolled version re-derives the weekday and the month-name mapping that the crate's own dependency owns, and it is the same calendar knowledge the crate explicitly refused to re-derive 400 lines later ("The calendar arithmetic is `chrono`'s — `checked_add_months` ... so no month-length or leap-year table is re-derived here"). Replacing the span changes no response and no test: it is the same rendering for the same instants.

## Evidence

Hand-rolled span — services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:1918-1961:
```
/// One instant as this service states it in a refusal: `EEE MMM dd HH:mm:ss UTC yyyy`
/// (`Sun Sep 06 02:00:00 UTC 2026`), the `java.util.Date` rendering of a UTC instant.
...
/// weekday is not part of that rendering, and it needs no calendar either: day zero of the
/// epoch — 1970-01-01 — was a Thursday, so it is the day count modulo 7.
fn stated_instant(at: &DateTime) -> String {
    /// Indexed by the epoch day count modulo 7, which is 0 on Thursday 1970-01-01.
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    /// The month abbreviations `Date.toString` uses, in month order.
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Ok(rendered) = at.fmt(::aws_smithy_types::date_time::Format::DateTime) else { ... };
    let Some((date, time)) = rendered.split_once('T') else { ... };
    ...
    let weekday = WEEKDAYS[at.secs().div_euclid(86_400).rem_euclid(7) as usize];
    format!("{weekday} {month} {day} {hour_minute_second} UTC {year}")
}
```

The crate API that replaces it — services/service-com-amazonaws-applicationsignals-applicationsignals/Cargo.toml:34-39:
```
# Calendar arithmetic: a calendar-month SLO evaluation interval is aligned with the
# calendar ... only the month-length and leap-year rules ...
chrono = { version = "0.4.31", default-features = false, features = ["std"] }
```
and already used for calendar work in this crate — src/ops/batch_get_service_level_objective_budget_report.rs:296-302:
```
/// The calendar arithmetic is `chrono`'s — `checked_add_months`, which clamps a
/// day-of-month the target month does not have — so no month-length or leap-year table is
/// re-derived here.
...
    let anchored = ::chrono::DateTime::from_timestamp(anchor, 0)?;
```

The only consumer of the span — src/state.rs:1907-1915:
```
pub fn time_range_refusal(start_time: &DateTime, end_time: &DateTime) -> Option<String> {
    ...
    Some(format!(
        "Invalid start time {} and end time {} provided: start time must be before end time",
        stated_instant(start_time),
        stated_instant(end_time),
    ))
```

Test coverage that pins the rendering byte-for-byte (so a refactor is verifiable and must leave responses unchanged) — tests/all/time_ranged_reads.rs:327-356, `a_range_that_does_not_run_forwards_is_refused`, four rows over every time-ranged read:
```
            message: "Invalid start time Sun Sep 06 02:00:00 UTC 2026 and end time \
                      Sat Sep 05 23:13:20 UTC 2026 provided: start time must be before end time",
...
            message: "Invalid start time Sat Jan 01 00:00:00 UTC 2000 and end time \
                      Fri Dec 31 00:00:00 UTC 1999 provided: start time must be before end time",
```

## Steps to Reproduce

1. Open services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs and read `stated_instant` at line 1929: the WEEKDAYS table (line 1931), the MONTHS table (lines 1933-1935), the RFC-3339 split/trim surgery (lines 1937-1952) and the modulo-7 weekday at line 1960.
2. Open the same crate's Cargo.toml line 39 and see `chrono = { version = "0.4.31", default-features = false, features = ["std"] }` already declared as a direct dependency.
3. Open src/ops/batch_get_service_level_objective_budget_report.rs lines 296-302 and see the crate already using `::chrono::DateTime::from_timestamp` precisely so that no month-length/leap-year table is re-derived — the rule the span in state.rs breaks for the weekday and month name.
4. Run the existing `cargo test -p service-com-amazonaws-applicationsignals-applicationsignals time_ranged_reads` (tests/all/time_ranged_reads.rs:327-356) to see the four rows that pin the exact rendering; formatting the same instant with chrono's `%a %b %d %H:%M:%S UTC %Y` yields the identical strings, so the duplication can be removed with no response or test change.
