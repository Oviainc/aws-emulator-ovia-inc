Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: abe0d61d6a18bd2197d540196c8b7c6e43f8cd4b

# UpdateAssetProperty answers 404 for a composite-model property that DescribeAssetProperty and ListAssetProperties both report as a property of the asset

_Filed: 2026-10-29T11:51:00Z_
_Operation: UpdateAssetProperty | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `abe0d61d6a18bd2197d540196c8b7c6e43f8cd4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateAssetProperty resolves `propertyId` only against the asset record's materialised `properties` list, which `CreateAsset` fills from the asset model's TOP-LEVEL `model.properties` alone. A property declared by one of the model's composite models therefore never resolves, and the update answers `ResourceNotFoundException` "The asset ... has no property ...".

Two sibling reads of this same crate report exactly that property as a property of the asset, through the one shared resolver `composite_model_properties`: DescribeAssetProperty reports it under `compositeModel.assetProperty`, and ListAssetProperties with `filter=ALL` materialises it into the asset's property summaries with `asset_property_from_model`. So the crate reports a property through one path and denies its existence through the write path that the operation's own modelled documentation pairs with that read ("To keep your existing property's alias or notification state, you must include the existing values in the UpdateAssetProperty request. For more information, see DescribeAssetProperty."). The consequence is that no property of a composite model can ever be given a `propertyAlias`, a `propertyNotificationState` or a `propertyUnit` — the alias binding that is also what associates a data stream with that property.

The helper the write path needs already exists in this crate and is already called by five other handlers; only the write paths resolve properties by the asset record alone.

## Evidence

The refusal, which never consults the composite resolver — src/ops/update_asset_property.rs:264-269:
```
        let at = asset.property_position(&input.property_id).ok_or_else(|| {
            not_found(format!(
                "The asset {} has no property {}.",
                input.asset_id, input.property_id
            ))
        })?;
```
and `property_position` searches only the asset record's own list — src/state.rs:1360-1367: "pub fn property_position(&self, label: &str) -> Option<usize> { ... None => self.properties.iter().position(|property| property.id == label) }", a list `CreateAsset` fills from the top-level definitions only — src/ops/create_asset.rs:298-308: "let mut properties: Vec<AssetProperty> = Vec::with_capacity(model.properties.len()); ... for property in &model.properties {".

The sibling read that DOES resolve such a property — src/ops/describe_asset_property.rs:237-250:
```
        // A property declared by one of the model's composite models, which this
        // operation reports in `compositeModel` instead: "If you describe an asset
        // property in a composite model, this response includes the asset property
        // information in compositeModel."
        if let Some(model) = model {
            for composite in &model.composite_models {
                ...
                let contributed = store.composite_model_properties(model, composite_id);
```
and the listing that reports the same properties as the asset's — src/ops/list_asset_properties.rs:218-223: "for definition in store.composite_model_properties(model, composite_id) { let Some(property) = crate::state::asset_property_from_model(&definition) else { continue; }; summaries.push(summarize(&property, Some(composite_id), &prefix)); }".

The shared helper states it is the ONE definition of a composite model's properties — src/state.rs:4375-4378: "One definition of \"the properties of a composite model\", so every view of one (the model's own, and an asset's instance of it) answers from the same place."

The modelled sentence that pairs the update with that read — quoted in src/ops/update_asset_property.rs:51-54: "Updates an asset property's alias and notification state. This operation overwrites the property's existing alias and notification state. To keep your existing property's alias or notification state, you must include the existing values in the UpdateAssetProperty request. For more information, see DescribeAssetProperty."

Test coverage: the read side is pinned — tests/all/describe_asset_property.rs:428 `a_property_of_a_composite_model_is_reported_as_a_composite_model_property` ("case {case:?}: a composite model's property is not reported as the asset's, got {body}"). Nothing pins the update's refusal for such a property: tests/all/asset_property_update.rs:559 `a_label_that_names_no_stored_property_is_a_modelled_refusal` only tries an absent asset id, an absent property id and two absent external ids.

## Steps to Reproduce

1. CreateAssetModel with one `assetModelCompositeModels` entry, e.g. {"name": "Shift", "type": "CUSTOM", "properties": [{"name": "Temp", "dataType": "DOUBLE", "type": {"measurement": {}}}]}.
2. CreateAsset from that model and wait for it to settle.
3. GET /asset-models/{assetModelId} and read the composite model's property id P.
4. GET /assets/{assetId}/properties/{P} (DescribeAssetProperty) -> 200, with the property reported under `compositeModel.assetProperty`; GET /assets/{assetId}/properties?filter=ALL (ListAssetProperties) also lists P as a property of the asset.
5. PUT /assets/{assetId}/properties/{P} with {"propertyAlias": "/plant/1/shift/temp"} (UpdateAssetProperty).
6. Observed: 404 ResourceNotFoundException "The asset {assetId} has no property {P}." — so P can never be given an alias, notification state or unit.
7. Expected, from the two sibling reads that resolve P through `composite_model_properties` (src/ops/describe_asset_property.rs:250, src/ops/list_asset_properties.rs:218) and from the operation's own doc pointing at DescribeAssetProperty: the write resolves the same property the reads report.
