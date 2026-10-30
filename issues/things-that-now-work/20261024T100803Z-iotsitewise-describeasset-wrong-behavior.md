Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# An asset never reports its composite models: assetCompositeModels / assetCompositeModelSummaries are always null

_Filed: 2026-10-24T10:08:02Z_
_Operation: DescribeAsset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An asset model's composite models are stored and reported by DescribeAssetModel, but nothing is materialised onto the assets instantiated from that model.

Expected: an asset created from a model that holds a composite model (created inline through CreateAssetModel's assetModelCompositeModels, or afterwards through CreateAssetModelCompositeModel) reports that composite model on DescribeAssetResponse.assetCompositeModelSummaries (and, for the legacy view, assetCompositeModels), and the composite model's properties are addressable on the asset — ingestible with BatchPutAssetPropertyValue and readable with GetAssetPropertyValue, with a `path` from the asset root. UpdateAssetModel propagating a new composite model to existing assets is part of the same rule ('Updates an asset model and all of the assets that were created from the model').

Actual: CreateAsset materialises `properties` only from `model.properties` (create_asset.rs lines 253-270) and stores no composite models on AssetRecord at all; DescribeAsset hardcodes `asset_composite_models: None` and `asset_composite_model_summaries: None` (describe_asset.rs lines 210 and 215). Consequently a composite-model property cannot be written (BatchPutAssetPropertyValue's `resolve_entry` only scans `asset.properties`, so the entry fails with ResourceNotFoundException), cannot be read by GetAssetPropertyValue, and the ExecuteQuery `asset_property` view emits a literal Null for the composite-model id column (execute_query.rs lines 1078-1081). DescribeAssetProperty can only report the model-level definition, with alias and notification forced absent (describe_asset_property.rs lines 301-320).

## Evidence

Smithy model: DescribeAssetResponse declares assetCompositeModels (AssetCompositeModels) and assetCompositeModelSummaries (AssetCompositeModelSummaries); CreateAssetModel declares assetModelCompositeModels and CreateAssetModelCompositeModel is implemented, so a composite model is reachable over the wire. Handlers: src/ops/describe_asset.rs:210,215 (both hardcoded None), src/ops/create_asset.rs:253-280 (no composite models materialised), src/ops/describe_asset_property.rs:301-320 (doc comment states 'A composite model's properties are declared on the asset model and this emulator materialises no per-asset instance of them'), src/ops/execute_query.rs:1078-1081 (same admission). Tests: no test under tests/all/ reads assetCompositeModels / assetCompositeModelSummaries (grep for assetCompositeModel finds nothing), so nothing asserts the current behavior.

## Steps to Reproduce

1. CreateAssetModel (e.g. 'Boiler') and wait for ACTIVE.
2. CreateAssetModelCompositeModel on it with assetModelCompositeModelType=CUSTOM, a name, and one measurement property definition.
3. CreateAsset from that asset model and wait for ACTIVE.
4. DescribeAsset on the new asset -> response carries no assetCompositeModelSummaries and no assetCompositeModels, where AWS lists the composite model and its properties.
5. BatchPutAssetPropertyValue addressing the composite model's propertyId on that asset -> errorEntries with ResourceNotFoundException ('the asset has no property ...') instead of being ingested.
