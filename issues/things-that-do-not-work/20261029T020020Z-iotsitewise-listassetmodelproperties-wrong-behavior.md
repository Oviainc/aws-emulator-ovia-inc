Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# filter=ALL omits the properties of a component-model-based composite model, because the walk reads `composite.properties` instead of the shared resolver its asset-level twin uses

_Filed: 2026-10-29T02:00:20Z_
_Operation: ListAssetModelProperties | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

With `filter=ALL` the listing walks each composite model's own stored `properties` list (`composite.properties.as_deref().unwrap_or(&[])`). A composite model created with `composedAssetModelId` stores `properties: None` — the component model it reuses provides them — so none of that composite model's properties appear in the page, although `filter` is documented as "ALL – The list includes all asset model properties for a given asset model ID" and the handler's own comment says ALL "also walks the properties each of its composite models defines".

The crate has exactly one resolver for "the properties of a composite model" (`state::composite_model_properties`), documented as the single definition so that "every view of one ... answers from the same place". Its asset-level twin ListAssetProperties reads through it; this model-level listing does not, so the two listings of the same composite model disagree, and DescribeAssetModelCompositeModel reports properties this listing claims the model does not have.

## Evidence

The walk that skips the resolver — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_asset_model_properties.rs:215-220:
```
        if include_composite {
            for composite in &record.composite_models {
                for property in composite.properties.as_deref().unwrap_or(&[]) {
                    summaries.push(summarize(property, Some(composite)));
                }
            }
        }
```
and the contract it is implementing, src/ops/list_asset_model_properties.rs:175-180:
```
        // `filter` defaults to BASE — "The list includes only base asset model
        // properties for a given asset model ID" — which is the model's own property
        // definitions. ALL also walks the properties each of its composite models
        // defines.
```
(model text: `smithy_query(query="doc", shape="ListAssetModelPropertiesRequest", member="filter")` — "ALL – The list includes all asset model properties for a given asset model ID.")

Why the list is empty for such a composite model — src/ops/create_asset_model_composite_model.rs:481-491: "A component-model-based composite model has no properties of its own: the component model it reuses provides them ... let properties = if composed_asset_model_id.is_some() { None".

The one resolver, and the siblings that use it:
- src/state.rs:4325-4351 — "The property definitions composite model `composite_id` of `model` contributes: its OWN definitions, or, for a component-model-based composite model, the properties of the `COMPONENT_MODEL` it reuses. ... One definition of \"the properties of a composite model\", so every view of one (the model's own, and an asset's instance of it) answers from the same place."
- src/ops/list_asset_properties.rs:218 — `for definition in store.composite_model_properties(model, composite_id) {`
- src/ops/describe_asset_model_composite_model.rs:233 — `let properties = store.composite_model_properties(record, &composite_model_id);`

Test coverage: tests/all/asset_model_properties.rs drives the BASE/ALL filters over composite models that carry their own inline definitions; no row creates one with `composedAssetModelId`, so the omission is unpinned.

## Steps to Reproduce

1. CreateAssetModel A with one property "Speed" (DOUBLE measurement).
2. CreateAssetModel B.
3. CreateAssetModelCompositeModel on B reusing A: composedAssetModelId = A, no assetModelCompositeModelProperties -> 202.
4. GET /asset-models/B/composite-models/{compositeId} -> 200, assetModelCompositeModelProperties contains "Speed".
5. GET /asset-models/B/properties?filter=ALL (ListAssetModelProperties) -> observed: only B's own base properties; "Speed" is absent from this and every later page.
6. Expected, per the member doc ("ALL – The list includes all asset model properties for a given asset model ID") and the shared resolver ListAssetProperties uses: the component model's "Speed" appears, attributed to the composite model.
