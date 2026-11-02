Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# An entry naming a composite model's property is reported as ResourceNotFoundException, although DescribeAssetProperty answers 200 for that same (assetId, propertyId) pair

_Filed: 2026-10-29T17:38:07Z_
_Operation: BatchGetAssetPropertyValueHistory | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resolve_entry` checks an entry's `(assetId, propertyId)` against the asset record's own top-level property list alone, so a property declared by one of the asset model's composite models resolves to nothing and the entry lands in `errorEntries` with `ResourceNotFoundException`.

The crate publishes exactly those property ids as properties of the same asset, under the same `(assetId, propertyId)` pair, through its one shared resolver `composite_model_properties`: `DescribeAssetProperty` answers 200 for such a pair and reports it under `compositeModel.assetProperty`. So a client that takes a property id from DescribeAssetProperty / ListAssetProperties and asks this batch for its history is told the asset has no such property. This is the same defect already filed for the sibling batch read (BatchGetAssetPropertyValue) and for GetAssetPropertyValueHistory; this handler is an uncovered copy of it.

## Evidence

The entry resolution — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_get_asset_property_value_history.rs:371-392:
```
        (Some(asset_label), Some(property_id), None) => {
            // The shared resolver every other handler uses, so one asset is named
            // one way across the service.
            let asset_id = store.resolve_asset_id(asset_label).ok_or_else(|| { ... })?;
            let has_property = store.assets.get(&asset_id).is_some_and(|asset| {
                asset
                    .properties
                    .iter()
                    .any(|property| property.id == property_id)
            });
            if !has_property {
                return Err((
                    ERROR_NOT_FOUND,
                    format!("The asset {asset_label} has no property {property_id}."),
                ));
            }
```
The sibling read that DOES answer such a pair, through the shared resolver — src/ops/describe_asset_property.rs:241-251:
```
        if let Some(model) = model {
            for composite in &model.composite_models {
                ...
                let contributed = store.composite_model_properties(model, composite_id);
```
with that resolver declaring itself the one definition — src/state.rs:4369-4379:
```
    /// One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
    pub fn composite_model_properties(
```
Test coverage: tests/all/describe_asset_property.rs pins the 200 for a CUSTOM composite model's property; tests/all/batch_property_value_history.rs exercises top-level properties and aliases only, so nothing pins the error entry this handler produces for a composite property.

## Steps to Reproduce

1. CreateAssetModel with one assetModelCompositeModels entry carrying a property, e.g. {"name": "Shift", "type": "CUSTOM", "properties": [{"name": "Throughput", "dataType": "DOUBLE", "type": {"measurement": {}}}]}.
2. CreateAsset from that model, wait for ACTIVE, and read the composite property's id P from ListAssetProperties (filter=ALL); DescribeAssetProperty for P answers 200.
3. POST /properties/batch/history with one entry {"entryId": "e1", "assetId": "{assetId}", "propertyId": "P"}.
4. Observed: the entry appears in errorEntries with errorCode ResourceNotFoundException and message "The asset {assetId} has no property P."
5. Expected, from describe_asset_property.rs:241-251 and state.rs:4369-4379: the entry resolves the same property the describe publishes and is answered as a success entry (with its samples, or none).
