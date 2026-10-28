Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# A clientToken already spent on another CreatePipeline mints a second pipeline instead of the conflict this crate's idempotency rule states

_Filed: 2026-10-28T17:34:37Z_
_Operation: CreatePipeline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreatePipeline recognises a retry by looking up the pipeline the request NAMES and comparing that record's stored `client_token`. A token is therefore only ever compared against the one record whose name the new request happens to repeat: a DIFFERENT request reusing a token already spent (same token, another `pipelineName`) finds no record under the new name, falls straight through the replay/collision branch and CREATES A SECOND PIPELINE under that spent token, leaving two records carrying the same `clientToken`.

The crate has a shared idempotency registry whose whole point is this case — `TokenVerdict::Spent` — and about forty handlers (CreateAsset, CreateAssetModel, CreateProject, CreatePortal, CreateDataset, UpdateAsset, DeleteAsset, …) route their token through it. The rule is written out in the crate's own test suite: a different request reusing a spent token is refused `ConflictingOperationException` (409) naming the resource the first call acted on, and changes nothing. CreatePipeline models that very error (it raises it for a name collision) but never consults the registry, so the one condition the rule exists for is answered 200 with a brand-new resource.

The same shape, with the same consequence, is in `create_task.rs:203-231`, `create_workspace.rs:225-244` and `update_workspace.rs:181` (whose replay compares `record.update_token` of the workspace the path names, so the same token spent on another workspace re-performs the update).

## Evidence

The rule, written out by the crate's own suite:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/tests/all/client_token_idempotency.rs:8-16
    "//! The rule these suites pin: a mutating request that carries a `clientToken`
     //! is served ONCE. A repeat of the SAME request under the SAME token is a RETRY
     //! of that call: it is answered from what the first call left behind and
     //! changes nothing further — it mints no second resource, restarts no owed
     //! transition and mints no second version. A DIFFERENT request reusing a token
     //! already spent is refused `ConflictingOperationException` (409) naming the
     //! resource the first call acted on, and changes nothing."

The same rule in the state module the other handlers call:
  src/state.rs:9134-9145
    "/// What the `clientToken` a mutating request carries means for that request.
     pub enum TokenVerdict {
         /// Serve the request: it carries no token, or none this store has seen.
         Serve,
         /// The SAME request already ran under this token, so this one is a retry of
         /// it: answer from the resource named here and write nothing.
         Replay(String),
         /// A DIFFERENT request already spent this token: refuse it, naming the
         /// resource that earlier call acted on.
         Spent(String),
     }"
  and e.g. src/ops/create_project.rs:208 — "match store.idempotent_verdict(Self::NAME, input.client_token.as_deref(), &fingerprint) {"

The handler that skips it (the token is only ever compared inside the branch reached when a pipeline of THAT NAME already exists):
  src/ops/create_pipeline.rs:215-243
    "if let Some(existing) = store.pipelines.get(&store_key) {
         // \"If you retry a request that completed successfully using the same client
         // token, the server returns the cached result from the original successful
         // request without performing the operation again\" ...
         if input.client_token.is_some() && input.client_token == existing.client_token {
             return Ok(CreatePipelineOutput { ... });
         }
         // `pipelineName` \"must be unique within the workspace\" ...
         return Err(CreatePipelineError::Modeled(
             CreatePipelineErrorModeled::ConflictingOperationException(...)));
     }"
  src/ops/create_pipeline.rs:302 — "client_token: input.client_token.clone()," (the token is stored on the record and nowhere else; no `record_idempotent_call`)

Same shape in the siblings:
  src/ops/create_task.rs:203-231 — "if let Some(existing) = store.tasks.get(&store_key) { ... if input.client_token.is_some() && input.client_token == existing.client_token {"
  src/ops/create_workspace.rs:225-232 — "if let Some(existing) = store.workspaces.get(&input.workspace_name) { if input.client_token.is_some() && input.client_token == existing.client_token {"
  src/ops/update_workspace.rs:181 — "if input.client_token.is_some() && input.client_token == record.update_token {"

Test coverage: tests/all/pipeline_lifecycle.rs:926-978 (`a_retried_create_with_the_same_client_token_replays_its_answer`) covers only (a) the same request under the same token and (b) the same NAME under another token; it never sends the same token with a different `pipelineName`, so nothing pins the current answer. tests/all/workspace_lifecycle.rs:816-888 is the same two cases for workspaces.

## Steps to Reproduce

1. CreateWorkspace "plant-a".
2. POST /workspaces/plant-a/pipelines with pipelineName="p1", computations=[], clientToken="11111111-2222-3333-4444-555555555555-aaa" → 200, pipeline p1 created.
3. POST /workspaces/plant-a/pipelines with pipelineName="p2" (any body) and the SAME clientToken="11111111-2222-3333-4444-555555555555-aaa".
4. Observed: 200 and a second pipeline p2 is created under the already-spent token (ListPipelines reports p1 and p2).
5. Expected, per the rule in tests/all/client_token_idempotency.rs:8-16 that every registry-backed sibling implements: 409 ConflictingOperationException naming p1, and nothing created.
6. CreateAsset with the same token reuse (different assetName) answers that 409 — the two creates of this crate disagree.
