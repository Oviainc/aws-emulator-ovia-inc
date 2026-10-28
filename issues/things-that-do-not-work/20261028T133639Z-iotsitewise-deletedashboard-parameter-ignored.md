Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# DeleteDashboard drops clientToken, on the strength of a comment pointing at DeleteProject — which now honours it

_Filed: 2026-10-28T13:36:39Z_
_Operation: DeleteDashboard | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteDashboard models `clientToken` as an `@idempotencyToken` query member, and the handler explicitly declines to honour it: it removes the dashboard and keeps no token -> answer mapping, so a retry carrying the token of a delete that already landed answers 404 ResourceNotFoundException instead of replaying the success. The comment that justifies this says it is left unhonoured "exactly as `DeleteProject` beside it leaves it to the idempotency issue" — but `delete_project.rs` has since been converted and now runs the crate's shared idempotency engine (`request_fingerprint` / `idempotent_verdict` / `record_idempotent_call`), replaying `DeleteProjectOutput {}` for a token already spent. So the stated justification is false inside this same crate, and DeleteDashboard is the one Monitor delete left behind: DeleteProject, DeletePortal, DeleteAccessPolicy, DeleteWorkspace, DeleteAsset, DeleteAssetModel, DeleteTimeSeries all consult the token and DeleteDashboard alone ignores it. Effect: an SDK-internal retry after a timeout turns a successful DeleteDashboard into a 404 for the caller.

## Evidence

The handler that ignores the member, with the comment that names the sibling:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_dashboard.rs:162-170
    "// An id that resolves to no stored dashboard is the modelled 404 rather than a\n // silent success — which is also what a retry carrying the `clientToken` of a\n // delete that already landed gets, because this store keeps no token -> answer\n // mapping to replay from, exactly as `DeleteProject` beside it leaves it to the\n // idempotency issue.\n let mut store = state.lock();\n if store.dashboards.remove(&input.dashboard_id).is_none() { return Err(not_found(&input.dashboard_id)); }"
  (grep for `client_token` in that file returns only the doc header — the handler never reads `input.client_token`.)

The sibling it points at, which DOES honour it:
  src/ops/delete_project.rs:167-185
    "// `clientToken` makes this delete idempotent, as the member documents: \"A unique case-sensitive identifier that you can provide to ensure the idempotency of the request.\" A retry carries the token of the call it repeats, so it replays that call's success rather than the not-found refusal the removal itself would now produce ...\n let fingerprint = crate::state::request_fingerprint(&[&input.project_id], None);\n match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) { crate::state::TokenVerdict::Replay(_) | crate::state::TokenVerdict::Spent(_) => { return Ok(DeleteProjectOutput {}); } ... }"

The modelled member, as the handler's own generated header quotes it:
  src/ops/delete_dashboard.rs:51-53 — "* `clientToken` (ClientToken, len 36..64, pattern ^\\S{36,64}$, @idempotencyToken) — A unique case-sensitive identifier that you can provide to ensure the idempotency of the request. Don't reuse this client token if a new idempotent request is required."

Test coverage: tests/all/dashboard_delete.rs sends a `clientToken` only on calls that cannot replay — the delete that lands uses no token (line 410: `delete_dashboard(&router, &doomed, None)`) and the token in the 404 table (line 437) was never spent on a success — so the assertions stay green once the token is honoured; only the stale prose at dashboard_delete.rs:397-399 ("a `clientToken` buys no replay of the answer that already went out, exactly as the creates in this crate leave idempotency to its own issue") describes a crate rule that no longer exists. No test sends one token twice to DeleteDashboard.

## Steps to Reproduce

1. CreateProject, then CreateDashboard in it; note the dashboardId.
2. DELETE /dashboards/{dashboardId}?clientToken=3f2a1b0c-9d8e-7f6a-5b4c-3d2e1f0a9b8c-tok → 204.
3. Repeat the identical request with the SAME clientToken (what an SDK retry sends) → 404 ResourceNotFoundException.
4. Do the same with a project: DELETE /projects/{projectId}?clientToken=... twice → the second call replays 204 (delete_project.rs:180-185), so the two deletes of the same crate disagree about the same modelled member.
