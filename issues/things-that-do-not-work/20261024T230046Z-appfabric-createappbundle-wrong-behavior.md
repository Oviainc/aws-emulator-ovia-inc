Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# CreateAppBundle's idempotency token is not scoped to the Region the per-Region bundle lives in, so a replay returns another Region's bundle

_Filed: 2026-10-24T23:00:46Z_
_Operation: CreateAppBundle | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateAppBundle treats an app bundle as a per-Region resource: the record stores the Region the create call was signed for, the quota is counted per Region (`bundles_in_region`), and the minted ARN carries that Region. The spent-token bookkeeping, however, is Region-blind: the key is `operation#token` and the fingerprint contains only `customerManagedKeyArn` plus the tags. So the same clientToken replayed from a DIFFERENT Region (with the same body) is treated as a retry of the first call: the handler answers 201 with the bundle that lives in the FIRST Region — an ARN naming a Region the caller never asked for — and the Region the caller did sign for is left holding no bundle at all, although its own quota is untouched and a tokenless create there would have succeeded.

The treatment is also self-inconsistent inside this one handler: when `customerManagedKeyIdentifier` is a bare key id it is expanded with the request's Region before going into the fingerprint (`customer_managed_key_arn(identifier, &region)`), so the very same cross-Region replay instead yields HTTP 409 `ConflictException` (IdempotentParameterMismatch) for a request whose members are byte-for-byte identical. The sibling create handlers show the intended rule — every parameter that scopes the resource belongs in the fingerprint — and the Region is exactly such a scope for this operation.

## Evidence

The resource is per-Region — src/state.rs:170-173: "The Region the create call was signed for: the Region this bundle lives in, whose quota it counts against (see [`MAX_APP_BUNDLES_PER_REGION`]), and the one its [`arn`](Self::arn) carries." and src/state.rs:716-727 `bundles_in_region`, used at src/ops/create_app_bundle.rs:190 `if store.bundles_in_region(&region) >= crate::state::MAX_APP_BUNDLES_PER_REGION`.
The token scope omits the Region — src/state.rs:1061-1067:
```
/// Keyed by operation as well as token, because an idempotency token is scoped
/// to the operation it was passed to: the same token on `CreateAppBundle` and on
/// `CreateIngestion` is two independent requests.
pub fn client_token_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}
```
and src/ops/create_app_bundle.rs:150-156 builds the fingerprint from `customerManagedKeyArn` and the tags only, then src/ops/create_app_bundle.rs:163-176 replays whatever ARN the token holds:
```
            if let Some(spent) = store.client_tokens.get(&key).cloned() {
                …
                if let Some(bundle) = store.resolve_bundle(&spent.arn) {
                    return Ok(CreateAppBundleOutput {
                        app_bundle: bundle.to_app_bundle(),
                    });
```
The rule the siblings state — src/ops/create_ingestion.rs:253-255: "The PARENT is one of this request's parameters, so it belongs in the fingerprint: the same token under a different bundle is a different request, not a retry of this one."
The inconsistent half: src/ops/create_app_bundle.rs:146-149 expands a bare key id with the request Region (`crate::state::customer_managed_key_arn(identifier, &region)`) before it enters the fingerprint, so the identical cross-Region replay is a 409 instead of a replay.
No test covers a cross-Region token: tests/all/app_bundle_region_quota.rs:208-246 (`every_region_holds_its_own_app_bundle`) creates with `json!({})` and no clientToken, and the token test at tests/all/app_bundle_region_quota.rs:343-358 stays inside one `fixture_region(index)`.

## Steps to Reproduce

1. POST /appbundles signed for us-east-1 with {"clientToken":"11111111-1111-1111-1111-111111111111"} -> 201, appBundle.arn contains `:appfabric:us-east-1:`
2. POST /appbundles signed for us-west-2 with the SAME body -> observe 201 replaying the us-east-1 bundle (its ARN), even though us-west-2 holds no bundle
3. GET /appbundles signed for us-west-2 -> the only bundle listed is the us-east-1 one; the Region the caller created in never got a bundle
4. Repeat with {"clientToken":"1111…","customerManagedKeyIdentifier":"22222222-2222-2222-2222-222222222222"} in both Regions -> the second call now answers 409 ConflictException (IdempotentParameterMismatch) for the identical body, because the fingerprint embeds the request Region through the expanded KMS ARN
