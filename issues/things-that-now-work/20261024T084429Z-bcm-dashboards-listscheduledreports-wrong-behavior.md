Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# ListScheduledReports reports healthStatus HEALTHY unconditionally instead of deriving it from stored state

_Filed: 2026-10-24T08:44:29Z_
_Operation: ListScheduledReports | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ScheduledReportSummary.healthStatus` is `@required` and documented as "The health status of the scheduled report as of its last refresh time." The emulator returns the health stored on the record, and when nothing has ever executed the report it fabricates `{statusCode: HEALTHY}` with no `lastRefreshedAt` — even when the store itself can prove the report is broken.

Expected: a report whose `dashboardArn` names a dashboard that no longer exists (or whose `widgetIds` name a widget the dashboard no longer holds) is reported UNHEALTHY with `statusReasons` [DASHBOARD_NOT_FOUND] / [WIDGET_ID_NOT_FOUND], the same derivation the service already implements for ExecuteScheduledReport (`crate::state::scheduled_report_health`).

Actual: create a report, delete its dashboard, call ListScheduledReports → the summary still says HEALTHY, while ExecuteScheduledReport on the SAME report answers UNHEALTHY/DASHBOARD_NOT_FOUND. The listing's health value is therefore a canned value that contradicts both the store and the service's own other answer.

## Evidence

Spec: `ScheduledReportSummary.healthStatus` is @required with HealthStatusCode documented HEALTHY/UNHEALTHY, and `StatusReason` includes DASHBOARD_NOT_FOUND / WIDGET_ID_NOT_FOUND (smithy_query shape=ScheduledReportSummary, HealthStatus).
Handler: `src/ops/list_scheduled_reports.rs` pushes `report.summary()`; `src/state.rs` `ScheduledReportRecord::summary()` does `health_status: self.health_status.clone().unwrap_or_else(|| HealthStatus { status_code: HEALTHY, last_refreshed_at: None, status_reasons: None })` — it never calls the existing `ComAmazonaws...State::scheduled_report_health()` helper that derives health from the dashboards map.
Tests: `tests/all/scheduled_report_listing.rs:194-204` asserts HEALTHY only for freshly created reports whose dashboard still exists; grep for DASHBOARD_NOT_FOUND/UNHEALTHY shows those reasons are asserted only in `tests/all/scheduled_report_execution.rs`, so the deleted-dashboard listing case is untested.

## Steps to Reproduce

1. CreateDashboard with one widget; note the dashboard ARN
2. CreateScheduledReport referencing that dashboard ARN
3. DeleteDashboard with the dashboard ARN
4. ListScheduledReports → the summary for the report still carries healthStatus.statusCode = HEALTHY with no statusReasons
5. ExecuteScheduledReport with the report ARN → UNHEALTHY with statusReasons [DASHBOARD_NOT_FOUND], contradicting the listing
