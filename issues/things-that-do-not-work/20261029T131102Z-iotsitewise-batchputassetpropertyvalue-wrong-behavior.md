Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# No value can ever be ingested for a composite model's property: the (assetId, propertyId) entry resolves only the asset record's top-level property list

_Filed: 2026-10-29T13:11:02Z_
_Operation: BatchPutAssetPropertyValue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An entry that names a property by `assetId` + `propertyId` is resolved against `AssetRecord::properties` alone. That list is filled by `CreateAsset` from the asset model's TOP-LEVEL `model.properties` only — a property declared by one of the model's composite models (a CUSTOM grouping, an AWS/ALARM model, or a component-model-based composite model) is never materialised onto the asset. Every such entry therefore reports the per-entry error `ResourceNotFoundException` "The asset {assetId} has no property {propertyId}." and no measurement of a composite model's property can ever be written.

The same crate reports exactly those properties AS properties of the asset through its one shared resolver `composite_model_properties`: `DescribeAssetProperty` answers them under `compositeModel.assetProperty`, and `ListAssetProperties` with `filter=ALL` materialises them into the asset's property summaries. So the crate publishes a property id through its reads and then denies that the asset holds it on the ingest path — the property id a client takes from those reads is unusable for writing. Because the alias form is the only other way in, and an alias can only be bound to a property `UpdateAssetProperty` can resolve (which has the identical defect, filed separately), a composite model's property has no reachable ingest path at all.

The helper the ingest path needs already exists in this crate and is already called by five other handlers; it is only the (assetId, propertyId) resolution that goes through the asset record alone.

## Evidence

The entry resolution, which searches only the asset record's own list — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_put_asset_property_value.rs:569-578:
```
    let property = asset
        .properties
        .iter()
        .find(|property| &property.id == property_id)
        .ok_or_else(|| {
            (
                "ResourceNotFoundException",
                format!("The asset {asset_id} has no property {property_id}."),
            )
        })?;
```

That list holds the TOP-LEVEL definitions only — src/ops/create_asset.rs:298-308:
```
        let mut properties: Vec<AssetProperty> = Vec::with_capacity(model.properties.len());
        let mut initial_values: Vec<(String, AssetPropertyValue)> = Vec::new();
        for property in &model.properties {
            let Some(materialised) = crate::state::asset_property_from_model(property) else {
                continue;
            };
```
(nothing in the file walks `model.composite_models`.)

The sibling reads of this same crate that DO report those properties as the asset's, through the shared resolver — src/ops/list_asset_properties.rs:205-224:
```
            for composite in &model.composite_models {
                ...
                for definition in store.composite_model_properties(model, composite_id) {
                    let Some(property) = crate::state::asset_property_from_model(&definition)
                    else {
                        continue;
                    };
                    summaries.push(summarize(&property, Some(composite_id), &prefix));
                }
            }
```
and src/ops/describe_asset_property.rs:237-250: "// A property declared by one of the model's composite models, which this operation reports in `compositeModel` instead: \"If you describe an asset property in a composite model, this response includes the asset property information in compositeModel.\"" ... "let contributed = store.composite_model_properties(model, composite_id);"

The shared helper states it is the ONE definition, explicitly including an asset's instance of a composite model — src/state.rs:4347-4357:
```
    /// The property definitions composite model `composite_id` of `model`
    /// contributes: its OWN definitions, or, for a component-model-based
    /// composite model, the properties of the `COMPONENT_MODEL` it reuses.
    ...
    /// One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
    pub fn composite_model_properties(
```

Test coverage: the read side is pinned — tests/all/asset_model_composite_model_delete.rs:865-878 asserts "the premise every row rests on: the composite model REPORTS the [lent] property". Nothing in tests/all pins the ingest refusal for such a property; the batch-put suites only exercise top-level properties and aliases.

## Steps to Reproduce

1. CreateAssetModel with one `assetModelCompositeModels` entry, e.g. {"name": "Pump", "type": "CUSTOM", "properties": [{"name": "Temp", "dataType": "DOUBLE", "type": {"measurement": {}}}]}.
2. CreateAsset from that model and wait for it to settle (ACTIVE).
3. GET /assets/{assetId}/properties?filter=ALL (ListAssetProperties) -> 200; note the property id P of the composite model's `Temp`. GET /assets/{assetId}/properties/{P} (DescribeAssetProperty) -> 200, reporting it under `compositeModel.assetProperty`.
4. POST /properties (BatchPutAssetPropertyValue) with entries: [{"entryId":"e1","assetId":"{assetId}","propertyId":"{P}","propertyValues":[{"value":{"doubleValue":1.5},"timestamp":{"timeInSeconds":1700000000}}]}].
5. Observed: 200 with `errorEntries` carrying ResourceNotFoundException "The asset {assetId} has no property {P}." and nothing stored — and GET /properties/latest?assetId={assetId}&propertyId={P} 404s for the same reason, so the property can never hold a reading.
6. Expected, from the two sibling reads that resolve P through `composite_model_properties` (src/ops/list_asset_properties.rs:218, src/ops/describe_asset_property.rs:250) and from state.rs:4347-4357 declaring that resolver the one definition for "an asset's instance of" a composite model: the ingest path resolves the same property the reads publish.
