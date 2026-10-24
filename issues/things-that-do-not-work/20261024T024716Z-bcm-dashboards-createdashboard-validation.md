Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# CreateDashboard accepts a duplicate dashboard name although the model requires the name to be unique within the account

_Filed: 2026-10-24T02:47:16Z_
_Operation: CreateDashboard | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `name` member of CreateDashboard is documented as "The name of the dashboard. The name must be unique within your account." The emulator never consults the existing dashboards' names: it mints a fresh ARN per call and inserts a second record, so two dashboards with identical names coexist and ListDashboards reports both. Expected: a CreateDashboard whose `name` equals the name of a dashboard already stored for the account is refused with the operation's modelled ValidationException (HTTP 400 — CreateDashboard declares no ConflictException) and no dashboard is created. Actual: HTTP 200 with a second ARN, creating state the real service cannot hold.

## Evidence

Smithy model: CreateDashboardRequest.name targets DashboardName (@required) whose documentation states "The name must be unique within your account."; CreateDashboard declares ValidationException (HTTP 400) as its only input-refusal error. Handler: src/ops/create_dashboard.rs lines 195-217 — the comment explicitly chooses the opposite rule ("A fresh ARN per call, so two dashboards created with the same `name` are distinct resources") and the only pre-insert check is the MAX_DASHBOARDS quota. A test pins the divergent behavior: tests/all/dashboard_lifecycle.rs:473 `same_name_creates_distinct_dashboards` asserts two creates with one name yield two different ARNs and both are listed. The separate open issue about DashboardName covers only the @pattern (double-space) enforcement, not name uniqueness.

## Steps to Reproduce

1. POST CreateDashboard with name="Monthly Spend" and one valid widget — observe 200 and arn A.
2. POST CreateDashboard again with the SAME name="Monthly Spend" and a valid widget — expected ValidationException (HTTP 400) because the name must be unique within the account; observed HTTP 200 and a second arn B.
3. Call ListDashboards — observe two DashboardReference entries carrying the identical `name`.
