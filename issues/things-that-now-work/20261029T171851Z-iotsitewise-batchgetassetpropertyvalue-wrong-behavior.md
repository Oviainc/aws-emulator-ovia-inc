Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# An entry naming a composite model's property is reported as ResourceNotFoundException, although DescribeAssetProperty answers 200 for that same (assetId, propertyId) pair

_Filed: 2026-10-29T17:18:51Z_
_Operation: BatchGetAssetPropertyValue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resolve_entry` resolves an entry's (assetId, propertyId) pair against `AssetRecord::properties` alone. That list is filled by `CreateAsset` from the asset model's TOP-LEVEL `model.properties` only (`src/ops/create_asset.rs` never mentions a composite model), so every entry naming a property declared by one of the model's composite models lands in `errorEntries` with `ResourceNotFoundException` "The asset {assetId} has no property {propertyId}." — and, on a paginated request, is reported in `skippedEntries` with completionStatus ERROR for the same reason.

The same crate publishes those property ids AS properties of the asset under the identical (assetId, propertyId) addressing: `DescribeAssetProperty` answers 200 and reports the property under `compositeModel.assetProperty`, and `ListAssetProperties` with filter=ALL materialises them into the asset's property summaries — both through the one shared resolver `composite_model_properties`, which documents itself as the single definition of a composite model's properties for "every view of one (the model's own, and an asset's instance of it)". A batch read of ids the service itself hands out therefore reports them as absent.

## Evidence

The entry resolution — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/batch_get_asset_property_value.rs:317-328:
```
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
(`ERROR_NOT_FOUND` is "ResourceNotFoundException", src/ops/batch_get_asset_property_value.rs:355.)

That list holds top-level definitions only: `src/ops/create_asset.rs` contains no occurrence of `composite` (grep: "No matches"), so a composite model's property is never materialised onto the asset record.

The sibling read that resolves the same pair through the shared resolver — src/ops/describe_asset_property.rs:241-250:
```
        if let Some(model) = model {
            for composite in &model.composite_models {
                ...
                let contributed = store.composite_model_properties(model, composite_id);
```
and src/ops/list_asset_properties.rs:218 (`for definition in store.composite_model_properties(model, composite_id) {`).

The resolver states it is the one definition — src/state.rs:4376-4379:
```
    /// model's stored record rather than copied at create time, so a later change
    /// to the component model shows through. One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
```

Test coverage: tests/all/describe_asset_property.rs:515-519 asserts the 200 for a CUSTOM composite model's property ("case {case:?}: DescribeAssetProperty got {status} {body}"), so the id this batch read rejects is one the suite proves the service publishes. Nothing in tests/all pins the batch rejection: `grep "has no property"` over tests/all returns no match, and tests/all/batch_get_latest_values.rs contains no `composite` case.

## Steps to Reproduce

1. CreateAssetModel with an `assetModelCompositeModels` entry declaring one measurement property.
2. CreateAsset from that model; wait for ACTIVE.
3. GET /assets/{assetId}/properties/{P} (DescribeAssetProperty) for the composite property id P -> 200 with `compositeModel.assetProperty`.
4. POST /properties/batch/latest (BatchGetAssetPropertyValue) with entries: [{"entryId":"e1","assetId":"{assetId}","propertyId":"{P}"}].
5. Observed: 200 with `errorEntries` carrying errorCode ResourceNotFoundException and "The asset {assetId} has no property {P}.", and no `successEntries`.
6. Expected, from src/ops/describe_asset_property.rs:241-250 + src/state.rs:4376-4379: the entry resolves the same property the describe publishes and is answered as a success entry (with no `assetPropertyValue` if nothing has been written to it).
