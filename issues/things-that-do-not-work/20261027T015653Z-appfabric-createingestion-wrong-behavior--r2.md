Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5

# CreateIngestion evaluates the app-authorization precondition before the idempotency replay, so a retry of an existing ingestion is refused HTTP 400

_Filed: 2026-10-27T01:56:53Z_
_Operation: CreateIngestion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b4c888ef8ded8aa913bb7b3b9f51a492e4dac1e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateIngestion checks its state-dependent precondition (the bundle must hold a CONNECTED app authorization for the request's (app, tenantId) pair) BEFORE it looks at the spent `clientToken`. Every other create handler in this crate deliberately orders its state-dependent precondition AFTER the replay, and says so in a comment, precisely so that a true retry of a resource that already exists is never refused by it. CreateIngestion breaks its own stated rule: once the authorization is no longer `Connected`, a retry carrying the SAME `clientToken` and the SAME parameters is answered HTTP 400 ValidationException instead of replaying the ingestion the first call created — even though that ingestion still exists and is still reported by GetIngestion/ListIngestions, and the spent token still names it.

The de-connect is reachable with ordinary, modelled calls: UpdateAppAuthorization resets the authorization's status to `PendingConnect` on any applied update (update_app_authorization.rs:271-279), including a credential-only rotation. A client whose first CreateIngestion timed out and that retries with the same token after rotating the credential is told its request was invalid, while the resource exists — the opposite of what the `@idempotencyToken` member promises and of what the same handler guarantees for its quota guard.

## Evidence

Precondition placed BEFORE the replay — src/ops/create_ingestion.rs:250-262:
```
let parent = store.app_bundles.get(&bundle_key);
if parent.and_then(|bundle| bundle.connected_authorization(&input.app, &input.tenant_id)).is_none() {
    ... return Err(CreateIngestionErrorModeled::ValidationException(
        crate::state::app_authorization_not_connected(&input.app, held_status.as_deref()),
    ).into());
}
```
The replay only happens afterwards — src/ops/create_ingestion.rs:283-307: `if let Some(token) = input.client_token.as_deref() { ... let replayed = store.app_bundles.get(&bundle_key).and_then(|bundle| bundle.resolve_ingestion(&spent.arn)) ... if let Some(ingestion) = replayed { return Ok(CreateIngestionOutput { ingestion }); }`

The rule this handler states for its OTHER precondition — src/ops/create_ingestion.rs:318-321: "// A RESOURCE GUARD on the store, answered with the quota error this // operation models. Checked AFTER the idempotent replay above, so a retry // of an ingestion that already exists is never refused by it."

Same rule in the sibling create handlers (both order their state-dependent check after the replay):
- src/ops/create_app_bundle.rs:188-194: "Checked AFTER the idempotent replay above, so a retry of a bundle that already exists is never refused by it." (replay at 167-185, quota check at 194-217)
- src/ops/create_app_authorization.rs:346-348: "Checked AFTER the idempotent replay above, so a // retry of an authorization that already exists is never refused by it." — and its at-most-one-per-triple check sits at 309-325, i.e. after the replay at 286-307.

What makes the authorization fall out of `Connected` — src/ops/update_app_authorization.rs:271-279: "// An applied update invalidates the connect handshake ... record.status = crate::state::APP_AUTHORIZATION_STATUS_PENDING_CONNECT.to_string();"

Model text on the member whose promise is broken — CreateIngestionRequest$clientToken (OPERATIONS.md:60; src/ops/create_ingestion.rs:66-75): "Specifies a unique, case-sensitive identifier that you provide to ensure the idempotency of the request. This lets you safely retry the request without accidentally performing the same operation a second time."

Test coverage: no test pins the current ordering. tests/all/client_token_region_scope.rs:279-289 and tests/all/app_bundle_ingestion_lifecycle.rs:1071-1102 exercise CreateIngestion/CreateAppBundle token replays only while the authorization stays `Connected`, so none of them observes this path.

## Steps to Reproduce

1. POST /appbundles -> note appBundle.arn (bundle B)
2. POST /appbundles/{B}/appauthorizations with app=SLACK, authType=apiKey, credential.apiKeyCredential.apiKey=k1, tenant={tenantIdentifier:t-1,tenantDisplayName:t-1} -> 201 (status PendingConnect)
3. POST /appbundles/{B}/appauthorizations/{authArn}/connect -> 200 (status Connected)
4. POST /appbundles/{B}/ingestions with {app:SLACK, tenantId:t-1, ingestionType:auditLog, clientToken:T} -> 201, note ingestion.arn (ingestion I)
5. PATCH /appbundles/{B}/appauthorizations/{authArn} with {credential:{apiKeyCredential:{apiKey:k2}}} -> 200 (status is reset to PendingConnect)
6. Retry the identical call from step 4 (same clientToken T, same body) -> HTTP 400 ValidationException "The app authorization for app SLACK and the specified tenant is PendingConnect, not Connected", instead of replaying ingestion I
7. GET /appbundles/{B}/ingestions/{I} -> 200, proving the resource the retry was refused for still exists
