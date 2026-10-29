Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# A clientToken already spent on another CreateWorkspace mints a second workspace instead of the conflict this crate's idempotency rule states

_Filed: 2026-10-28T21:57:32Z_
_Operation: CreateWorkspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateWorkspace does not use the crate's shared idempotency registry. It compares the request's `clientToken` only against the token stored on the workspace that carries THE SAME NAME, so a token is scoped to a name rather than to the call it was spent on. A second CreateWorkspace that reuses a token already spent on a different request — any other `workspaceName`, and likewise any other description/encryption/tags under a new name — finds no workspace under that name, falls straight past the whole idempotency block, and creates a second workspace. The token that was supposed to make the call idempotent protects nothing.

The crate states the opposite answer for this situation in one place and implements it in about thirty handlers: `idempotent_verdict` returns `TokenVerdict::Spent` when the stored fingerprint differs from this request's, and the enum documents `Spent` as "A DIFFERENT request already spent this token: refuse it". The nearest sibling — CreateApplication, the other workspace-scoped create with a `clientToken` — hand-rolls the same rule faithfully: it finds the record by TOKEN (not by name), compares every member the create carries, and answers `ConflictingOperationException` when they differ. CreateWorkspace models that very error (it already raises it for a name collision), so answering it costs nothing. UpdateWorkspace carries the same shape (`input.client_token == record.update_token`, compared only against the workspace the request names), so a token spent updating one workspace is honoured again on another.

## Evidence

The handler, scoping the token to the name instead of to the call:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_workspace.rs:219-245 —
    "        // `clientToken` makes the create idempotent, as the member documents: \"If
        // you retry a request that completed successfully using the same client
        // token, the retry succeeds without performing any further actions.\" A
        // workspace is identified by its name, so a second create of that name
        // either IS that retry — same token — or collides with the workspace the
        // name already belongs to.
        if let Some(existing) = store.workspaces.get(&input.workspace_name) {
            if input.client_token.is_some() && input.client_token == existing.client_token {
                ... return Ok(...)
            }
            return Err(... ConflictingOperationException ...)
        }"
  followed by the unconditional write at create_workspace.rs:269 — "store.workspaces.insert(record.name.clone(), record);"
  (nothing in this file calls `idempotent_verdict` or `record_idempotent_call`, so the token is never registered either.)

The crate's rule for a token a DIFFERENT request already spent:
  src/state.rs:9174-9176 — "    /// A DIFFERENT request already spent this token: refuse it, naming the
    /// resource that earlier call acted on.
    Spent(String),"
  src/state.rs:9270-9274 — "match self.idempotent_calls.get(&idempotency_key(operation, token)) {
            Some(call) if call.request == request => TokenVerdict::Replay(call.resource_id.clone()),
            Some(call) => TokenVerdict::Spent(call.resource_id.clone()),
            None => TokenVerdict::Serve,
        }"

The sibling workspace-scoped create that implements it:
  src/ops/create_application.rs:227-263 — "// ... The retry has to be the SAME request, so the
        // members that identify the application are compared too; a token already
        // spent on a different request collides with the application it made,
        // rather than answering from it.
        if let Some(token) = input.client_token.as_deref() {
            if let Some(existing) = store.applications.values().find(|record| record.client_token.as_deref() == Some(token)) {
                if existing.workspace_name == input.workspace_name && existing.name == input.name && ... { return Ok(created(existing, now)); }
                return Err(... ConflictingOperationException { message: already_using_message(RESOURCE_APPLICATION, \"clientToken\"), ... })"

Test coverage: nothing pins the current answer. tests/all/workspace_lifecycle.rs:816-850 sends the SAME body twice under one token (replay) and the same NAME under a different token (409); it never sends one token with two different workspace names. tests/all/create_workspace_bounds.rs:228-258 only exercises the token's modelled length/pattern bounds.

## Steps to Reproduce

1. POST /workspaces {"workspaceName":"alpha","encryptionConfiguration":{...},"clientToken":"11111111-1111-1111-1111-111111111111"} -> 200, workspace alpha created.
2. POST /workspaces {"workspaceName":"beta","encryptionConfiguration":{...},"clientToken":"11111111-1111-1111-1111-111111111111"} (the SAME token, a different request).
3. Observed: 200 and a second workspace `beta` is created — `store.workspaces.get("beta")` is None, so the token is never consulted.
4. ListWorkspaces -> both alpha and beta, created under one token.
5. Expected per state.rs:9174-9176 and the sibling CreateApplication (create_application.rs:252-263): the second request is refused with ConflictingOperationException naming the workspace the token was spent on, and nothing is created.
6. Same shape against UpdateWorkspace: a token spent updating alpha is accepted again as a fresh update of beta (create_workspace.rs/update_workspace.rs:181 compares it only against the addressed workspace's `update_token`).
