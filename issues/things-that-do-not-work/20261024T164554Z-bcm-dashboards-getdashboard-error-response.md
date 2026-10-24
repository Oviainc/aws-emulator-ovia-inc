Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# Reads never authorize the caller, so a denied principal can read dashboards, reports and resource policies

_Filed: 2026-10-24T16:45:54Z_
_Operation: GetDashboard | Class: error-response_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The five reporting operations that model `AccessDeniedException` (HTTP 403) — `GetDashboard`, `GetScheduledReport`, `GetResourcePolicy`, `ListDashboards` and `ListScheduledReports` — make no IAM decision: each resolves the engine state, takes the lock and projects the record.

Expected: a caller whose identity-based policies do not allow `bcm-dashboards:GetDashboard` (resp. `:GetScheduledReport`, `:GetResourcePolicy`, `:ListDashboards`, `:ListScheduledReports`) on the resource is refused with the modelled `AccessDeniedException` (403) instead of being handed the resource. `GetResourcePolicy` is the sharpest case: it exposes the resource-based policy of a dashboard, i.e. who the dashboard is shared with, to any caller that can reach the endpoint.

Actual: every one of them answers HTTP 200 with the full record for any credential, including one whose IAM policy explicitly denies the action. A 403 is unreachable for the whole service — `AccessDeniedException` is constructed nowhere in the crate.

## Evidence

Spec: `AccessDeniedException` (@httpError 403) is declared for GetDashboard, GetScheduledReport, GetResourcePolicy, ListDashboards and ListScheduledReports (OPERATIONS.md error lists per operation).
Handlers: `src/ops/get_dashboard.rs` lines 169-193, `src/ops/get_scheduled_report.rs` lines 159-177, `src/ops/get_resource_policy.rs` lines 160-187, `src/ops/list_dashboards.rs` lines 154-225, `src/ops/list_scheduled_reports.rs` lines 155-226 — all five open with `service_state(...)` + `state.lock()` and then project, with no caller/identity step; each even annotates `let _ = ctx;`, discarding the only request metadata an identity could be resolved from.
Shared facility that exists and is unused here: `service_common::iam_authorization` (used in-handler by `services/service-com-amazonaws-bcmdataexports-.../src/state/authorization.rs`, and by the rum, transfer, b2bi, evs, fis crates); this crate's `Cargo.toml` lists neither `service_common` nor the IAM/STS crates.
Tests: no test under `tests/` asserts a 403 for any operation (grep `AccessDenied|403` matches only the media-type probe at `tests/all/response_media_type.rs:292`).

## Steps to Reproduce

1. Register this service beside the IAM and STS emulators on one engine.
2. With the account's ambient credential, CreateDashboard (name="ReadProbe") and keep the ARN.
3. Create an IAM user whose inline policy allows nothing for bcm-dashboards, and an access key for it.
4. Call GetDashboard with that denied access key id and the dashboard ARN — expect HTTP 403 AccessDeniedException; observe HTTP 200 with arn/name/type/widgets/createdAt/updatedAt.
5. Call GetResourcePolicy, ListDashboards, ListScheduledReports and GetScheduledReport with the same denied credential — each returns 200 with the stored data instead of 403.
