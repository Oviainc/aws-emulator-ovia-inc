Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# Round reported instrumentation status event times to the nearest minute

_Filed: 2026-10-24T16:45:09Z_
_Operation: GetInstrumentationConfigurationStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`InstrumentationStatusEvent.Time` is documented as "The time when the status was reported, rounded to the nearest minute." The real service normalises each reported status event onto a minute boundary, which has two observable consequences: the `Time` a read reports is a whole minute, and two reports of the same status inside one minute collapse into a single event.

Expected: `ReportInstrumentationConfigurationStatus` with `Time = 2026-01-01T12:00:37Z` followed by `GetInstrumentationConfigurationStatus` reports one event whose `Time` is the minute-rounded instant; a second report of the same status at `12:00:51Z` adds no new event.

Actual: the emulator keys the status history on the exact instant (whole seconds plus nanoseconds) and renders `Time` straight back from that key, so it echoes `12:00:37Z` and records `12:00:51Z` as a second, distinct event. A client that pages the chronology sees sub-minute timestamps and a different event count than AWS returns for the same reports.

## Evidence

Spec: `smithy_query(query="shape", shape="InstrumentationStatusEvent")` — member `Time`: "The time when the status was reported, rounded to the nearest minute." Implementation: `crate::state` `status_event_key` / `status_event_instant` build the key as `(at.secs(), at.subsec_nanos(), status)` (src/state.rs ~2578-2594) from the report's raw `Time`, and `InstrumentationStatusHistory::page` reconstructs `time: DateTime::from_secs_and_nanos(key.0, key.1)` (~line 2557) — no minute normalisation anywhere; the write path is `src/ops/report_instrumentation_configuration_status.rs` (`store.record_instrumentation_status(..., &report.time, ...)`), the read path `src/ops/get_instrumentation_configuration_status.rs`. Tests: `tests/all/instrumentation_configuration_status.rs` reports whole-second timestamps and compares `event["Time"]` for exact equality with the value sent, so no test asserts the documented rounding.

## Steps to Reproduce

1. POST /create-instrumentation-configuration for a BREAKPOINT configuration (service `payments`, environment `prod`, SNAPSHOT, a CodeLocation)
2. POST /report-instrumentation-configuration-status with one report for that configuration, `Status: ACTIVE`, `Time` set to an instant 37 seconds past a minute boundary
3. POST /report-instrumentation-configuration-status again with the same status at 51 seconds past that same minute
4. POST /get-instrumentation-configuration-status for that configuration with `Status: ACTIVE`
5. Observe two events with sub-minute `Time` values; AWS reports one event on the minute boundary
