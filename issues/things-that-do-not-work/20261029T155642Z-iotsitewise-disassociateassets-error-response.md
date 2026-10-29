Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: cb9254e7b4a839ef1fe220636b4ce55207805929

# The reused-clientToken conflict names an asset ARN rebuilt from the requesting region instead of the one the asset carries, the rule this crate states twice and its own sibling AssociateAssets follows

_Filed: 2026-10-29T15:56:42Z_
_Operation: DisassociateAssets | Class: error-response_

> _Note: this issue was identified against the code base at commit `cb9254e7b4a839ef1fe220636b4ce55207805929`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a `clientToken` was already spent by a DIFFERENT request, `DisassociateAssets` refuses with `ConflictingOperationException` whose `resourceArn` is MANUFACTURED from the current request's region: `crate::state::asset_arn(region, asset_id)`. It never looks the asset up, although the asset the conflicting call acted on (the PARENT of the association) is not removed by a disassociate and so still carries its own stored ARN.

This crate states the opposite rule explicitly, twice, and its direct sibling implements it. `AssociateAssets` — same resource kind, same error, same fingerprint members, same `spent_on` value — reads the stored ARN and only falls back to building one when the asset is gone. `ExecuteAction` says why: the ARN is "read back from the store so the refusal cannot name an ARN no resource of this store has". `CreateGateway` and `UpdateGateway` word the rule as a rule: the conflict names the resource "by its own stored id and the ARN it was minted with, never one rebuilt from this request's region".

The crate also shows that rebuilding is only acceptable when there is nothing to read: `DeleteAsset`'s own helper documents its rebuild as justified "because the asset that call deleted no longer carries an ARN of its own". That justification does not hold here — `DisassociateAssets` deletes no asset, it only removes one hierarchy edge — and `DisassociateAssets`'s helper offers no justification at all.

Consequence: a store reached from two regional endpoints (the crate derives the region per request from the SigV4 credential scope, defaulting to DEFAULT_REGION for an unsigned one) answers the conflict with an ARN that names no asset this store holds, so a client that resolves `resourceArn` to find the colliding resource finds nothing. The same hand-built pattern is on the `token_spent` of `DisassociateTimeSeriesFromAssetProperty` (src/ops/disassociate_time_series_from_asset_property.rs:306-316) and `AssociateTimeSeriesToAssetProperty` (src/ops/associate_time_series_to_asset_property.rs:360), whose target asset likewise survives the call.

## Evidence

The refusal that rebuilds the ARN — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/disassociate_assets.rs:226-228 and 269-282:
```
            crate::state::TokenVerdict::Spent(spent_on) => {
                return Err(token_spent(&region, &spent_on));
            }
...
/// HTTP 409 `ConflictingOperationException` for a `clientToken` a DIFFERENT
/// request already spent: the conflict is with the PARENT asset that request
/// wrote to, so `resourceId`/`resourceArn` name it.
fn token_spent(region: &str, asset_id: &str) -> DisassociateAssetsError {
    DisassociateAssetsError::Modeled(
        DisassociateAssetsErrorModeled::ConflictingOperationException(
            ConflictingOperationException {
                message: crate::state::already_using_message("asset", "clientToken"),
                resource_id: asset_id.to_string(),
                resource_arn: crate::state::asset_arn(region, asset_id),
```

Its direct sibling, which reads the stored ARN first — src/ops/associate_assets.rs:228-239:
```
            crate::state::TokenVerdict::Spent(spent_on) => {
                let arn = store
                    .assets
                    .get(&spent_on)
                    .map(|asset| asset.arn.clone())
                    .unwrap_or_else(|| crate::state::asset_arn(&region, &spent_on));
                return Err(conflicting_operation(
                    crate::state::already_using_message("asset", "clientToken"),
                    &spent_on,
                    &arn,
                ));
            }
```

The rule, stated as a rule, twice in this crate — src/ops/create_gateway.rs:311-316:
```
/// HTTP 409 `ResourceAlreadyExistsException` for a collision on the gateway's own
/// IDENTITY ... while `resourceId`/`resourceArn` point at the gateway the create collided with:
/// its own stored id and the ARN it was minted with, never one rebuilt from this
/// request's region.
```
and src/ops/update_gateway.rs:232-237: "the error's members name \"the resource that conflicts with this operation\" — the gateway holding that name, by its own stored id and the ARN it was minted with, never one rebuilt from this request."
and the reason, src/ops/execute_action.rs:404-410: "The ARN is the one that resource CARRIES, read back from the store so the refusal cannot name an ARN no resource of this store has; one deleted since the action ran leaves only its id to build one from."

The crate's own statement of WHEN rebuilding is justified — src/ops/delete_asset.rs:269-273: "`resourceId`/`resourceArn` name it — built from this request's region, because the asset that call deleted no longer carries an ARN of its own." DisassociateAssets removes no asset: it only calls `store.disassociate_child(...)` (src/ops/disassociate_assets.rs:242-246), so the parent asset is still in `store.assets` with its minted ARN.

That a stored asset ARN is region-stamped at CREATE time, not at read time — src/ops/create_asset.rs:349 (`arn: crate::state::asset_arn(&region, &asset_id),`) with the region from src/state.rs:7065-7071 (`pub fn request_region(parts) -> String` = the SigV4 credential scope region, or DEFAULT_REGION).

Test coverage: nothing distinguishes the two sources today, because the harness signs every request for one region and builds its oracle from that same region — tests/all/client_token_replay.rs:758-766: "/// An ARN of this service as an INDEPENDENT oracle: built here from the region /// the harness signs for and the emulator's account id, rather than from the /// helper the handlers use. fn expected_arn(resource: &str) -> String { format!(\"arn:aws:iotsitewise:{REGION}:{}:{resource}\", engine::EMULATOR_ACCOUNT_ID) }" — and tests/all/asset_hierarchies.rs:417-424 pins AssociateAssets' in-flight conflict ARN in the same single region. So changing DisassociateAssets to read the stored ARN with the same fallback AssociateAssets uses leaves every existing test green.

## Steps to Reproduce

1. Sign a CreateAssetModel + two CreateAsset calls for region R1, and associate the child under a hierarchy of the parent so the pair exists. The parent asset's stored `assetArn` names R1.
2. Send a DisassociateAssets for the pair, signed for R1, carrying clientToken T -> 200; T is now spent on that call, recorded against the parent asset id.
3. Send a DIFFERENT DisassociateAssets request (another parent/hierarchy/child triple) carrying the SAME clientToken T, signed for a different region R2.
4. Observed: 409 ConflictingOperationException whose `resourceId` is the first call's parent asset id but whose `resourceArn` is `arn:aws:iotsitewise:R2:<account>:asset/<parentId>` — an ARN no asset of this store carries, so resolving it finds nothing.
5. Expected, per the rule this crate states in src/ops/create_gateway.rs:311-316 and src/ops/update_gateway.rs:232-237 ("the ARN it was minted with, never one rebuilt from this request's region") and per the sibling src/ops/associate_assets.rs:228-233: the stored `asset.arn` (naming R1), falling back to a built ARN only when the store no longer holds that asset.
