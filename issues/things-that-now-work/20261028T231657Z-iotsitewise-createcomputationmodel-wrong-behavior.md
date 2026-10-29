Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d204f113841562be393bb0d029d4ee5d3cf48852

# A different CreateComputationModel reusing a spent clientToken is answered 202 with the FIRST model's id, arn and status, and creates nothing

_Filed: 2026-10-28T23:16:57Z_
_Operation: CreateComputationModel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d204f113841562be393bb0d029d4ee5d3cf48852`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateComputationModel recognises a retry by the `clientToken` ALONE: it scans the stored models for one carrying that token and returns that model's id, ARN and status, whatever the rest of the request says. So a second, DIFFERENT create — another name, another configuration, another data binding, other tags — that happens to reuse a token is answered 202 with the first model's identifiers and writes nothing: no computation model is created for the body the caller sent, and every later call the caller makes with the returned id operates on a model with a different configuration (and therefore different action definitions).

This crate has one rule for a spent token and states it in three places: `TokenVerdict::Spent` means "A DIFFERENT request already spent this token: refuse it"; the idempotency registry produces that verdict precisely when the stored request fingerprint differs; and the crate's own end-to-end suite spells the rule out ("A DIFFERENT request reusing the token is refused `ConflictingOperationException` (409) naming the resource the first call acted on, and changes nothing"). About forty handlers of this crate implement it — including DeleteComputationModel, i.e. the sibling operation on the very same resource, which answers `Spent` with 409 — and CreateApplication implements the SAME bespoke by-token scan this handler uses while comparing every identifying member and refusing a mismatch with the modelled conflict. CreateComputationModel models `ConflictingOperationException` (409), so the refusal is available to it; nothing about this operation justifies the divergence, and its own comment offers only the id-minting argument ("the token of the create that minted one is what a retry is recognised by"), which says nothing about a request whose body was never served.

The same token-only scan is used by CreateDatasetExportJob (src/ops/create_dataset_export_job.rs:202-212, workspace compared but nothing else), CreateEnrichmentJob (src/ops/create_enrichment_job.rs:229-235) and StartSearch (src/ops/start_search.rs:219-224); folding them onto the comparison CreateApplication already performs fixes them all.

## Evidence

The handler, keyed on the token alone:
  src/ops/create_computation_model.rs:211-227 —
    "// `clientToken` makes the create idempotent, as the member documents: \"A unique
     // case-sensitive identifier that you can provide to ensure the idempotency of
     // the request.\" A computation model is named by the id this call mints, so the
     // token of the create that minted one is what a retry is recognised by.
     if let Some(token) = input.client_token.as_deref() {
         if let Some(existing) = store
             .computation_models
             .values()
             .find(|model| model.client_token.as_deref() == Some(token))
         {
             return Ok(CreateComputationModelOutput {
                 computation_model_id: existing.id.clone(),
                 computation_model_arn: existing.arn.clone(),
                 computation_model_status: existing.status(now),
             });
         }
     }"
  — returning before the write at src/ops/create_computation_model.rs:259 ("let record = crate::state::ComputationModelRecord { ... }") ever happens.

The sibling with the SAME bespoke scan that compares the request and refuses:
  src/ops/create_application.rs:228-264 —
    "// The retry has to be the SAME request, so the
     // members that identify the application are compared too; a token already
     // spent on a different request collides with the application it made,
     // rather than answering from it.
     if let Some(token) = input.client_token.as_deref() {
         if let Some(existing) = store.applications.values().find(|record| record.client_token.as_deref() == Some(token)) {
             if existing.workspace_name == input.workspace_name
                 && existing.name == input.name
                 && existing.idc_instance_arn == input.idc_instance_arn
                 && existing.description == input.description
                 && existing.tags == input.tags.clone().unwrap_or_default()
             { return Ok(created(existing, now)); }
             return Err(... ConflictingOperationException { message: already_using_message(RESOURCE_APPLICATION, \"clientToken\"), resource_id: existing.id.clone(), resource_arn: existing.arn.clone() } ...);"

The same-resource sibling refusing the same verdict:
  src/ops/delete_computation_model.rs:186-188 —
    "crate::state::TokenVerdict::Spent(spent_on) => {
         return Err(token_spent(&region, &spent_on));
     }"
  with src/ops/delete_computation_model.rs:256-261 — "/// HTTP 409 `ConflictingOperationException` for a `clientToken` a DIFFERENT request already spent".

The crate's definition of the rule:
  src/state.rs:9152-9154 — "/// A DIFFERENT request already spent this token: refuse it, naming the
  /// resource that earlier call acted on.
  Spent(String),"
  src/state.rs:9338-9342 — "match self.idempotent_calls.get(&idempotency_key(operation, token)) {
      Some(call) if call.request == request => TokenVerdict::Replay(call.resource_id.clone()),
      Some(call) => TokenVerdict::Spent(call.resource_id.clone()),"
  tests/all/client_token_spent_by_another_request.rs:8-15 — "The rule these suites pin: a `clientToken` is spent by ONE request of an operation. A repeat of the SAME request under that token is a RETRY of that call [...] A DIFFERENT request reusing the token is refused `ConflictingOperationException` (409) naming the resource the first call acted on, and changes nothing: it mints no second resource and performs no second write."

The operation models the refusal: OPERATIONS.md / smithy_query(operation="CreateComputationModel") — errors include `ConflictingOperationException (HTTP 409)` and `ResourceAlreadyExistsException (HTTP 409)`.

Test coverage: nothing pins the current answer for a DIFFERENT request. tests/all/computation_model_lifecycle.rs:822-867 (`a_retried_create_mints_no_second_model`) sends the IDENTICAL body twice under one token and then the same body under a second token — "let body = |token: &str| json!({ \"computationModelName\": \"retried\", ... \"clientToken\": token })" — and never sends two different bodies under one token.

## Steps to Reproduce

1. POST /computation-models with {"computationModelName": "first", "computationModelConfiguration": <anomaly detection A>, "computationModelDataBinding": {...}, "clientToken": "11111111-2222-3333-4444-555555555555-aa"} → 202, model M1 created.
2. POST /computation-models with a DIFFERENT body — {"computationModelName": "second", another computationModelConfiguration and data binding — and the SAME clientToken.
3. Observed: 202 carrying M1's computationModelId, computationModelArn and status. ListComputationModels still reports exactly one model, named "first": the second request created nothing, and the caller holds an id for a model whose configuration (and whose action definitions) are not the ones it asked for.
4. Expected, per state.rs:9152-9154 and tests/all/client_token_spent_by_another_request.rs:8-15, and as CreateApplication and DeleteComputationModel already implement: 409 ConflictingOperationException naming M1, with nothing created.
5. The same reuse against CreateDatasetExportJob, CreateEnrichmentJob and StartSearch replays the first job/search in the same way.
