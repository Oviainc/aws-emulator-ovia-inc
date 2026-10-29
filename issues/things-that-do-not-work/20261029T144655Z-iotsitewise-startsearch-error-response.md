Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 58d3cdd8a54717975c7a5e45d2c60ee5b89f816e

# StartSearch refuses a reused clientToken with a ConflictingOperationException naming a searchId the store no longer holds, where its own replay path and every sibling serve the request fresh

_Filed: 2026-10-29T14:46:55Z_
_Operation: StartSearch | Class: error-response_

> _Note: this issue was identified against the code base at commit `58d3cdd8a54717975c7a5e45d2c60ee5b89f816e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSearch resolves `clientToken` through the crate's shared idempotency registry. Its two verdict arms disagree about a resource that has gone since the token was spent:

- the `Replay` arm looks the search up in the store and, when it is no longer there, falls through and serves the call as the fresh start it now is (its comment says exactly that);
- the `Spent` arm never looks: it rebuilds a search ARN from the token's stored key and returns ConflictingOperationException, whose `resourceId`/`resourceArn` are required members, so the 409 names a search no workspace holds.

Every other handler of this crate that reports the conflict for a spent token reads the resource out of the store first and falls through to `Serve` when it is gone, and says why. A workspace delete forgets that workspace's searches (and `workspace_contents` does not count searches, so it is not refused), so the dangling case is reachable: the caller is then permanently blocked on that token — the request can never be served and the error it gets points at a resource that does not exist.

## Evidence

The two arms, in one handler:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/start_search.rs:227-237 —
    "            crate::state::TokenVerdict::Replay(replayed) => {
                // The retry reports that search at whatever status it has reached by
                // now. One gone since — its workspace deleted — leaves the retry
                // nothing to answer from, so it is served as the fresh start it now
                // is.
                let (workspace_name, search_id) =
                    crate::state::workspace_scoped_token_key(&replayed);
                if let Some(existing) = store.search(&workspace_name, &search_id) {
                    return Ok(started(existing, now));
                }
            }"
  src/ops/start_search.rs:238-248 —
    "            crate::state::TokenVerdict::Spent(spent_on) => {
                // The conflict names the search the token's own call started — the
                // search the caller has to look at — as the error's required members
                // state it.
                let (workspace_name, search_id) =
                    crate::state::workspace_scoped_token_key(&spent_on);
                return Err(token_spent(
                    &search_id,
                    crate::state::search_arn(&region, &workspace_name, &search_id),
                ));
            }"

The rule the siblings state and implement for the same verdict:
  src/ops/batch_associate_data_segments_to_dataset.rs:257-268 — "crate::state::TokenVerdict::Spent(spent_on) => {
                // The ARN the conflict reports is the one the stored dataset CARRIES; a
                // dataset removed since the token was spent leaves no resource to name,
                // so such a request is served as the fresh call it now is.
                let conflicting = store.datasets.get(&spent_on)...; if let Some((dataset_id, arn)) = conflicting { return Err(token_spent(&dataset_id, arn)); }"
  src/ops/put_asset_model_interface_relationship.rs:225-236 — "// The ARN the conflict reports is the one the stored model CARRIES; a model removed since the token was spent leaves no resource to name, so such a request is served as the fresh call it now is."
  src/ops/delete_asset_model_interface_relationship.rs:205-216 — the same arm, same comment.
  src/ops/execute_action.rs:219-227 — "if let Some((resource_id, resource_arn)) = store.actions.get(&spent_on).and_then(...) { return Err(token_spent(resource_id, resource_arn)); }" (otherwise falls through).

Why the dangling case is reachable:
  src/state.rs:5636-5641 — "pub fn forget_workspace(&mut self, workspace_name: &str) -> Option<WorkspaceRecord> { let record = self.workspaces.remove(workspace_name)?; self.searches.retain(|(workspace, _), _| workspace != workspace_name);"
  src/state.rs:5576-5580 — "The records this store keys by a workspace but a caller cannot CREATE one at a time (a search, a query, an enrichment job, ...) are NOT contents in this sense" (so DeleteWorkspace is not refused because searches exist).

Test coverage: tests/all/search_client_token_names_the_call.rs and tests/all/search_lifecycle.rs exercise the replay and the conflict while the search still exists; neither deletes the workspace first, so nothing pins the dangling refusal.

## Steps to Reproduce

1. CreateWorkspace W.
2. StartSearch in W with clientToken T and statement S1 -> 202, searchId X.
3. DeleteWorkspace W (accepted: searches are not workspace contents) — X is forgotten.
4. CreateWorkspace W again.
5. StartSearch in W with the SAME clientToken T but statement S2 (a different request, so the registry verdict is `Spent`).
6. Observed: 409 ConflictingOperationException whose resourceId is X and whose resourceArn is a search ARN rebuilt for X, although no workspace holds X; no search is started, and every retry under T answers the same.
7. Expected per this handler's own Replay comment (start_search.rs:229-231) and the siblings quoted above: with the resource the token was spent on gone, the request is served as the fresh start it now is.
