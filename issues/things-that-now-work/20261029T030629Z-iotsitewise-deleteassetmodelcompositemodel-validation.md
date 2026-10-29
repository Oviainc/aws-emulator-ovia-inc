Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# The surviving-formula refusal reads `composite.properties` directly, so deleting a component-model-based composite model out from under a metric that names one of its properties by id is accepted

_Filed: 2026-10-29T03:06:29Z_
_Operation: DeleteAssetModelCompositeModel | Class: validation_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler implements the documented refusal "You can't delete a composite model if a parent asset model exists that contains a property formula expression that depends on" it (quoted in the handler itself) through `formula_dependent`, whose set of "the properties that are going away" is built by reading the RAW stored field `composite.properties` of each doomed composite model.

A component-model-based composite model (one created with `composedAssetModelId`) stores `properties: None` — its properties come from the `COMPONENT_MODEL` it reuses, which is exactly why this crate has ONE resolver for them, `crate::state::ComAmazonawsIotsitewiseAwsiotsitewiseState::composite_model_properties`, documented as the single definition of "the properties of a composite model" and used by `DescribeAssetModelCompositeModel`, `DescribeAssetCompositeModel` and `ListAssetProperties`. Because the delete handler bypasses that resolver, `doomed_properties` is EMPTY for every component-model-based composite model, so a surviving `metric`/`transform` variable whose `value.propertyId` names one of the properties that composite model contributed is not detected and the delete goes through — leaving the formula reading a property id the asset model no longer exposes. The identical request against a CUSTOM composite model is refused 400, and a test pins that refusal.

(The second raw read, in the referrer scan at delete_asset_model_composite_model.rs:425-431, has the same blind spot for a formula declared on a component-model-based composite model.) The `propertyPath` form of the reference is still caught, because `depends_on` also matches a path segment naming a doomed composite model id; only the `propertyId` form — the first case the existing test exercises — slips through.

## Evidence

The raw read, and what the handler claims it computes:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_asset_model_composite_model.rs:400-418 —
    "// The properties the doomed composite models define: the ones that go away,\n    // and so the ones no surviving expression may still read. A composite model\n    // may define none (it may have no properties of its own at all), which is\n    // not a reason to skip the scan ...\n    let doomed_properties: HashSet<&str> = store\n        .asset_models\n        .get(asset_model_id)\n        ...\n        .flat_map(|composite| composite.properties.iter().flatten())\n        .filter_map(|property| property.id.as_deref())\n        .collect();"

The crate's one definition of a composite model's properties, which that line bypasses:
  src/state.rs:4328-4354 — "/// The property definitions composite model `composite_id` of `model`\n/// contributes: its OWN definitions, or, for a component-model-based\n/// composite model, the properties of the `COMPONENT_MODEL` it reuses.\n/// ... One definition of \"the properties of\n/// a composite model\", so every view of one (the model's own, and an asset's\n/// instance of it) answers from the same place.\n    pub fn composite_model_properties(...)" — whose body returns `component.properties.clone()` when `model.composed_asset_model_id(composite_id)` is `Some`.

The siblings that DO go through it:
  src/ops/describe_asset_model_composite_model.rs:228-233 — "That rule is\n        // the shared `composite_model_properties`, so every view of a composite\n        // model's properties — this one and an asset's instance of it — answers\n        // from the same place.\n        ... let properties = store.composite_model_properties(record, &composite_model_id);"
  src/ops/describe_asset_composite_model.rs:218-219 and src/ops/list_asset_properties.rs:218 (`store.composite_model_properties(model, composite_id)`).

Why a component-model-based composite model stores no properties of its own:
  src/ops/update_asset_model_composite_model.rs:433-435 — "// `None` for a component-model-based composite model, which is what it\n        // already stored: its properties are the reused model's.\n        composite.properties = properties;" (and create_asset_model_composite_model.rs:484 sets the same `None`).

The documented rule the handler is implementing:
  src/ops/delete_asset_model_composite_model.rs:326-338 — "// \"You can't delete a composite model if a parent asset model exists\n        // that contains a property formula expression that depends on\" it: a\n        // surviving transform or metric whose variables name a property that is\n        // going away would be left dangling.\n        if let Some((referrer_model, referrer_property)) = formula_dependent(&store, &asset_model_id, &doomed)"

Test coverage: tests/all/asset_model_composite_model_delete.rs:547-677 pins the refusal only for CUSTOM composite models — the `Reference::MetricPropertyId` case builds the source as `{"assetModelCompositeModelName": "Source", "assetModelCompositeModelType": "CUSTOM", "assetModelCompositeModelProperties": [...]}` (lines 586-593) and asserts `status, 400` (line 670-674). No case creates the source with `composedAssetModelId`, so the component-model-based variant is unpinned.

## Steps to Reproduce

1. POST /asset-models to create asset model C (a COMPONENT_MODEL-usable model) carrying one property, e.g. {"name": "Setpoint", "dataType": "DOUBLE", "type": {"measurement": {}}}.
2. POST /asset-models to create asset model M.
3. POST /asset-models/{M}/composite-models with {"assetModelCompositeModelName": "Source", "assetModelCompositeModelType": "COMPONENT_MODEL", "composedAssetModelId": "<C>"} -> 202, note the minted assetModelCompositeModelId S.
4. GET /asset-models/{M}/composite-models/{S} -> 200; read the contributed property's id P out of `assetModelCompositeModelProperties` (it is C's property id, reported through the shared resolver).
5. POST /asset-models/{M}/composite-models with a second CUSTOM composite model whose property is a metric: {"name": "Average", "dataType": "DOUBLE", "type": {"metric": {"expression": "avg(setpoint)", "variables": [{"name": "setpoint", "value": {"propertyId": "<P>"}}], "window": {"tumbling": {"interval": "1h"}}}}} -> 202.
6. DELETE /asset-models/{M}/composite-models/{S}.
7. Observed: 202 — the composite model is removed and the surviving metric still names property P, which the asset model no longer exposes.
8. Expected (the behaviour the same request gets for a CUSTOM source, pinned at tests/all/asset_model_composite_model_delete.rs:670-674): 400 InvalidRequestException "Composite model {S} is referenced by the formula expression of property Average of asset model {M} when delete was called", because `doomed_properties` should be the properties the composite model CONTRIBUTES (crate::state::composite_model_properties), not only the ones it stores itself.
