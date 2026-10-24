Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# No bcm-dashboards write authorizes the caller, so an IAM-denied principal can create, update, delete and execute

_Filed: 2026-10-24T16:45:54Z_
_Operation: CreateDashboard | Class: error-response_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every state-changing bcm-dashboards operation that models `AccessDeniedException` (HTTP 403) answers it never: `CreateDashboard`, `UpdateDashboard`, `DeleteDashboard`, `CreateScheduledReport` and `ExecuteScheduledReport` all go straight from `crate::state::service_state(...)` to `state.lock()` and write, with no IAM decision in between. `AccessDeniedException` is constructed nowhere in the crate (it exists only as a generated type in `src/ops/types/shared_types.rs`).

Expected: a caller whose identity-based policies do not allow `bcm-dashboards:CreateDashboard` (resp. `:UpdateDashboard`, `:DeleteDashboard`, `:CreateScheduledReport`, `:ExecuteScheduledReport`) on the target resource is refused with the modelled `AccessDeniedException` (403) BEFORE any state is written — the same decision sibling crates in this tree make in-handler through `service_common::iam_authorization` over IAM's and STS's own stores (see `services/service-com-amazonaws-bcmdataexports-.../src/state/authorization.rs`, and the same pattern in the rum, transfer, b2bi, evs and fis crates).

Actual: any caller — including one whose IAM policy explicitly denies the action, or one presenting a credential for a principal that is allowed nothing — gets HTTP 200, a minted ARN / a mutated or deleted record. The request's access key id is read only to put it in the audit line (`crate::state::record_state_change` -> `engine::extract_akid`), never to make a decision. For `DeleteDashboard` this is unauthorized data loss.

This issue covers the write path as one unit because the fix is one shared helper plus one call per handler; the read path is filed separately against `GetDashboard`.

## Evidence

Spec: the model declares `AccessDeniedException` at @httpError 403 for CreateDashboard, UpdateDashboard, DeleteDashboard, CreateScheduledReport and ExecuteScheduledReport (OPERATIONS.md error lists; `smithy_query(query="shape",shape="AccessDeniedException")` documents it as "You do not have sufficient permissions to perform this action. Verify your IAM permissions and any resource policies.").
Handlers: `src/ops/create_dashboard.rs` lines 170-261, `src/ops/update_dashboard.rs` lines 168-244, `src/ops/delete_dashboard.rs` lines 154-184, `src/ops/create_scheduled_report.rs` lines 176-292, `src/ops/execute_scheduled_report.rs` lines 200-333 — each locks the store and mutates with no authorization call.
Repo-wide grep for `authorize|iam` under this crate's `src/` returns only `engine::extract_akid` in the audit helper (`src/state.rs:569`) and doc mentions of the role ARN pattern; `Cargo.toml` does not depend on `service_common` or the IAM/STS crates, while bcm-data-exports does and calls `crate::state::authorize(...)` from its handlers.
Tests: grep for `AccessDenied|403` under `tests/` matches only `tests/all/response_media_type.rs:292` (a media-type probe), so no test asserts either the current permissive behavior or the refusal — the gap is uncovered, not pinned.

## Steps to Reproduce

1. Register this service beside the IAM and STS emulators on one engine (as bcm-data-exports' tests/all/front_door_authorization.rs does).
2. Create an IAM user with an inline policy that allows nothing for bcm-dashboards (or explicitly denies bcm-dashboards:CreateDashboard), and an access key for it.
3. Call CreateDashboard with that access key id in the SigV4 credential scope, name="DeniedProbe", widgets=[one valid widget] — expect HTTP 403 AccessDeniedException and no dashboard created; observe HTTP 200 with a minted ARN.
4. Call ListDashboards with an allowed credential — the dashboard the denied principal created is there.
5. Repeat with UpdateDashboard, DeleteDashboard (on an existing dashboard ARN), CreateScheduledReport and ExecuteScheduledReport using the same denied credential — each returns 200 and really mutates state instead of 403.
