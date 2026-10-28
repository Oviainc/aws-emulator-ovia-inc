Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a67556f919df77bd586efc9ebe2e6b88c4f9f9e7

# A different request reusing a spent clientToken is answered 200 with an empty errors list while associating nothing

_Filed: 2026-10-28T19:50:53Z_
_Operation: BatchAssociateProjectAssets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a67556f919df77bd586efc9ebe2e6b88c4f9f9e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

BatchAssociateProjectAssets routes its `clientToken` through the crate's shared idempotency registry, but it collapses the registry's two distinct verdicts into one arm: `TokenVerdict::Replay(..) | TokenVerdict::Spent(..)`. `Spent` is, by the state module's own definition, a DIFFERENT request (same operation, same token, different fingerprint) — not a retry. The handler nevertheless returns `Ok(BatchAssociateProjectAssetsOutput { errors: Some(Vec::new()) })`, i.e. HTTP 200 with "no per-asset errors", and falls out of the handler before the membership is touched. The asset ids the second request named are never associated with the project, yet the caller is told the batch succeeded with zero failures. A later ListProjectAssets reports only the first call's assets.

The state module states the opposite rule for this verdict, and about forty registry-backed handlers of this same crate implement it (e.g. UpdateAssetProperty, AssociateAssets, DisassociateAssets return the modelled conflict and change nothing). The two batch project-asset handlers are the only ones that turn the verdict into a success: the identical branch is in `batch_disassociate_project_assets.rs:194-205`, where a different request under a spent token is answered 200 with `errors: []` while the named assets stay in the project.

The handler comment justifies the collapse by pointing at CreateDashboard, but that analogy does not hold: CreateDashboard's replay returns the dashboard the first call minted (`create_dashboard.rs:204-211`), so its 200 describes state that really exists, whereas here the 200 asserts an association that was never recorded.

## Evidence

The crate's own definition of the verdict the handler treats as success:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:9174-9176
    "    /// A DIFFERENT request already spent this token: refuse it, naming the\n     /// resource that earlier call acted on.\n     Spent(String),"

The handler that answers it 200 instead:
  src/ops/batch_associate_project_assets.rs:177-190
    "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {
         crate::state::TokenVerdict::Replay(replayed)
         | crate::state::TokenVerdict::Spent(replayed) => {
             // A project deleted since leaves the retry nothing to answer from, so
             // that retry is served as the fresh association it now is ...
             if store.projects.contains_key(&replayed) {
                 return Ok(BatchAssociateProjectAssetsOutput {
                     errors: Some(Vec::new()),
                 });
             }"
  and the write it skips, src/ops/batch_associate_project_assets.rs:252 — "project.asset_ids = associated;"

The sibling handlers of this crate that implement the documented rule:
  src/ops/update_asset_property.rs:243-245
    "crate::state::TokenVerdict::Spent(spent_on) => {
         return Err(token_spent(&region, &spent_on));
     }"
  src/ops/associate_assets.rs:228-239 — same `Spent` arm, `conflicting_operation(...)`, "a token already spent on a DIFFERENT request belongs to another call, which is the modelled conflict."

The same branch in the sibling batch handler:
  src/ops/batch_disassociate_project_assets.rs:194-204 — "crate::state::TokenVerdict::Replay(replayed)\n | crate::state::TokenVerdict::Spent(replayed) => { ... return Ok(BatchDisassociateProjectAssetsOutput { errors: Some(Vec::new()) });"

Test coverage: nothing pins the current answer. tests/all/project_assets.rs:329-334 deliberately gives every request its own token — "// A token is spent by ONE request: `clientToken` makes the call\n// idempotent, so each of a row's requests carries a token of its own" — so the spent-token-with-different-assetIds case is never exercised, and tests/all/client_token_replay.rs:912-978 only replays the IDENTICAL disassociate request under its own token.

## Steps to Reproduce

1. CreatePortal, then CreateProject in it; CreateAsset twice to get assets A and B.
2. POST /projects/{projectId}/assets/associate with {"assetIds": [A], "clientToken": "11111111-2222-3333-4444-555555555555-aa"} → 200, membership = [A].
3. POST /projects/{projectId}/assets/associate with {"assetIds": [B]} and the SAME clientToken.
4. Observed: 200 with "errors": [] — the caller is told B was associated.
5. GET /projects/{projectId}/assets → only A is listed; B was silently dropped and nothing in the response said so.
6. Compare: the same token reuse against AssociateAssets or UpdateAssetProperty is refused 409 ConflictingOperationException and changes nothing, which is what state.rs:9174-9176 says this verdict means.
