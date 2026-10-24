Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# Child resource ARNs carry the request's Region instead of the owning app bundle's, so one response contradicts itself

_Filed: 2026-10-24T23:00:46Z_
_Operation: CreateIngestion | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An app bundle is stored with the Region it was created in, and the crate documents that Region as "the Region this bundle lives in … and the one its arn carries". But the three handlers that mint a resource UNDER a bundle (CreateIngestion, CreateAppAuthorization, CreateIngestionDestination) build the child ARN from `crate::state::request_region(ctx)` — the Region of the CURRENT request — rather than from the stored parent's Region, while they copy the parent's ARN verbatim into the response.

When the create call for a child is signed for a different Region than the bundle was created in (the store serves every Region in one process, and the crate's own test harness already creates bundles in per-Region scopes because only one bundle per Region is allowed), the SAME response body then names two different Regions: `ingestion.arn` is `arn:aws:appfabric:<request-region>:…:appbundle/<id>/ingestion/<id>` while `ingestion.appBundleArn` is `arn:aws:appfabric:<bundle-region>:…:appbundle/<id>`. The ARN the handler mints claims the child is nested under a bundle ARN that does not exist, contradicting the comment that says the ARN "is nested under the bundle that owns it". The same defect applies to the app-authorization ARN and the ingestion-destination ARN, which additionally embed the parent chain in their resource path.

## Evidence

Parent Region is persisted and documented as the ARN's Region — src/state.rs:170-173:
```
    /// The Region the create call was signed for: the Region this bundle lives
    /// in, whose quota it counts against (see [`MAX_APP_BUNDLES_PER_REGION`]),
    /// and the one its [`arn`](Self::arn) carries.
    pub region: String,
```
Child ARN built from the REQUEST's Region, not the parent's — src/ops/create_ingestion.rs:199 `let region = crate::state::request_region(ctx);` and src/ops/create_ingestion.rs:315-320:
```
        // An ingestion's ARN is nested under the bundle that owns it, the way
        // AppFabric's own resource chain nests them.
        let arn = crate::state::appfabric_arn(
            &region,
            &format!("appbundle/{}/ingestion/{id}", bundle.id),
        );
```
while the response's `appBundleArn` is the stored parent ARN (src/ops/create_ingestion.rs:324 `app_bundle_arn: app_bundle_arn.clone(),`, read at lines 208-212).
Same pattern in the siblings: src/ops/create_app_authorization.rs:313-316 (`appfabric_arn(&region, &format!("appbundle/{bundle_key}/appauthorization/{id}"))` with `region` from `request_region` at line 221) and src/ops/create_ingestion_destination.rs:307-313 (`appbundle/{bundle_key}/ingestion/{}/ingestiondestination/{id}` with `region` from line 199).
The harness already signs different calls for different Regions: tests/all/app_bundle_ingestion_lifecycle.rs:42-57 `fixture_region` (`REGION`, then `us-west-2`, `eu-west-1`, …) is used by `fixture_bundle` (lines 160-170), while `call` (line 111) signs for `REGION` — so `create_ingestion` on `fixture_bundle(1)` already mints a child in `us-east-1` under a `us-west-2` bundle. No test asserts on that combination: tests/all/app_bundle_ingestion_lifecycle.rs:865-869 only checks `appBundleArn` for a bundle created in `REGION`.

## Steps to Reproduce

1. POST /appbundles signed for us-west-2 with an empty body -> note appBundle.arn, which contains `:appfabric:us-west-2:`
2. POST /appbundles/{that-arn}/appauthorizations signed for us-east-1 (app=SLACK, authType=apiKey, credential.apiKeyCredential.apiKey=k, tenant={tenantIdentifier:t-1,tenantDisplayName:t-1}) -> 201, and observe appAuthorization.appAuthorizationArn contains `:appfabric:us-east-1:` while appAuthorization.appBundleArn contains `:appfabric:us-west-2:`
3. POST /appbundles/{that-arn}/ingestions signed for us-east-1 (app=SLACK, tenantId=t-1, ingestionType=auditLog) -> 201
4. Observe the single response body naming two Regions: ingestion.arn `arn:aws:appfabric:us-east-1:000000000000:appbundle/<id>/ingestion/<id>` vs ingestion.appBundleArn `arn:aws:appfabric:us-west-2:000000000000:appbundle/<id>`
