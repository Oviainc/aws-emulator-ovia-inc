Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# DeleteDashboard on an absent/already-deleted dashboard must be an idempotent 200 no-op, not ValidationException

_Filed: 2026-10-24T02:47:16Z_
_Operation: DeleteDashboard | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteDashboard models NO not-found error (its only client errors are AccessDeniedException, ThrottlingException and ValidationException), and the recorded service treats a delete of a well-formed DashboardArn that no longer resolves as an idempotent no-op: HTTP 200 echoing the requested `arn` in the required output member. The emulator instead removes the record only when it is present and otherwise answers HTTP 400 `ValidationException` ("Member must identify an existing dashboard: arn <arn>"). Expected: DeleteDashboard(arn) on an ARN that satisfies the modelled DashboardArn pattern returns 200 with `arn` = the requested ARN whether or not a record existed, deleting it when it did. Actual: the second (retry) delete of the same ARN, and any delete of a never-created-but-well-formed ARN, is refused with a 400 ValidationException, so a client retrying a timed-out delete sees a hard failure for a resource that is in fact gone.

## Evidence

Smithy model: DeleteDashboard output requires `arn` (DashboardArn) and the operation declares no ResourceNotFoundException at all — the only shape left for an absent resource would be ValidationException, but the recorded corpus trace `bcm-dashboards.delete-nonexistent-dashboard-idempotent-noop` (cited in open issue 20261022T215638Z-bcm-dashboards-dashboardname-pattern-unenforced-constraint-not-found, cause 3: "re-run delete_dashboard(arn=arn) and confirm it is a 200 no-op (DeleteDashboard models no ResourceNotFoundException)") records the real answer as a 200 no-op. Handler: src/ops/delete_dashboard.rs lines 163-179 returns `DeleteDashboardErrorModeled::ValidationException` when `state.dashboards.remove(&input.arn)` yields None. Current tests PIN the divergent behavior rather than the recorded one: tests/all/dashboard_lifecycle.rs:907-916 asserts status 400 + ValidationException for a repeat delete, and tests/all/resource_not_found.rs:61 documents DeleteDashboard as answering ValidationException — both would need to change with the fix. No existing issue is filed against operation DeleteDashboard.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBCMDashboardsService.CreateDashboard and a valid name + one widget; note the returned `arn`.
2. Call DeleteDashboard with that `arn` — observe 200 and the echoed arn (correct).
3. Call DeleteDashboard again with the SAME `arn` — expected HTTP 200 with `arn` echoed (idempotent no-op); observed HTTP 400 with code ValidationException.
4. Call DeleteDashboard with a well-formed but never-created ARN (e.g. arn:aws:bcm-dashboards::000000000000:dashboard/does-not-exist) — expected 200 no-op; observed 400 ValidationException.
