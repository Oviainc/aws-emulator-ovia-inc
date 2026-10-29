Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# CreateAppAuthorization decides its two handler-owned input rules before resolving the app bundle, so a non-existent bundle is answered 400 where every sibling answers 404

_Filed: 2026-10-28T15:46:06Z_
_Operation: CreateAppAuthorization | Class: error-response_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAppAuthorization` evaluates both rules its module header says it owns — the supported-`app` set and the `credential`/`authType` pairing — BEFORE it takes the state lock and resolves `appBundleIdentifier`. Every sibling handler in this same crate that owns one of those two rules resolves the app bundle FIRST and only then applies the rule:

* `CreateIngestion` owns the SAME supported-`app` check (its comment says so verbatim) and runs it AFTER the bundle lookup;
* `UpdateAppAuthorization` owns the SAME `credential`/`authType` pairing check and runs it AFTER both the bundle and the authorization lookup.

The consequence is two different answers to one and the same pair of faults. A request that names a bundle the store does not hold AND an unsupported `app` (e.g. `app="DISCORD"`) is answered:

* `CreateAppAuthorization` -> HTTP 400 `ValidationException` "app DISCORD is not supported" (the bundle is never looked at);
* `CreateIngestion` -> HTTP 404 `ResourceNotFoundException` "The specified app bundle ID is not valid".

The same split exists for the credential rule between `CreateAppAuthorization` (400) and `UpdateAppAuthorization` (404). This contradicts the rule this crate's own suite states for the chain: the app bundle is the SCOPE every nested path is rooted at, so a bundle identifier naming nothing stored is refused as the app bundle. `CreateAppAuthorization` is the one handler on the chain that can refuse a request without ever asking whether the bundle in its own URI exists.

The existing wording suite does not pin either ordering: its `CreateAppAuthorization` row sends `app: "SLACK"` with a matching `apiKeyCredential`, so both of the handler-owned checks pass and the bundle refusal is reached regardless of order.

## Evidence

The deviating handler — both rules decided before the store is even locked:

services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/create_app_authorization.rs:212-217
```
        if !crate::state::is_supported_app(&input.app) {
            return Err(CreateAppAuthorizationErrorModeled::ValidationException(
                crate::state::app_not_supported(&input.app),
            )
            .into());
        }
```
src/ops/create_app_authorization.rs:222-228
```
        let credential_kind = crate::state::credential_kind(&input.credential);
        if credential_kind != crate::state::credential_kind_for_auth_type(&input.auth_type) {
            return Err(CreateAppAuthorizationErrorModeled::ValidationException(
                crate::state::credential_does_not_match_auth_type(&input.auth_type),
            )
            .into());
        }
```
only then, at src/ops/create_app_authorization.rs:240-246:
```
        let mut store = state.lock();
        let Some(bundle_key) = store.bundle_key(&input.app_bundle_identifier) else {
            return Err(CreateAppAuthorizationErrorModeled::ResourceNotFoundException(
                crate::state::app_bundle_id_not_valid(&input.app_bundle_identifier),
            )
            .into());
        };
```

The sibling that owns the SAME app-set rule and orders it the other way — src/ops/create_ingestion.rs:212-218 resolves the bundle first:
```
        let mut store = state.lock();
        let Some(bundle_key) = store.bundle_key(&input.app_bundle_identifier) else {
            return Err(CreateIngestionErrorModeled::ResourceNotFoundException(
                crate::state::app_bundle_id_not_valid(&input.app_bundle_identifier),
            )
            .into());
        };
```
and only then, src/ops/create_ingestion.rs:234-243, which names the other handler as the co-owner of the rule:
```
        // `app` is a plain `String255` in the model (no `@enum`), so the
        // boundary validator cannot refuse an application AppFabric does not
        // support: the set is the server's to check, and it is the same set
        // `CreateAppAuthorization` checks.
        if !crate::state::is_supported_app(&input.app) {
```

The sibling that owns the SAME credential rule and orders it the other way — src/ops/update_app_authorization.rs:210-223 resolves bundle then authorization, and only at :239-247 checks the credential kind:
```
        let mut store = state.lock();
        let Some(bundle) = store.resolve_bundle_mut(&input.app_bundle_identifier) else {
            return Err(UpdateAppAuthorizationErrorModeled::ResourceNotFoundException(
                crate::state::app_bundle_id_not_valid(&input.app_bundle_identifier),
...
        let credential_kind = input.credential.as_ref().map(crate::state::credential_kind);
        if let Some(kind) = credential_kind {
            if kind != crate::state::credential_kind_for_auth_type(&auth_type) {
```

The crate's own stated chain rule — tests/all/chain_read_not_found_attribution.rs:7-9:
```
//! RULE: the app bundle is the SCOPE every nested path is rooted at, so a bundle
//! identifier naming nothing stored is refused as the app bundle; inside a bundle
//! that does resolve, a read that cannot reach what it asks for is refused as the
```

Test coverage: tests/all/app_bundle_refusal_wording.rs:171-186 is the row that exercises `CreateAppAuthorization` against an unknown bundle, and it sends `"app": "SLACK"` with `"credential": {"apiKeyCredential": ...}` / `"authType": "apiKey"` — both handler-owned checks pass, so the ordering is NOT pinned by it. tests/all/app_authorization_and_ingestion_dependency.rs:462-540 (`an_unsupported_app_is_refused_and_writes_nothing`) always creates the bundle first (lines 476-479), so it does not pin the ordering either.

## Steps to Reproduce

1. POST /appbundles with {} -> note nothing; do NOT create any bundle (or delete it) so that the identifier below names nothing
2. POST /appbundles/11111111-2222-4333-8444-555555555555/appauthorizations with body {"app":"DISCORD","authType":"apiKey","credential":{"apiKeyCredential":{"apiKey":"k"}},"tenant":{"tenantIdentifier":"t-1","tenantDisplayName":"t-1"}}
3. Observe HTTP 400 ValidationException "app DISCORD is not supported" — the handler never asked whether the bundle exists
4. POST /appbundles/11111111-2222-4333-8444-555555555555/ingestions with body {"app":"DISCORD","tenantId":"t-1","ingestionType":"auditLog"}
5. Observe HTTP 404 ResourceNotFoundException "The specified app bundle ID is not valid" for the identical pair of faults
6. Repeat the first call with a matching app but a mismatched credential ({"app":"SLACK","authType":"oauth2","credential":{"apiKeyCredential":{"apiKey":"k"}}}) -> HTTP 400, while the same mismatch on PATCH /appbundles/<unknown>/appauthorizations/<unknown> answers HTTP 404
