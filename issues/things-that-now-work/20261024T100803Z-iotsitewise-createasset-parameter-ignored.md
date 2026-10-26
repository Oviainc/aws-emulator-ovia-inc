Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# clientToken is dropped service-wide, so a retried create mints a second resource instead of replaying the first answer

_Filed: 2026-10-24T10:08:02Z_
_Operation: CreateAsset | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every mutating operation in this crate ignores its @idempotencyToken clientToken: no handler reads the member and the store keeps no token -> outcome mapping.

Expected: CreateAsset called twice with the same clientToken creates ONE asset and replays the first response (same assetId/assetArn/assetStatus); a token reused with a DIFFERENT request body is refused. This is what the @idempotencyToken trait and the member's own documentation ('A unique case-sensitive identifier that you can provide to ensure the idempotency of the request') promise, and it is what SDK/CLI automatic retries rely on.

Actual: create_asset.rs lines 197-200 state outright that 'clientToken is NOT honoured for idempotent replay', so a retry (including an SDK-internal retry after a timeout) mints a second asset with a new id. The same omission is documented in create_asset_model.rs:229, update_asset.rs:182, update_asset_model.rs:225, update_asset_property.rs:215, associate_assets.rs:201, associate_time_series_to_asset_property.rs:191 and disassociate_time_series_from_asset_property.rs:236; grep shows `client_token` appears only in src/ops/types/*, never in a handler.

## Evidence

Smithy model: CreateAssetRequest.clientToken targets ClientToken (len 36..64) and carries @idempotencyToken; the same member exists on CreateAssetModel, CreateAssetModelCompositeModel, UpdateAsset, UpdateAssetModel, UpdateAssetProperty, DeleteAsset, DeleteAssetModel, DeleteTimeSeries, AssociateAssets, DisassociateAssets, AssociateTimeSeriesToAssetProperty, DisassociateTimeSeriesFromAssetProperty and ExecuteQuery. Handlers: the comment blocks cited above, each deferring to 'the idempotency issue'. Tests: no test under tests/all/ sends clientToken (grep finds no match), so nothing asserts current behavior.

## Steps to Reproduce

1. CreateAssetModel, wait for ACTIVE.
2. CreateAsset with assetName='Boiler1', assetModelId=<model>, clientToken='c0ffee...' (36+ chars).
3. Repeat the identical CreateAsset call with the SAME clientToken.
4. Observe a second assetId is returned and ListAssets(filter=ALL, assetModelId=<model>) reports two assets, where AWS replays the first asset.
5. Repeat with the same clientToken but a different assetName -> accepted, where AWS refuses the mismatched replay.
