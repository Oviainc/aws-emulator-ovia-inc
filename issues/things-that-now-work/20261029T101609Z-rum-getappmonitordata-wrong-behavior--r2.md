Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8feab889966a2449ff573dbe9da366d138b0e00f

# A negative TimeRange.Before is read as "omitted" and silently extends the query window to now

_Filed: 2026-10-29T10:16:09Z_
_Operation: GetAppMonitorData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8feab889966a2449ff573dbe9da366d138b0e00f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetAppMonitorData` builds the query window in `Window::from`. The handler's own doc states the rule it means to apply: `TimeRange$Before` carries `@default(0)`, so "a request that left it at the default is answered against the window ending NOW". The code under that doc tests `time_range.before <= 0`, not `== 0`, so ANY non-positive value — not only the default the doc names — is replaced by `now_seconds()`.

`TimeRange$Before` targets `QueryTimestamp`, a `long` with no `@range`, so a negative value is input the model admits and reaches the handler as a value the caller SENT rather than as an omitted member. For such a request the window silently becomes `[After, now]` instead of `[After, Before]`, and the call reports every stored event up to the present instead of the (empty) set the requested window covers — a success carrying events the request excluded, with no refusal and no indication the bound was discarded.

The neighbouring `page_size` in the same file folds a non-positive value into a default too, but states a reason for it ("A maximum of zero results is not what 'the maximum number of results to return' can mean"). No such reason exists for a timestamp: a negative epoch second is a meaningful instant, and `Window::covers` would correctly select nothing for it.

## Evidence

Smithy, `TimeRange$Before` (`smithy_query(query="shape", shape="TimeRange")`):
> "Before": {"target": "QueryTimestamp", "default": 0, "type": "long", "doc": "<p>The end of the time range to retrieve performance events from. If you omit this, the time range extends to the time that this operation is performed.</p>"}

The refusal-free widening — the doc says "left it at the default" (which `@default` makes `0`), the code accepts every non-positive value, `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:345-362`:
```
impl Window {
    /// The window `time_range` asks for.
    ///
    /// `TimeRange$Before` is modelled `@default(0)` and documented "If you omit this, the
    /// time range extends to the time that this operation is performed", so a request
    /// that left it at the default is answered against the window ending NOW. Both bounds
    /// are read in the unit this protocol carries a `RumEvent$timestamp` in — seconds
    /// since the epoch — because they bound that same instant.
    fn from(time_range: &TimeRange) -> Self {
        Self {
            after: time_range.after,
            before: if time_range.before <= 0 {
                now_seconds()
            } else {
                time_range.before
            },
        }
    }
```

What the window would otherwise have excluded, `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:364-369`:
```
    /// Whether an event at `seconds` falls in this window. Both bounds are inclusive —
    fn covers(&self, seconds: Option<i64>) -> bool {
        seconds.is_some_and(|seconds| self.after <= seconds && seconds <= self.before)
    }
```

The sibling rule in the same file that DOES state a reason for folding a non-positive value into a default, `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:292-305`:
```
/// `MaxQueryResults` is modelled `range 0..100` with `@default(0)`, so the boundary
/// validator has already refused anything outside it. A maximum of zero results is not
/// what "the maximum number of results to return" can mean, so the modelled default
/// stands for a request that named no page size, …
```

Test coverage: the time-window table in `services/service-com-amazonaws-rum-rum/tests/all/app_monitor_data.rs:306-337` exercises only non-negative bounds — its rows are `("the whole epoch", 0, None, …)`, `("entirely before the span", 0, Some(BASE - 1), vec![])`, `("end before start", BASE + 120, Some(BASE + 60), vec![])` — and the only `Before`-absent rows pass `None`, which the body builder omits (`app_monitor_data.rs:248-250`: `let mut time_range = json!({"After": after}); if … time_range["Before"] = json!(before);`). So no existing test sends a negative `Before`, and narrowing the branch to the default alone leaves every row's expectation unchanged.

## Steps to Reproduce

1. CreateAppMonitor(Name="m1", Domain="example.com")
2. PutRumEvents(Id=<the minted id>) with one RumEvent whose timestamp is the present instant
3. GetAppMonitorData(Name="m1", TimeRange={"After": -200, "Before": -100}, MaxResults=10)
4. Observed: 200 with Events containing the event just delivered — the window was replaced by [-200, now]
5. Expected: the window the request named is honoured (no event lies between -200 and -100), so Events is empty; the "extends to the time that this operation is performed" rule is documented for a request that OMITS Before, which on the wire is the member's @default of 0
