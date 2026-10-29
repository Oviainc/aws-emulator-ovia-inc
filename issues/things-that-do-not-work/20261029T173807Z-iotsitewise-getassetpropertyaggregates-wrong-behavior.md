Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# A composite model's property is answered 404 by the aggregates read, although DescribeAssetProperty reports that same (assetId, propertyId) pair as a property of the asset

_Filed: 2026-10-29T17:38:07Z_
_Operation: GetAssetPropertyAggregates | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The (assetId, propertyId) form of this read resolves the property against the asset record's own top-level property list only (`AssetRecord::property_position` over `asset.properties`). The crate publishes a second set of property ids AS properties of the same asset, addressed by the same (assetId, propertyId) pair: the properties contributed by the asset model's composite models, reached through the one shared resolver `ComAmazonawsIotsitewiseAwsiotsitewiseState::composite_model_properties`, which `DescribeAssetProperty` answers 200 from and reports under `compositeModel.assetProperty`.

So a client that takes a property id out of DescribeAssetProperty / ListAssetProperties and asks this operation for that property's aggregates is told the asset has no such property, instead of being answered the (possibly empty) set of windows of a property the service itself says exists. The same defect has already been filed for the sibling reads GetAssetPropertyValueHistory, BatchGetAssetPropertyValue and UpdateAssetProperty; this handler is a further copy of the same resolution and was not covered by any of them (only a pagination issue exists for this operation).

The handler already has the crate's shared lookups on this path (`resolve_asset_id`, `property_position`); only the composite contribution is missing from the resolution.

## Evidence

The resolution, which searches the asset record's own list alone — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_asset_property_aggregates.rs:317-335:
```
            (Some(asset_label), Some(property_label), None) => {
                let asset_id = store.resolve_asset_id(asset_label).ok_or_else(|| {
                    ...
                })?;
                let resolved = store.assets.get(&asset_id).and_then(|asset| {
                    asset
                        .property_position(property_label)
                        .map(|at| asset.properties[at].id.clone())
                });
                let property_id = resolved.ok_or_else(|| {
                    not_found(format!(
                        "The asset {asset_label} has no property {property_label}."
                    ))
                })?;
```
Its own comment claims the resolution is the crate's shared one — same file, lines 306-311:
```
        // A stream is named either by the asset and property ids of the property it
        // is associated with or by its alias; exactly one of the two forms identifies
        // it. Resolved exactly as `GetAssetPropertyValue` resolves it — the asset
        // label through the shared `resolve_asset_id`, the property through
        // `AssetRecord::property_position`, an alias through the shared
        // `resolve_alias_series` — so a stream is named one way across the service.
```
The sibling read of this crate that DOES answer such a pair, through the shared resolver — src/ops/describe_asset_property.rs:241-251:
```
        if let Some(model) = model {
            for composite in &model.composite_models {
                let Some(composite_id) = composite.id.as_deref() else {
                    continue;
                };
                // What a composite model contributes comes from the one shared
                // resolver, so a property contributed by the component model a
                // composite model reuses is describable here exactly as the
                // listings of that composite model report it.
                let contributed = store.composite_model_properties(model, composite_id);
```
and that resolver declares itself the ONE definition, explicitly including an asset's instance of a composite model — src/state.rs:4369-4379:
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
Test coverage: the read side is pinned (tests/all/describe_asset_property.rs drives the CUSTOM composite-model case and asserts 200 for that property id); nothing under tests/all pins a 404 from the aggregates read for such a property — tests/all/property_aggregates.rs and tests/all/single_property_aggregates.rs exercise top-level properties and aliases only.

## Steps to Reproduce

1. CreateAssetModel with one assetModelCompositeModels entry, e.g. {"name": "Shift", "type": "CUSTOM", "properties": [{"name": "Throughput", "dataType": "DOUBLE", "type": {"measurement": {}}}]}.
2. CreateAsset from that model and wait for it to report ACTIVE.
3. GET /assets/{assetId}/properties?filter=ALL and note the property id P of the composite model's Throughput; GET /assets/{assetId}/properties/{P} -> 200, reporting it under compositeModel.assetProperty.
4. GET /properties/aggregates?assetId={assetId}&propertyId={P}&resolution=1h&aggregateTypes=AVERAGE&startDate=...&endDate=... (GetAssetPropertyAggregates).
5. Observed: 404 ResourceNotFoundException "The asset {assetId} has no property {P}."
6. Expected, from describe_asset_property.rs:241-251 and state.rs:4369-4379 declaring composite_model_properties the one definition of an asset's instance of a composite model: the aggregates read resolves the same property the describe publishes, answering its windows (or none).
