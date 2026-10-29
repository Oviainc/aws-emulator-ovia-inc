Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# A composite model's property is answered 404 by the history read, although DescribeAssetProperty reports that same (assetId, propertyId) pair as a property of the asset

_Filed: 2026-10-29T17:18:51Z_
_Operation: GetAssetPropertyValueHistory | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The (assetId, propertyId) form of this read checks the property against `AssetRecord::properties` alone. That list is filled by `CreateAsset` from the asset model's TOP-LEVEL `model.properties` only — nothing in `create_asset.rs` mentions a composite model at all (grep for `composite` in that file returns no match) — so a property declared by one of the model's composite models (a CUSTOM grouping, an AWS/ALARM model, or a component-model-based composite model) is never on it.

The same crate publishes exactly those property ids AS properties of the asset, addressed by the same (assetId, propertyId) pair, through its one shared resolver `composite_model_properties`: `DescribeAssetProperty` answers 200 for such a pair and reports it under `compositeModel.assetProperty`, and the crate's own test suite pins that answer. So a client that takes a property id from `DescribeAssetProperty`/`ListAssetProperties` and asks this operation for that property's history is told the asset has no such property, instead of being answered the (possibly empty) history of a property the service says exists.

The handler already has the crate's shared lookups available on this path (`resolve_asset_id`, and `AssetRecord::property_position`, which eight sibling handlers call); only the composite contribution is missing from the resolution.

## Evidence

The resolution, which searches the asset record's own list only — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/get_asset_property_value_history.rs:239-249:
```
                let has_property = store.assets.get(&asset_id).is_some_and(|asset| {
                    asset
                        .properties
                        .iter()
                        .any(|property| property.id == property_id)
                });
                if !has_property {
                    return Err(not_found(format!(
                        "The asset {asset_label} has no property {property_id}."
                    )));
                }
```

That list never receives a composite model's properties: `src/ops/create_asset.rs` contains no occurrence of `composite` at all (grep pattern `composite` over that file: "No matches"), so only `model.properties` is materialised.

The sibling read of this same crate that DOES answer such a pair, through the shared resolver — src/ops/describe_asset_property.rs:237-250:
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

The shared resolver declares itself the ONE definition, explicitly including an asset's instance of a composite model — src/state.rs:4369-4379:
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

Test coverage: the read side is pinned — tests/all/describe_asset_property.rs:499-523 drives the CUSTOM composite model case and asserts the describe answers 200 for that property id:
```
        (
            "measurement with a unit of the custom composite model",
            "Shift",
            "CUSTOM",
            "Throughput",
...
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetProperty got {status} {body}"
        );
```
Nothing under tests/all pins a 404 from the history read for such a property: `grep "has no property"` over tests/all returns no match, and the history suites (tests/all/property_value_history.rs) exercise top-level properties and aliases only.

## Steps to Reproduce

1. CreateAssetModel with one `assetModelCompositeModels` entry, e.g. {"name": "Shift", "type": "CUSTOM", "properties": [{"name": "Throughput", "dataType": "DOUBLE", "type": {"measurement": {}}}]}.
2. CreateAsset from that model and wait for it to report ACTIVE.
3. GET /assets/{assetId}/properties?filter=ALL (ListAssetProperties) and note the property id P of the composite model's `Throughput`; GET /assets/{assetId}/properties/{P} (DescribeAssetProperty) -> 200, reporting it under `compositeModel.assetProperty`.
4. GET /properties/history?assetId={assetId}&propertyId={P}&... (GetAssetPropertyValueHistory).
5. Observed: 404 ResourceNotFoundException "The asset {assetId} has no property {P}."
6. Expected, from src/ops/describe_asset_property.rs:237-250 and src/state.rs:4369-4379 declaring `composite_model_properties` the one definition for "an asset's instance of" a composite model: the history read resolves the same property the describe publishes (answering its samples, or an empty history).
