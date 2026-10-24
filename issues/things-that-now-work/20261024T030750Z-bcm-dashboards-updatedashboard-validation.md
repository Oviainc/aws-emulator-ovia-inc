Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Refuse an UpdateDashboard that renames a dashboard onto a name already in the account

_Filed: 2026-10-24T03:07:50Z_
_Operation: UpdateDashboard | Class: validation_

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateDashboardRequest.name` is the "new name for the dashboard", and dashboard names are documented as unique within the account (CreateDashboardRequest.name: "The name must be unique within your account"). The handler assigns `dashboard.name = input.name` with no check against the other stored dashboards, so renaming dashboard B to the name dashboard A already carries succeeds with HTTP 200 and leaves the account holding two dashboards with the same name.

Expected: the rename is refused with the operation's modelled `ValidationException` (HTTP 400) and the stored record is unchanged (including `updatedAt`). Actual: HTTP 200, the duplicate name is persisted and both names show up in ListDashboards.

## Evidence

Spec: `smithy_query(query="shape",shape="CreateDashboardRequest")` states the account-wide uniqueness rule for dashboard names; `shape="UpdateDashboardRequest"` documents `name` as the new name for the dashboard, i.e. the same namespace. Handler: `src/ops/update_dashboard.rs` lines ~203-216 — only existence of the target ARN is checked; the name is written unconditionally. Tests: `tests/all/dashboard_lifecycle.rs::update_dashboard_mutates_the_stored_record` covers successful mutation only; no test renames one dashboard onto another's name.

## Steps to Reproduce

1. CreateDashboard with name="Alpha" (arn A) and CreateDashboard with name="Beta" (arn B).
2. UpdateDashboard with arn=B, name="Alpha" — expect ValidationException (HTTP 400) and GetDashboard(B) still reporting "Beta"; actual HTTP 200 and GetDashboard(B) reports "Alpha".
3. ListDashboards — expect one dashboard named "Alpha"; actual two.
