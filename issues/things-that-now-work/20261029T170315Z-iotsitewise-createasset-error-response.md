Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# The already-exists refusal names an asset ARN rebuilt from the requesting region instead of the one the colliding asset carries, the rule this crate states twice and CreateGateway/CreateTask follow

_Filed: 2026-10-29T17:03:15Z_
_Operation: CreateAsset | Class: error-response_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a create collides on a dimension another asset already holds — a caller-supplied `assetId` that is taken, or an `assetExternalId` already in use — `CreateAsset` answers `ResourceAlreadyExistsException` whose `resourceArn` is MANUFACTURED from the current request's region: `crate::state::asset_arn(region, asset_id)`. The colliding asset is, by construction, in `store.assets` at that moment (the collision was detected by looking it up), and it carries the `arn` it was minted with, but the handler never reads it.

The helper's own doc says the error points at the stored asset ("`asset_id` is THAT asset, the one the caller has to look at"), and this crate states the sourcing rule as a rule in two other handlers, both of which implement it: `CreateGateway`'s identical name-collision refusal reads `existing.arn.clone()` under a doc that says the conflict names the resource "by its own stored id and the ARN it was minted with, never one rebuilt from this request's region", and `CreateTask` reads the stored ARN with a build only as a fallback "so the refusal cannot name an ARN no task of this store has".

Consequence: a store reached from two regional endpoints (the crate derives the region per request from the SigV4 credential scope) answers the collision with an ARN that names no asset this store holds, so a client resolving `resourceArn` to find the asset it collided with finds nothing. The same hand-built pattern is on `CreateAsset`'s `token_spent` helper in the same file, and on `CreateAssetModel`'s `already_exists`/`token_spent` pair (src/ops/create_asset_model.rs:454, 463-472), whose colliding asset model likewise survives in the store with its own ARN.

## Evidence

The refusal that rebuilds the ARN, and its own doc claiming to name the stored asset — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_asset.rs:411-422:
```
/// HTTP 409 `ResourceAlreadyExistsException` for a `dimension` of the request
/// (`ID`, `externalId`) that an asset already holds; `asset_id` is THAT asset,
/// the one the caller has to look at.
fn already_exists(region: &str, asset_id: &str, dimension: &str) -> CreateAssetError {
    CreateAssetError::Modeled(CreateAssetErrorModeled::ResourceAlreadyExistsException(
        ResourceAlreadyExistsException {
            message: crate::state::already_using_message("asset", dimension),
            resource_id: asset_id.to_string(),
            resource_arn: crate::state::asset_arn(region, asset_id),
        },
    ))
}
```
and the two call sites, each of which has just proved the colliding asset is stored — src/ops/create_asset.rs:320-344:
```
        // A caller-supplied id must be globally unique; an absent one is minted.
        let asset_id = match input.asset_id.clone() {
            Some(id) => {
                if store.assets.contains_key(&id) {
                    return Err(already_exists(&region, &id, "ID"));
                }
...
        if let Some(external) = input.asset_external_id.as_deref() {
            if let Some(existing) = store.asset_external_ids.get(external).cloned() {
                return Err(already_exists(&region, &existing, "externalId"));
            }
        }
```

The rule, stated as a rule in this crate and implemented — src/ops/create_gateway.rs:311-327:
```
/// HTTP 409 `ResourceAlreadyExistsException` for a collision on the gateway's own
/// IDENTITY — the unique `gatewayName` a create claims. ... while `resourceId`/`resourceArn` point at the gateway the create collided with:
/// its own stored id and the ARN it was minted with, never one rebuilt from this
/// request's region.
fn already_exists(existing: &crate::state::GatewayRecord) -> CreateGatewayError {
    ...
            resource_id: existing.id.clone(),
            resource_arn: existing.arn.clone(),
```
and the stored-ARN-with-fallback shape a sibling create already uses — src/ops/create_task.rs:226-240: "The ARN the conflict reports is the one the stored task CARRIES, read back from the record rather than rebuilt, so the refusal cannot name an ARN no task of this store has" ... `.map(|task| task.arn.clone()).unwrap_or_else(|| crate::state::task_arn(&region, &workspace_name, &task_name))`.

That a stored asset ARN is region-stamped at CREATE time, not at read time — src/ops/create_asset.rs:349 (`arn: crate::state::asset_arn(&region, &asset_id),`) with `region` from `crate::state::request_region(ctx.parts())` (src/ops/create_asset.rs:~182).

Test coverage: nothing distinguishes the two sources today, because the harness signs every request for one region and builds its oracle from that same region — tests/all/client_token_replay.rs:758-766 ("An ARN of this service as an INDEPENDENT oracle: built here from the region the harness signs for"). So reading the stored `asset.arn` with the same fallback CreateTask uses leaves every existing test green.

## Steps to Reproduce

1. Sign a CreateAssetModel and a CreateAsset for region R1, giving the asset assetExternalId='plant-1'. The stored asset carries assetArn naming R1.
2. Send a second CreateAsset, signed for a DIFFERENT region R2, with the same assetExternalId='plant-1' (or with assetId set to the first asset's id).
3. Observed: 409 ResourceAlreadyExistsException whose resourceId is the first asset's id but whose resourceArn is arn:aws:iotsitewise:R2:<account>:asset/<id> — an ARN no asset of this store carries, so resolving it finds nothing (src/ops/create_asset.rs:414-421).
4. Expected, per the rule this crate states at src/ops/create_gateway.rs:311-316 ("the ARN it was minted with, never one rebuilt from this request's region") and per the sibling src/ops/create_task.rs:233-239: the stored asset.arn (naming R1), building one only if the store no longer holds that asset.
5. The same sequence against CreateAssetModel (src/ops/create_asset_model.rs:323, 340, 463-472) behaves identically.
