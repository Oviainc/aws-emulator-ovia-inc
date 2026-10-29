Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# An entry naming a composite model's property is reported as ResourceNotFoundException, although DescribeAssetProperty answers 200 for that same (assetId, propertyId) pair

_Filed: 2026-10-29T17:38:07Z_
_Operation: BatchGetAssetPropertyAggregates | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resolve_entry` checks an entry's `(assetId, propertyId)` against the asset record's own top-level property list alone, so a property contributed by one of the asset model's composite models resolves to nothing and the entry is pushed into `errorEntries` with `ResourceNotFoundException` instead of being aggregated.

The crate publishes those same property ids as properties of the same asset, addressed by the same pair, through its one shared resolver `composite_model_properties`, which `DescribeAssetProperty` answers 200 from. The already-filed issue on this operation covers only its `nextToken` binding, not this resolution.

## Evidence

The entry resolution — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_get_asset_property_aggregates.rs:365-386:
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
                    "ResourceNotFoundException",
                    format!("The asset {asset_label} has no property {property_id}."),
                ));
            }
```
and that `Err` becomes an entry of `errorEntries` in the output (same file, lines 306-311: `Ok(BatchGetAssetPropertyAggregatesOutput { error_entries, success_entries, skipped_entries, next_token })`).

The sibling read that DOES answer such a pair — src/ops/describe_asset_property.rs:241-251:
```
        if let Some(model) = model {
            for composite in &model.composite_models {
                ...
                let contributed = store.composite_model_properties(model, composite_id);
```
with the resolver declaring itself the one definition of an asset's instance of a composite model — src/state.rs:4369-4379:
```
    /// One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
    pub fn composite_model_properties(
```
Test coverage: tests/all/describe_asset_property.rs pins the 200 for a composite model's property; tests/all/property_aggregates.rs covers top-level properties and aliases only, so no test pins the error entry produced here.

## Steps to Reproduce

1. CreateAssetModel with one assetModelCompositeModels entry carrying a DOUBLE measurement property; CreateAsset from it and wait for ACTIVE.
2. Read the composite property's id P from ListAssetProperties (filter=ALL); DescribeAssetProperty for P answers 200.
3. POST /properties/batch/aggregates with one entry {"entryId": "e1", "assetId": "{assetId}", "propertyId": "P", "aggregateTypes": ["AVERAGE"], "resolution": "1h", "startDate": ..., "endDate": ...}.
4. Observed: errorEntries carries that entry with errorCode ResourceNotFoundException "The asset {assetId} has no property P."
5. Expected, from describe_asset_property.rs:241-251 and state.rs:4369-4379: the entry resolves the property the describe publishes and is answered as a success entry (with its windows, or none).
