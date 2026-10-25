Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# assetModelVersion (LATEST | ACTIVE) is silently ignored by the asset-model reads

_Filed: 2026-10-24T10:08:02Z_
_Operation: DescribeAssetModel | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `assetModelVersion=ACTIVE` reports the definition of the model's active version, and `LATEST` the newest one. While an UpdateAssetModel is still settling (the model reports UPDATING), the two selections differ: ACTIVE must answer the pre-update definition.

Actual: describe_asset_model.rs never reads input.asset_model_version — it always answers the single stored definition — and list_asset_model_properties.rs explicitly discards it (`let _ = &input.asset_model_version;`, line 200) on the strength of a now-stale comment that 'UpdateAssetModel, the only operation that could supersede it, is still a 501 scaffold'. UpdateAssetModel IS implemented now and bumps AssetModelRecord.version (and the ETag) on every write, so a version selector that selects nothing is observable: during the UPDATING window a caller asking for ACTIVE is handed the LATEST definition.

## Evidence

Smithy model: DescribeAssetModelRequest.assetModelVersion and ListAssetModelPropertiesRequest.assetModelVersion both target AssetModelVersionFilter with pattern ^(LATEST|ACTIVE)$ and are bound as httpQuery 'assetModelVersion'; DescribeAssetModelResponse.assetModelVersion reports the version read. Handlers: src/ops/describe_asset_model.rs:192-287 (member never referenced), src/ops/list_asset_model_properties.rs:194-200 (member discarded with a stale justification), src/ops/update_asset_model.rs:507 (`record.begin_update(now)` advances version/ETag). Tests: tests/all/update_asset_model.rs and asset_model_properties.rs do not send assetModelVersion.

## Steps to Reproduce

1. CreateAssetModel with one measurement property; wait for ACTIVE and note the property set and eTag.
2. UpdateAssetModel with a different property set (If-Match = the eTag just read) -> 202, model reports UPDATING.
3. Immediately DescribeAssetModel?assetModelVersion=ACTIVE -> the NEW definition is returned (and ListAssetModelProperties?assetModelVersion=ACTIVE likewise), where AWS answers with the active (pre-update) version.
4. Repeat with assetModelVersion=LATEST -> identical response, so the selector has no effect at all.
