Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# GetScheduledReport omits healthStatus for a report ListScheduledReports reports as HEALTHY

_Filed: 2026-10-24T03:07:50Z_
_Operation: GetScheduledReport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For one and the same stored report, the two readers disagree. `ScheduledReportRecord.health_status` is `None` until something executes the report; `ScheduledReportRecord::summary()` resolves that to `HealthStatus { statusCode: HEALTHY }` (because `ScheduledReportSummary.healthStatus` is `@required`), while `ScheduledReportRecord::report()` passes the `None` straight through. So after CreateScheduledReport, ListScheduledReports says the report is HEALTHY and GetScheduledReport returns a `scheduledReport` with no `healthStatus` member at all.

Expected: a report the service accepted at create time reports the same health in both views — `healthStatus.statusCode = HEALTHY` (the model: "HEALTHY indicates the scheduled report is configured properly and has all required permissions to execute"), with no `statusReasons` (modelled as present only when UNHEALTHY). Actual: the detail view is missing a field the summary view of the same record carries.

## Evidence

Spec: `smithy_query(query="shape",shape="HealthStatus")` documents HEALTHY as "configured properly and has all required permissions to execute"; `shape="ScheduledReportSummary"` marks `healthStatus` @required while `shape="ScheduledReport"` leaves it optional — the health of a resource is a property of the resource, not of the view, so a detail read cannot report less than the listing. Handler/state: `src/ops/get_scheduled_report.rs` returns `report.report()`, and `src/state.rs` lines ~167-181 (`report()`) forwards `health_status` unchanged while lines ~214-218 (`summary()`) substitute HEALTH_STATUS_HEALTHY. Tests: `tests/all/scheduled_report_lifecycle.rs` asserts the created record's fields but accepts an absent healthStatus; `tests/all/scheduled_report_listing.rs` asserts the HEALTHY summary — nothing asserts the two agree.

## Steps to Reproduce

1. CreateDashboard, then CreateScheduledReport against it — note the report ARN.
2. ListScheduledReports — the summary carries healthStatus.statusCode = "HEALTHY".
3. GetScheduledReport with that ARN — expect scheduledReport.healthStatus.statusCode = "HEALTHY" too; actual the healthStatus member is absent from the response body.
