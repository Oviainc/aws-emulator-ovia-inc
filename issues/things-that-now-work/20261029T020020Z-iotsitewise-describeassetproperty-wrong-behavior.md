Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# A property of a component-model-based composite model is answered 404, because the composite branch reads `composite.properties` instead of the crate's one composite-model property resolver

_Filed: 2026-10-29T02:00:20Z_
_Operation: DescribeAssetProperty | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeAssetProperty's composite-model branch looks for the requested property only in the composite model's OWN stored `properties` list (`composite.properties.iter().flatten()`). A composite model created with `composedAssetModelId` stores `properties: None` by design — the component model it reuses provides them — so for every property of such a composite model this loop finds nothing and the handler falls through to its 404 ("The asset {} has no property {}.").

That is wrong twice over. First, the handler's own quoted contract says the property must be reported: "If you describe an asset property in a composite model, this response includes the asset property information in compositeModel." Second, the crate already has exactly one resolver for this question — `ComAmazonawsIotsitewiseAwsiotsitewiseState::composite_model_properties`, whose doc declares itself the single definition of "the properties of a composite model" so that "every view of one (the model's own, and an asset's instance of it) answers from the same place". DescribeAssetCompositeModel and ListAssetProperties both read through it, so the SAME property of the SAME asset is listed by ListAssetProperties and reported by DescribeAssetCompositeModel while DescribeAssetProperty says the asset has no such property.

## Evidence

The composite branch reads the stored list directly — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_asset_property.rs:237-243:
```
        // A property declared by one of the model's composite models, which this
        // operation reports in `compositeModel` instead: "If you describe an asset
        // property in a composite model, this response includes the asset property
        // information in compositeModel."
        if let Some(model) = model {
            for composite in &model.composite_models {
                let Some(definition) = composite.properties.iter().flatten().find(|property| {
```
and when the loop finds nothing, src/ops/describe_asset_property.rs:277-280:
```
        Err(not_found(format!(
            "The asset {} has no property {}.",
            input.asset_id, input.property_id
        )))
```

Why that list is empty for a component-model-based composite model — src/ops/create_asset_model_composite_model.rs:481-491:
```
        // A component-model-based composite model has no properties of its own: the
        // component model it reuses provides them, and
        // `DescribeAssetModelCompositeModel` reads them from there.
        let properties = if composed_asset_model_id.is_some() {
            None
```

The one resolver this handler skips — src/state.rs:4325-4351:
```
    /// The property definitions composite model `composite_id` of `model`
    /// contributes: its OWN definitions, or, for a component-model-based
    /// composite model, the properties of the `COMPONENT_MODEL` it reuses.
    ///
    /// ... One definition of "the properties of
    /// a composite model", so every view of one (the model's own, and an asset's
    /// instance of it) answers from the same place.
    pub fn composite_model_properties(
```

Siblings that do read through it, for the same asset and the same composite model:
- src/ops/describe_asset_composite_model.rs:218-219 — `let properties = store\n            .composite_model_properties(model, &composite_model_id)`
- src/ops/list_asset_properties.rs:218 — `for definition in store.composite_model_properties(model, composite_id) {`
- src/ops/describe_asset_model_composite_model.rs:226-233 — "A component-model-based composite model has no properties of its own: the component model it reuses provides them ... That rule is the shared `composite_model_properties`, so every view of a composite model's properties — this one and an asset's instance of it — answers from the same place."

Test coverage: tests/all/describe_asset_property.rs covers composite models only in the form that stores its own definitions (grep for `composed` in that file matches nothing; the suite's composite rows at tests/all/describe_asset_property.rs:428-543 build "Shift"/alarm composite models with inline `properties`), so nothing pins the component-model case either way.

## Steps to Reproduce

1. CreateAssetModel A declaring one property (for example a DOUBLE measurement "Speed"), and note its assetModelId and the property's id.
2. CreateAssetModel B (the host).
3. CreateAssetModelCompositeModel on B with assetModelCompositeModelType="AWS/COMPONENT_MODEL"-style component reuse, i.e. composedAssetModelId = A, and no assetModelCompositeModelProperties.
4. GET /asset-models/B/composite-models/{compositeId} (DescribeAssetModelCompositeModel) -> 200 and assetModelCompositeModelProperties contains A's "Speed" property.
5. CreateAsset from model B, then GET /assets/{assetId}/properties (ListAssetProperties?filter=ALL) -> the "Speed" property is listed, carrying the composite model's id.
6. GET /assets/{assetId}/properties/{speedPropertyId} (DescribeAssetProperty) -> observed 404 ResourceNotFoundException "The asset {assetId} has no property {speedPropertyId}."
7. Expected, per the sentence the handler quotes and per the shared resolver the siblings use: 200 with the property reported under compositeModel.
