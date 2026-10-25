Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 11e424e87637980d781b5ebada45988cd752adfb

# ListAppMonitors pages at 100 when MaxResults is absent, not the documented default of 50

_Filed: 2026-10-25T01:00:19Z_
_Operation: ListAppMonitors | Class: pagination_

> _Note: this issue was identified against the code base at commit `11e424e87637980d781b5ebada45988cd752adfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAppMonitors` is the only paginated listing in this crate that does not apply the default page size its model documents. The model says `ListAppMonitorsRequest$MaxResults` has a default of 50; the handler substitutes `MAX_PAGE_SIZE` (100) when the request names no `MaxResults`, so a caller that omits the member gets up to 100 summaries on the first page (and no `NextToken` where one is owed up to the 100th monitor) instead of 50.

The crate's two other paginated listings read the SAME doc sentence from their own `MaxResults` member and define a separate `DEFAULT_PAGE_SIZE = 50` constant for exactly this case, so the three listings of one service disagree about how an absent `MaxResults` is read. No comment or test in the crate justifies the divergence: the handler's comment asserts "an absent one pages at that maximum" without citing the member's documentation, and the existing `ListAppMonitors` pagination test only seeds stores of 0/1/3/7 monitors, so nothing pins the 100.

## Evidence

Model (smithy_query doc, `ListAppMonitorsRequest$MaxResults`): "The maximum number of results to return in one operation. The default is 50. The maximum that you can specify is 100."

Handler, services/service-com-amazonaws-rum-rum/src/ops/list_app_monitors.rs:182-188:
```
// `MaxResults` is modelled `range 1..100`, so the boundary validator has already
// refused anything outside it; an absent one pages at that maximum.
let page_size = input
    .max_results
    .and_then(|requested| usize::try_from(requested).ok())
    .unwrap_or(MAX_PAGE_SIZE)
    .clamp(1, MAX_PAGE_SIZE);
```
with list_app_monitors.rs:222-224: `/// The largest page this operation returns: ... const MAX_PAGE_SIZE: usize = 100;` — there is no `DEFAULT_PAGE_SIZE` in this file at all.

Sibling in the same crate, services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs:274-277:
```
/// The page this operation returns when the request names no `MaxResults`.
/// From the model: `ListRumMetricsDestinationsRequest$MaxResults` documents "The default
/// is 50."
const DEFAULT_PAGE_SIZE: usize = 50;
```
and services/service-com-amazonaws-rum-rum/src/ops/batch_get_rum_metric_definitions.rs:335-338 (same constant, same quoted sentence), both applied via `.unwrap_or(DEFAULT_PAGE_SIZE)`.

Test coverage: services/service-com-amazonaws-rum-rum/tests/all/metrics_destinations.rs:838-869 pins the sibling's behaviour — `const DOCUMENTED_DEFAULT: usize = 50;` ... "expected the documented default page of {DOCUMENTED_DEFAULT}". The `ListAppMonitors` suite (tests/all/app_monitor_lifecycle.rs:956-1000) only uses `store_size in [0,1,3,7]`, so no existing test pins a 100-item default page and changing it leaves every existing assertion unchanged.

## Steps to Reproduce

1. Create 60 app monitors (names unique) through CreateAppMonitor.
2. Call `POST /appmonitors` with no `maxResults` query parameter.
3. Observe 60 `AppMonitorSummaries` in one page and no `NextToken`; the model documents a default of 50, i.e. 50 summaries plus a `NextToken`.
4. Call `GET /rummetrics/{name}/metricsdestination` with 51 destinations stored and no `maxResults` for contrast: that listing stops at 50 and returns a token.
