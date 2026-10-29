Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# CreateDashboard stores widget time ranges without validating DateTimeValue form, range order, or widget-id uniqueness

_Filed: 2026-10-24T09:36:42Z_
_Operation: CreateDashboard | Class: validation_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateDashboard persists the submitted `widgets` list verbatim — the only pre-store checks the handler makes are dashboard-name uniqueness and the dashboard quota, and `crate::state::adopt_widgets` only mints an id for a widget that arrived without one. Three spec-described input rules therefore never run and invalid widget configurations are accepted with HTTP 200 and read back by GetDashboard/ListDashboards:

1. `DateTimeValue` pairs `type` (ABSOLUTE | RELATIVE) with a free-form `value` string whose form the model documents: "Represents a point in time that can be specified as either an absolute date (for example, \"2025-07-01\") or a relative time period using ISO 8601 duration format (for example, \"-P3M\" for three months ago)". `value` is an unconstrained GenericString (pattern ^[\S\s]*$), so nothing at the boundary enforces the documented form and the handler adds no check: a widget whose `configs[].queryParameters.costAndUsage.timeRange.startTime` is `{"type": "RELATIVE", "value": "last quarter"}` or `{"type": "ABSOLUTE", "value": "not-a-date"}` is stored and returned. Expected: ValidationException (HTTP 400), nothing created.
2. `DateTimeRange` is documented as "a time period with explicit start and end times" with both members @required; an ABSOLUTE range whose `endTime` precedes its `startTime` (e.g. startTime 2025-07-01, endTime 2025-01-01) is not a time period and is accepted today. Expected: ValidationException.
3. `Widget.id` is documented as "The unique identifier for the widget" and the handler honours a client-supplied id as-is (pinned by tests), but it never checks that the ids in one `widgets` list are distinct: two widgets carrying the same 32-hex id are both stored, which makes a scheduled report's `widgetIds` resolve ambiguously and makes `ExecuteScheduledReport`'s WIDGET_ID_NOT_FOUND derivation meaningless. Expected: ValidationException for a duplicate widget id inside one dashboard.

The same unvalidated widget path is reached by UpdateDashboard (`input.widgets` → `state.adopt_widgets`), so fixing it in one shared place covers both.

## Evidence

Spec: smithy_query(query="shape", shape="DateTimeValue") — type is the DateTimeType enum (ABSOLUTE/RELATIVE), value is GenericString with the doc naming "2025-07-01" for absolute and ISO 8601 durations such as "-P3M" for relative; shape="DateTimeRange" — startTime/endTime both @required, "Defines a time period with explicit start and end times"; shape="Widget" — id is "The unique identifier for the widget" with pattern ^[0-9a-f]{32}$; CreateDashboard declares ValidationException at HTTP 400 (OPERATIONS.md). Handler: src/ops/create_dashboard.rs lines 170-242 — the only checks are `state.dashboard_arn_by_name` and `state.dashboards.len() >= MAX_DASHBOARDS`; `input.widgets` goes through `state.adopt_widgets` (src/state.rs:410-420) which only fills a missing id and never inspects `configs`. Tests: tests/all/dashboard_lifecycle.rs:140-170 submits RELATIVE ("-P3M"/"P0D") and ABSOLUTE ("2025-01-01"/"2025-07-01") ranges only as VALID input and asserts they round-trip; no test submits a malformed DateTimeValue, an inverted range, or two widgets sharing one id, so the permissive behavior is uncovered rather than asserted. The earlier CreateDashboard/validation issue (20261024T024716Z-bcm-dashboards-createdashboard-validation, fixed) covered only duplicate dashboard names; the open CreateScheduledReport/validation issue covers `widgetDateRangeOverride` on the scheduled-report input, not the widget configs of a dashboard.

## Steps to Reproduce

1. Call CreateDashboard with name="TimeRangeProbe" and one widget whose configs[0].queryParameters.costAndUsage.timeRange = {startTime: {type: "RELATIVE", value: "last quarter"}, endTime: {type: "RELATIVE", value: "P0D"}} — expected HTTP 400 ValidationException because a RELATIVE value must be an ISO 8601 duration; observed HTTP 200 with a minted dashboard ARN.
2. Call GetDashboard on that ARN — the bogus relative value is read back verbatim.
3. Call CreateDashboard with an ABSOLUTE timeRange of startTime 2025-07-01 and endTime 2025-01-01 (end before start) — expected ValidationException; observed HTTP 200 and the inverted range persisted.
4. Call CreateDashboard with two widgets that both carry "id": "ffffffffffffffffffffffffffffffff" — expected ValidationException for the duplicate widget id; observed HTTP 200 and both widgets stored under the same id, after which a scheduled report naming that widgetId cannot be resolved unambiguously.
