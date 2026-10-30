Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# assetModelCompositeModels reports a component-model-based composite model with no properties, while DescribeAssetModelCompositeModel reports the component's — the two views of one stored composite model disagree

_Filed: 2026-10-29T02:00:20Z_
_Operation: DescribeAssetModel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeAssetModel builds each reported `AssetModelCompositeModel` from the stored record's own `properties` field (`composite.properties.clone()`). For a composite model created with `composedAssetModelId` that field is `None` by design, so this read reports the composite model with no property definitions at all, while DescribeAssetModelCompositeModel — reading the same stored record through `state::composite_model_properties` — reports the component model's properties for it.

The state module states that this must not happen: `composite_model_properties` is documented as the ONE definition of "the properties of a composite model", so "every view of one (the model's own, and an asset's instance of it) answers from the same place" — and "the model's own" view is precisely this operation. The handler itself applies that principle to the summaries member two lines below ("Derived through the shared helper rather than withheld, so the two views of one stored list cannot disagree") but not to the properties it reports.

## Evidence

The direct read — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_asset_model.rs:248-261:
```
        let composite_models = (!top_level.is_empty()).then(|| {
            top_level
                .iter()
                .filter_map(|id| record.composite_model(id))
                .map(|composite| AssetModelCompositeModel {
                    properties: if input.exclude_properties {
                        None
                    } else {
                        composite.properties.clone()
                    },
                    ..composite.clone()
                })
```
beside its own statement of the no-disagreement rule, src/ops/describe_asset_model.rs:262-264:
```
        // The summaries are the SAME stored composite models, viewed by id/name/type
        // and their path from the model root. Derived through the shared helper
        // rather than withheld, so the two views of one stored list cannot disagree.
```

The rule it breaks — src/state.rs:4325-4351: "The property definitions composite model `composite_id` of `model` contributes: its OWN definitions, or, for a component-model-based composite model, the properties of the `COMPONENT_MODEL` it reuses. ... One definition of \"the properties of a composite model\", so every view of one (the model's own, and an asset's instance of it) answers from the same place."

The sibling view that answers differently for the same record — src/ops/describe_asset_model_composite_model.rs:226-233: "A component-model-based composite model has no properties of its own: the component model it reuses provides them ... That rule is the shared `composite_model_properties`, so every view of a composite model's properties — this one and an asset's instance of it — answers from the same place." followed by `let properties = store.composite_model_properties(record, &composite_model_id);`

Why the stored field is empty — src/ops/create_asset_model_composite_model.rs:481-491: `let properties = if composed_asset_model_id.is_some() { None`.

Test coverage: tests/all/asset_model_composite_model.rs exercises composite models that carry inline `assetModelCompositeModelProperties`; no row describes the parent model after creating a composedAssetModelId-based composite model, so nothing pins the empty report.

## Steps to Reproduce

1. CreateAssetModel A with one property "Speed" (DOUBLE measurement).
2. CreateAssetModel B.
3. CreateAssetModelCompositeModel on B with composedAssetModelId = A and no assetModelCompositeModelProperties -> 202.
4. GET /asset-models/B/composite-models/{compositeId} -> 200 with assetModelCompositeModelProperties = ["Speed" ...].
5. GET /asset-models/B (DescribeAssetModel) -> observed: the same composite model appears in assetModelCompositeModels with `properties` absent/null.
6. Expected, per src/state.rs:4325-4351 ("every view of one ... answers from the same place"): the same property list the composite-model read reports.
