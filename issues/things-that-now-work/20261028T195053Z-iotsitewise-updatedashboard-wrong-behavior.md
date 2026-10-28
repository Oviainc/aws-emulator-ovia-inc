Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# An update reusing a clientToken another update already spent answers 200 but writes nothing, so the dashboard silently keeps its old definition

_Filed: 2026-10-28T19:50:53Z_
_Operation: UpdateDashboard | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateDashboard collapses the idempotency registry's `Replay` and `Spent` verdicts into one arm (`TokenVerdict::Replay(replayed) | TokenVerdict::Spent(replayed)`) and returns `Ok(UpdateDashboardOutput {})` for both. `Spent` means a DIFFERENT request (same operation and token, different `dashboardName` / `dashboardDescription` / `dashboardDefinition` fingerprint), which the state module documents as a request to REFUSE. Because the handler returns Ok before `dashboard.apply_update(..)` runs, the second, different update is answered HTTP 200 — the same answer a successful update gives, and the operation's output carries no members to tell them apart — while the stored dashboard keeps the name, description and definition the first call wrote. The caller has no way to learn its edit was discarded.

The handler comment defends this by citing CreateDashboard, but a create's replay returns the resource the first call minted (`create_dashboard.rs:204-211`), so its 200 describes state that exists; an update's 200 asserts a write that never happened. Every registry-backed mutation elsewhere in this crate refuses the `Spent` verdict instead (UpdateAsset, UpdateAssetProperty, UpdateAssetModel, DeleteAsset, AssociateAssets …).

The identical branch, with the same consequence, is in `src/ops/update_project.rs:181-182` (the project keeps its old projectName/description) and `src/ops/update_access_policy.rs:200-201` (the policy keeps its old identity/resource/permission, so a privilege change is silently dropped).

## Evidence

The state module's own meaning of the verdict these handlers answer with success:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:9174-9176
    "    /// A DIFFERENT request already spent this token: refuse it, naming the\n     /// resource that earlier call acted on.\n     Spent(String),"
  and src/state.rs:9270-9273 — "Some(call) if call.request == request => TokenVerdict::Replay(...),\n Some(call) => TokenVerdict::Spent(...)" (so `Spent` is reached only when the request body DIFFERS).

The handler:
  src/ops/update_dashboard.rs:185-196
    "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {
         crate::state::TokenVerdict::Replay(replayed)
         | crate::state::TokenVerdict::Spent(replayed) => {
             // A dashboard deleted since leaves the retry nothing to answer from, ...
             if store.dashboards.contains_key(&replayed) {
                 return Ok(UpdateDashboardOutput {});
             }"
  skipping its own write, src/ops/update_dashboard.rs:207-212 — "dashboard.apply_update(input.dashboard_name, input.dashboard_description, input.dashboard_definition, now);"
  on the strength of src/ops/update_dashboard.rs:169-173 — "This operation's error list declares no conflict error, so the token alone names the call it repeats: a request reusing it is answered from that call rather than refused with an error this operation does not model, exactly as `CreateDashboard` answers a reused token."
  while CreateDashboard's replay really does answer from the first call's resource: src/ops/create_dashboard.rs:204-211 — "if let Some(existing) = store.dashboard_for_client_token(token) { return Ok(CreateDashboardOutput { dashboard_id: existing.id.clone(), dashboard_arn: existing.arn.clone() }); }"

The sibling updates of this crate that refuse the same verdict:
  src/ops/update_asset_property.rs:243-245 — "crate::state::TokenVerdict::Spent(spent_on) => { return Err(token_spent(&region, &spent_on)); }"

The same branch in two more handlers:
  src/ops/update_project.rs:181-182 and src/ops/update_access_policy.rs:200-201 — "crate::state::TokenVerdict::Replay(replayed)\n | crate::state::TokenVerdict::Spent(replayed) => {"

Test coverage: tests/all/client_token_replay.rs:488-502 exercises only the RETRY of the IDENTICAL update ("// The RETRY of the first update: it is answered from the call it repeats and writes nothing"); the second, differing update in that table is always sent under a token of its own (client_token_replay.rs:479-482), so the spent-token-with-a-different-body case is unpinned for UpdateDashboard, UpdateProject and UpdateAccessPolicy.

## Steps to Reproduce

1. CreatePortal → CreateProject → CreateDashboard (dashboardName "First", definition "{\"widgets\":[]}").
2. PUT /dashboards/{dashboardId} with {"dashboardName": "First", "dashboardDefinition": "{\"widgets\":[]}", "clientToken": "11111111-2222-3333-4444-555555555555-aa"} → 200.
3. PUT /dashboards/{dashboardId} with {"dashboardName": "Second", "dashboardDefinition": "{\"widgets\":[1]}"} and the SAME clientToken.
4. Observed: 200 (indistinguishable from a successful update).
5. GET /dashboards/{dashboardId} → dashboardName is still "First" and the definition is unchanged: the second update was silently discarded.
6. Compare: the identical token reuse against UpdateAssetProperty is refused 409 ConflictingOperationException and changes nothing — the rule state.rs:9174-9176 states for this verdict.
