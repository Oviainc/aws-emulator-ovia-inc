Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# A different CreateProject reusing a spent clientToken is answered 200 with the FIRST project's id and arn, and creates nothing

_Filed: 2026-10-28T21:06:52Z_
_Operation: CreateProject | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateProject routes its `clientToken` through the crate's shared idempotency registry but collapses the registry's two distinct verdicts into one arm: `TokenVerdict::Replay(replayed) | TokenVerdict::Spent(replayed)`. By the state module's own definition `Spent` is reached only when the stored fingerprint DIFFERS from this request's — i.e. a DIFFERENT request (another portalId / projectName / description / tags) that happens to carry a token already spent. The handler nevertheless returns `Ok(CreateProjectOutput { project_id, project_arn })` built from the project the FIRST call minted, and returns before the store is touched. No project is created for the second request, yet the caller is handed a 201 and an id — an id belonging to a project with a different name, in possibly a different portal. Every later call the caller makes with that id operates on somebody else's project.

The state module documents the opposite answer for this verdict, and about thirty registry-backed handlers of this same crate implement it, including the creates that model a conflict error (CreateAsset, CreateAssetModel, CreateDataset, CreateAssetModelCompositeModel all refuse `Spent`). CreateProject models no conflict error, but it does model `InvalidRequestException` (400) — the terminal client error this crate already uses for "no retry of this call can succeed" — so answering success is a choice, not the only option.

The handler comment defends the collapse by citing CreateDashboard, but that analogy covers only the Replay half: CreateDashboard looks the dashboard up BY TOKEN (`dashboard_for_client_token`) and so can only ever replay the same request, while here the 200 answers a request whose own body was never served.

The identical branch, with the same consequence, is in `create_portal.rs:265-279` (the answer carries the first portal's id, arn, start url and ssoApplicationId) and `create_access_policy.rs:226-237` (the first policy's id and arn, so a VIEWER request can be answered with an ADMINISTRATOR policy's id).

## Evidence

The crate's own definition of the verdict these handlers answer with success:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:9174-9176
    "    /// A DIFFERENT request already spent this token: refuse it, naming the
     /// resource that earlier call acted on.
     Spent(String),"
  and the one place it is produced, src/state.rs:9270-9273 —
    "match self.idempotent_calls.get(&idempotency_key(operation, token)) {
         Some(call) if call.request == request => TokenVerdict::Replay(call.resource_id.clone()),
         Some(call) => TokenVerdict::Spent(call.resource_id.clone()),
         None => TokenVerdict::Serve,
     }"  (so `Spent` means the request body DIFFERS)

The handler that answers it 200 with another project:
  src/ops/create_project.rs:208-221
    "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {
         crate::state::TokenVerdict::Replay(replayed)
         | crate::state::TokenVerdict::Spent(replayed) => {
             // One deleted since leaves the retry nothing to answer from, so that
             // retry is served as the fresh create it now is.
             if let Some(project) = store.projects.get(&replayed) {
                 return Ok(CreateProjectOutput {
                     project_id: project.id.clone(),
                     project_arn: project.arn.clone(),
                 });
             }"
  skipping the write it would otherwise do, src/ops/create_project.rs:275 — "store.projects.insert(record.id.clone(), record);"
  on the strength of src/ops/create_project.rs:193-196 — "This operation's error list declares no conflict error, so the token alone names the call it repeats: a request reusing it is answered from that call rather than refused with an error this operation does not model, exactly as `CreateDashboard` answers a reused token."
  while CreateDashboard's replay is keyed BY THE TOKEN itself, so it cannot answer a different request: src/ops/create_dashboard.rs — "store.dashboard_for_client_token(token)"

The sibling creates of this crate that refuse the same verdict:
  src/ops/create_asset.rs:237 — "crate::state::TokenVerdict::Spent(spent_on) => {"
  src/ops/create_asset_model.rs:271, src/ops/create_dataset.rs:284, src/ops/create_asset_model_composite_model.rs:289 — the same arm, returning the modelled conflict and writing nothing.

The same branch in two more creates:
  src/ops/create_portal.rs:265-279 — "crate::state::TokenVerdict::Replay(replayed)\n | crate::state::TokenVerdict::Spent(replayed) => { ... return Ok(CreatePortalOutput { portal_id: portal.id.clone(), ... }); }"
  src/ops/create_access_policy.rs:226-237 — "... if let Some(policy) = store.access_policies.get(&replayed) { return Ok(CreateAccessPolicyOutput { access_policy_id: policy.id.clone(), access_policy_arn: policy.arn.clone() }); }"

Test coverage: nothing pins the current answer. tests/all/client_token_replay.rs replays only the IDENTICAL create under its own token (e.g. client_token_replay.rs:166-171 sends the same accessPolicyResource twice), and tests/all/project_lifecycle.rs never sends one token with two different project bodies, so the `Spent` path for these three creates is unexercised.

## Steps to Reproduce

1. CreatePortal twice to get portals P1 and P2.
2. POST /projects with {"portalId": P1, "projectName": "First", "clientToken": "11111111-2222-3333-4444-555555555555-aa"} → 201, project A created.
3. POST /projects with {"portalId": P2, "projectName": "Second"} and the SAME clientToken.
4. Observed: 201 carrying project A's projectId and projectArn. No second project exists: GET /projects/{A} reports projectName "First" in portal P1, and no project named "Second" was created.
5. Expected per state.rs:9174-9176 ("refuse it, naming the resource that earlier call acted on"), which CreateAsset/CreateAssetModel/CreateDataset implement for the same verdict: the request is refused and nothing is created.
6. The same reuse against CreatePortal and CreateAccessPolicy answers with the first portal's / first policy's identifiers in the same way.
