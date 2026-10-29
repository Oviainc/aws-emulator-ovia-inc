Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: afab077872ee6c32a9626f154d3b33fea96b8a18

# A RumEvent whose timestamp the RFC 3339 formatter cannot render is stored with an empty timestamp, so GetAppMonitorData can never return it and the CloudWatch Logs copy loses the instant

_Filed: 2026-10-28T18:16:12Z_
_Operation: PutRumEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `afab077872ee6c32a9626f154d3b33fea96b8a18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutRumEvents` renders each delivered event into the JSON document the service persists (`event_json`). The event's instant is produced by the FALLIBLE `aws_smithy_types::DateTime::fmt(Format::DateTime)` and the error arm is discarded with `.unwrap_or_default()`, so an instant that formatter cannot render is written into the stored document as the EMPTY STRING and the call still answers 200.

`RumEvent$timestamp` is an unconstrained `@required` timestamp (no `@range`), so on this restJson1 service a client may send any epoch-seconds number; one outside the range RFC 3339 can render reaches this line. The consequences are both silent:

* the event is appended to the monitor's buffer (and counts against its capacity) but `GetAppMonitorData` can never report it: the read decides membership of the query window from exactly that field, and an event whose instant cannot be read "falls in no window" — so the event is invisible to EVERY query, including the widest one (`After=0`, no `Before`);
* the CloudWatch Logs copy is built from the same `event_json` (`cw_log_delivery::copy_of`), so the copy carries `"timestamp": ""` too and the client's instant is nowhere in the stored state.

The crate already decided how to handle this exact formatter failure elsewhere: `crate::state::iso_timestamp` calls the SAME formatter, notices the empty/short result and substitutes a parseable value "rather than a string no client can parse". The data-plane write skips that handling, so one path in this crate normalises an unrenderable instant and the other persists an empty one.

## Evidence

The write that discards the formatting error — `services/service-com-amazonaws-rum-rum/src/ops/put_rum_events.rs:301-309`:
```
    document.insert(
        "timestamp".to_string(),
        ::serde_json::Value::String(
            event
                .timestamp
                .fmt(::aws_smithy_types::date_time::Format::DateTime)
                .unwrap_or_default(),
        ),
    );
```
(its own doc, :293-297, states the document is what the event "is stored (and later reported) as").

The reader that then cannot see the event — `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:380-389`:
```
/// The stored document renders `RumEvent$timestamp` as an RFC 3339 instant, so it is read
/// back through the same codec rather than re-derived from anything else.
fn event_seconds(document: &::serde_json::Value) -> Option<i64> {
    let rendered = document.get("timestamp")?.as_str()?;
    ::aws_smithy_types::DateTime::from_str(rendered, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|instant| instant.secs())
}
```
and `:357-362`:
```
    /// Whether an event at `seconds` falls in this window. Both bounds are inclusive ...
    /// and an event whose instant cannot be read falls in no window.
    fn covers(&self, seconds: Option<i64>) -> bool {
        seconds.is_some_and(|seconds| self.after <= seconds && seconds <= self.before)
    }
```
reached from the one filter over the buffer, `:249-259` (`window.covers(event_seconds(&document)) && matches_filters(...)`).

The sibling path in the SAME crate that does handle the failure — `services/service-com-amazonaws-rum-rum/src/state.rs:1244-1255`:
```
    let rendered = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let trimmed = rendered.strip_suffix('Z').unwrap_or(rendered.as_str());
    if trimmed.len() == 19 {
        trimmed.to_string()
    } else {
        // Only reachable for an instant RFC 3339 cannot render (a clock outside year
        // 0001..9999). The shape admits 19 characters and nothing else, so answer the
        // epoch rather than a string no client can parse.
        "1970-01-01T00:00:00".to_string()
    }
```

The CloudWatch Logs copy reuses the same document — `services/service-com-amazonaws-rum-rum/src/state/cw_log_delivery.rs:206-220` (`copy_of` starts from `::serde_json::from_str(&event.event_json)`), and its header states the contract it is meant to meet (`:59-61`): "the MESSAGE is one JSON object per delivered event, carrying the event exactly as the batch delivered it".

Model side (`smithy_query(query="shape", shape="RumEvent")` / OPERATIONS.md:36): `RumEvents` (list<RumEvent>) is `@required` and `RumEvent$timestamp` carries no `@range`, so no modelled constraint keeps such a value out of the handler.

Test coverage: none. `services/service-com-amazonaws-rum-rum/tests/all/app_monitor_data.rs:21-23` fixes every fixture instant at `const BASE: i64 = 1_704_067_200;` ("`2024-01-01T00:00:00Z` ... with the rest at one-minute steps after it") and every row renders in range (`rendered_instant: "2024-01-01T00:00:00Z"`, :80); no test sends a timestamp the formatter cannot render, so nothing pins the current behaviour with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com") -> 200 with an Id.
2. PutRumEvents(Id=<that id>, BatchId="b1", AppMonitorDetails={...}, UserDetails={...}, RumEvents=[{id="11111111-1234-4234-8234-123456789012", timestamp=99999999999999, type="com.amazon.rum.page_view_event", details="{}"}]) -> observed HTTP 200 (accepted).
3. GetAppMonitorData(Name="m", TimeRange={After: 0}, MaxResults: 100) -> observed Events=[] : the accepted event is in the store but matches no window, because its stored document carries "timestamp": "". Expected: the event the service just acknowledged is reportable (or the request is refused), not silently unqueryable.
4. With CwLogEnabled=true on the monitor, tail the vended log group: the copied message carries "timestamp": "" instead of the instant the batch delivered.
