Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# The MaxResults page-size rule is implemented three times, once per paginated handler

_Filed: 2026-10-25T05:00:46Z_
_Operation: ListAppMonitors | Class: maintainability_

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same page-size computation — "take MaxResults if it converts to usize, otherwise the documented default of 50, then clamp into 1..=100" — plus its two constants is written out verbatim in three handlers of this crate: `ListAppMonitors`, `ListRumMetricsDestinations` and `BatchGetRumMetricDefinitions`. The pagination TOKEN codec was already factored into `crate::state` (`encode_pagination_token` / `decode_pagination_token`, whose own doc says "ONE codec for every paginated operation of this service ... so their token formats cannot drift"), but the page-size rule beside it was not, so the three copies can drift from each other while the token codec cannot. Folding the identical expression (and the two constants it reads) into the same shared `crate::state` home leaves every response byte-identical and every existing pagination test passing — it is the same arithmetic on the same inputs.

## Evidence

Copy 1 — services/service-com-amazonaws-rum-rum/src/ops/list_app_monitors.rs:186-190:
```
        let page_size = input
            .max_results
            .and_then(|requested| usize::try_from(requested).ok())
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
```
with `const DEFAULT_PAGE_SIZE: usize = 50;` (list_app_monitors.rs:226) and `const MAX_PAGE_SIZE: usize = 100;` (list_app_monitors.rs:230).

Copy 2 — services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs:189-193 (same five lines), with `const DEFAULT_PAGE_SIZE: usize = 50;` (:277) and `const MAX_PAGE_SIZE: usize = 100;` (:281).

Copy 3 — services/service-com-amazonaws-rum-rum/src/ops/batch_get_rum_metric_definitions.rs:226-230 (same five lines), with `const DEFAULT_PAGE_SIZE: usize = 50;` (:338) and `const MAX_PAGE_SIZE: usize = 100;` (:342).

The crate already holds the sibling pagination rule once — services/service-com-amazonaws-rum-rum/src/state.rs:1673-1675: "ONE codec for every paginated operation of this service (`ListAppMonitors`, `ListRumMetricsDestinations`), so their token formats cannot drift."

Tests that pin the behaviour and must stay unchanged: tests/all/list_app_monitors_page_size.rs:58 `list_app_monitors_pages_at_max_results_and_at_the_documented_default`; tests/all/metrics_destinations.rs:839 `list_pages_at_the_modelled_max_results_range_and_the_documented_default` and :577 `list_pages_over_every_destination_without_repeating_or_dropping`; tests/all/metric_definitions.rs:1093 `every_page_of_a_destinations_definitions_covers_each_one_exactly_once`; tests/all/app_monitor_lifecycle.rs:956 `list_app_monitors_pages_over_the_whole_store_without_repeating_or_dropping`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-rum-rum/src/ops/list_app_monitors.rs and read lines 186-190 and 224-230
2. Open services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs and read lines 189-193 and 274-281 — the same expression and the same two constants
3. Open services/service-com-amazonaws-rum-rum/src/ops/batch_get_rum_metric_definitions.rs and read lines 226-230 and 335-342 — a third copy
4. Note that services/service-com-amazonaws-rum-rum/src/state.rs:1679 and :1692 already host the shared pagination token codec, so the page-size rule has an existing single home to move to
