Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-signals.instrumentation-status-events-accumulate-multiple-transitions
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals.instrumentation-status-events-accumulate-multiple-transitions|_poll_until#9|AssertionError|raise AssertionError(f"condition never met within {timeout_s}s; last value={last!r}")
Issue-Commit: 05864672e4ab5cbf7bfedc8aa4e8ecdf6cc0144d

# A just-reported instrumentation status event is invisible to GetInstrumentationConfigurationStatus's default window, because the event is keyed at the NEAREST minute (up to 30s in the future) while the window closes at now

> _Note: this issue was identified against the code base at commit `05864672e4ab5cbf7bfedc8aa4e8ecdf6cc0144d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInstrumentationConfigurationStatus

## Description
Cause 1 of 1: ReportInstrumentationConfigurationStatus accepts a status report, but the following GetInstrumentationConfigurationStatus read over the documented default time window answers an empty `Events` list, so the reported status is not queryable. Failing check: application-signals.instrumentation-status-events-accumulate-multiple-transitions.

AWS: `ReportInstrumentationConfigurationStatus` (Smithy: POST /report-instrumentation-configuration-status, @idempotent, input `Configurations` -> `InstrumentationConfigurationStatusList` @length 1..100, output `UnprocessedStatusEvents` required) records a status event against the configuration a report names, and `GetInstrumentationConfigurationStatus` (Smithy: POST /get-instrumentation-configuration-status, @readonly, @paginated on `Events`, output `Events` -> `InstrumentationStatusEventList` required) reports those events. `GetInstrumentationConfigurationStatus.Status` is optional and `StartTime`/`EndTime` are optional: per the model docs the read "defaults to ACTIVE events from the last hour" when no status or time window is given. `InstrumentationStatusEvent.Time` is modelled as "The time when the status was reported, rounded to the nearest minute", and `ErrorCause` is "The error cause when the status is ERROR". A report whose identity (InstrumentationType + Service + Environment + SignalType + LocationHash) names a stored configuration is processed — it does NOT come back in `UnprocessedStatusEvents` — and the resulting event is therefore selected by a read whose window covers the instant the status was reported at. Reporting ACTIVE and then reading the default window is the operation pair's primary use ("Report ACTIVE periodically (for example, every minute) while instrumentation is running"), so an event reported inside the window must be in that window's answer. Likewise reporting ERROR and then READing status=ERROR returns an event carrying the reported `ErrorCause`, and the earlier ACTIVE events remain queryable under the ACTIVE filter: the per-status filters are independent chronologies, not one current-state field.

Emulator: the event's storage key is rounded to the NEAREST minute while the read's window end is NOT rounded, so the two use different time grids and the event lands outside its own window. In `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs`, `status_event_minute` computes `(secs + 30).div_euclid(60) * 60`, i.e. rounds the reported `Time` to the nearest minute boundary — which for any report whose second-of-minute is >= 30 is a boundary UP TO 30 SECONDS IN THE FUTURE. `status_event_key` keys the stored event at that boundary and `InstrumentationStatusHistory::record` inserts it there. The read's default window, built by `status_event_window` for `(None, None)`, is `[now - 3600s, now]` with `end == now` exactly, and `InstrumentationStatusHistory::page` selects on the STORED key with `at >= lower && at <= upper`. So whenever the reporting instant rounds up, the stored key is strictly greater than the window's `end`, the filter rejects the event, and `GetInstrumentationConfigurationStatus` answers HTTP 200 with `Events: []` — for roughly half of all wall-clock instants, and until the wall clock itself passes the rounded-up boundary. The replay shows this persisting across a 30-second poll at 1.5s intervals: the ACTIVE event reported moments earlier never appears, final probe value `[]`. The report itself is not refused (the configuration exists, so `record_instrumentation_status` returns true and `UnprocessedStatusEvents` is empty) — the write lands, the read cannot see it. Hypothesis: the same grid mismatch also truncates the leading edge of a caller-named window whose `EndTime` is not on a minute boundary, and it makes the modelled `NextToken` pagination over `Events` unreachable for the most recent minute of history.

Why: a client that reports a status and then reads it back — the documented agent loop for dynamic instrumentation, and the only way to observe a configuration's status — sees a configuration that it has just reported ACTIVE for as having no status events at all, and an ERROR report as carrying no `ErrorCause`. Nothing distinguishes that from "the report was lost": the response is a success with a required-but-empty `Events` list, and `UnprocessedStatusEvents` was empty too, so the client has no signal to retry. A client polling for its own just-written event waits indefinitely or concludes instrumentation never came up. Independence of the per-status streams is unobservable for the same reason.

Fix: the time-grid agreement between the status-event write and the status-event read, in this service's state module — `status_event_minute`/`status_event_key` (how an event is keyed) and `status_event_window` plus `InstrumentationStatusHistory::page` (how a window selects it) must share one grid. Either key an event at the minute its reported `Time` FLOORS to, or select on the event's actual reported instant (keeping the rounded value only for the `Time` the response reports) or widen the window's upper bound to the minute boundary it rounds to; the invariant to restore is that an event reported at an instant inside a window is selected by that window, whatever rounding the reported `Time` member is subject to.

Traces: application-signals.instrumentation-status-events-accumulate-multiple-transitions — create a PROBE/SNAPSHOT configuration, ReportInstrumentationConfigurationStatus ACTIVE, then poll GetInstrumentationConfigurationStatus with Status=ACTIVE and no StartTime/EndTime.

## Evidence
Check: application-signals.instrumentation-status-events-accumulate-multiple-transitions [mismatch]

Expected (AWS): after CreateInstrumentationConfiguration(InstrumentationType=PROBE, SignalType=SNAPSHOT, Service=<svc>, Environment=<env>) and ReportInstrumentationConfigurationStatus(Service=<svc>, Environment=<env>, Configurations=[{InstrumentationType: PROBE, SignalType: SNAPSHOT, LocationHash: <hash>, Status: ACTIVE, Time: <now>}]), a GetInstrumentationConfigurationStatus(..., Status=ACTIVE) with no StartTime/EndTime returns HTTP 200 with len(Events) >= 1. Reporting Status=ERROR with ErrorCause=RUNTIME_ERROR then yields len(Events) >= 1 under Status=ERROR with an entry whose ErrorCause == RUNTIME_ERROR, while the ACTIVE filter still answers len(Events) >= 1.

Actual (emulator): GetInstrumentationConfigurationStatus answers HTTP 200 with Events == [] for the whole 30-second poll. Harness detail:
  File "check_application_signals_instrumentation_status_events_accumulate_multiple_transitions.py", line 41, in run
    active_events = _poll_until(...)
  File "...", line 125, in _poll_until
    raise AssertionError(f"condition never met within {timeout_s}s; last value={last!r}")
  AssertionError: condition never met within 30s; last value=[]
The report call itself succeeded with an empty UnprocessedStatusEvents, so the write was accepted and only the read diverges.

Mechanism, from the checkout (services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs):
  - status_event_minute(): `let shifted = secs.saturating_add(SECONDS_PER_MINUTE / 2); (shifted.div_euclid(SECONDS_PER_MINUTE).saturating_mul(SECONDS_PER_MINUTE), 0)` — rounds to the NEAREST minute, i.e. forward by up to 30s.
  - status_event_key()/InstrumentationStatusHistory::record() store the event at that rounded instant.
  - status_event_window() for (None, None): `(now - STATUS_EVENT_DEFAULT_WINDOW, *now)` — the upper bound is `now`, unrounded (STATUS_EVENT_DEFAULT_WINDOW = 3600s).
  - InstrumentationStatusHistory::page(): `let at = (key.0, key.1); key.2 == query.status && at >= lower && at <= upper && ...` — compares the rounded key against the unrounded bounds, so a key rounded past `now` fails `at <= upper`.
For any report whose second-of-minute is >= 30 the stored key exceeds `now`, and the event is filtered out until the wall clock reaches that boundary.

The divergence is the empty `Events` list on a 200 response, not any particular timestamp, hash, service or environment value — those are per-run values from the request and the emulator's state.

## Steps to Reproduce
1. Boot the emulator and point an application-signals client at it.
2. CreateInstrumentationConfiguration(InstrumentationType="PROBE", Service=<svc>, Environment=<env>, SignalType="SNAPSHOT", Location={CodeLocation:{Language:"Python", FilePath:"src/checkout/PricingService.py", CodeUnit:"sim2real_module", MethodName:"applyDiscount"}}, CaptureConfiguration={CodeCapture:{CaptureLimits:{MaxHits:50}}}); keep the returned LocationHash.
3. ReportInstrumentationConfigurationStatus(Service=<svc>, Environment=<env>, Configurations=[{InstrumentationType:"PROBE", SignalType:"SNAPSHOT", LocationHash:<hash>, Status:"ACTIVE", Time: <current UTC time whose second-of-minute is >= 30>}]). Observe UnprocessedStatusEvents == [] — the report was accepted.
4. Immediately call GetInstrumentationConfigurationStatus(InstrumentationType="PROBE", Service=<svc>, Environment=<env>, SignalType="SNAPSHOT", LocationIdentifier={LocationHash:<hash>}, Status="ACTIVE") with NO StartTime/EndTime.
5. Observe HTTP 200 with Events == [] instead of at least one event; it stays empty until the wall clock passes the minute boundary the reported Time rounded UP to (up to 30s). Repeating the read in a loop for 30s, as the check does, still observes [].
6. Repeat step 3 with Status="ERROR", ErrorCause="RUNTIME_ERROR" and read with Status="ERROR": the same empty answer, so the ErrorCause is unobservable and the independence of the ACTIVE and ERROR streams cannot be observed either.
7. Contrast with a report whose Time has a second-of-minute < 30: the same read returns the event, confirming the grid mismatch rather than a lost write.
