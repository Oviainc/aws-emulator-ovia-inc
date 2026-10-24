Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# DeleteAppBundle deletes a bundle that still holds app authorizations instead of refusing

_Filed: 2026-10-24T06:55:14Z_
_Operation: DeleteAppBundle | Class: validation_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy model's own documentation for DeleteAppBundle states: "Deletes an app bundle. You must delete all associated app authorizations before you can delete an app bundle." On real AppFabric a DeleteAppBundle against a bundle that still holds one or more app authorizations is therefore refused (ConflictException, HTTP 409, is the only precondition-style error the operation models — AccessDenied/InternalServer/Throttling/Validation being the others) and the bundle, its authorizations and its ingestions all survive.

The emulator's handler takes no precondition at all: it resolves the bundle key and unconditionally removes the whole record, cascade-destroying every app authorization, ingestion and ingestion destination nested under it. Expected: the first delete is refused while authorizations exist, and succeeds only after each app authorization has been deleted. Actual: HTTP 204 with the bundle and its entire subtree gone, so the modeled ConflictException is unreachable on this operation and a client that relies on AWS protecting a bundle with live authorizations silently loses state.

Note the emulator already enforces the sibling precondition one level down: DeleteIngestion refuses (ValidationException) while the ingestion is not `disabled` or still owns destinations, citing the same style of documented sentence. DeleteAppBundle is the one documented precondition left unenforced.

## Evidence

Spec: smithy_query(doc, DeleteAppBundle) — "You must delete all associated app authorizations before you can delete an app bundle."; the operation models ConflictException (HTTP 409) and OPERATIONS.md lists no ResourceNotFoundException, so the conflict is the precondition channel.
Handler: src/ops/delete_app_bundle.rs lines 154-172 — `if let Some(key) = store.bundle_key(...) { store.app_bundles.remove(&key); "deleted" }`, no inspection of `bundle.app_authorizations` / `bundle.ingestions`.
Contrast: src/ops/delete_ingestion.rs lines 199-222 enforces the analogous documented preconditions for DeleteIngestion.
Test coverage: tests/all/app_bundle_ingestion_lifecycle.rs `deleting_an_app_bundle_removes_it_and_its_ingestions` (lines 1102-1182) asserts the current cascade — the bundle it deletes holds a connected app authorization (created by the `authorize` helper at lines 135-167, invoked from `create_ingestion` at 169-184) plus an ingestion, and the test asserts HTTP 204. That test pins the divergent behavior and will need to be reworked alongside the fix.

## Steps to Reproduce

1. POST /appbundles with an empty body -> note appBundle.arn
2. POST /appbundles/{arn}/appauthorizations with app=SLACK, authType=apiKey, credential.apiKeyCredential.apiKey=..., tenant={tenantIdentifier:t-1,tenantDisplayName:t-1} -> 201
3. DELETE /appbundles/{arn}
4. Observe HTTP 204 and the authorization gone; real AppFabric refuses the delete (HTTP 409 ConflictException) until DeleteAppAuthorization has removed the authorization
5. GET /appbundles/{arn} -> 404 in the emulator, 200 on AWS
