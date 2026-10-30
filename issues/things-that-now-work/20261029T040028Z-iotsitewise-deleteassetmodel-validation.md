Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# A COMPONENT_MODEL still reused by another model's composite model is deleted, silently emptying that composite model's property list

_Filed: 2026-10-29T04:00:28Z_
_Operation: DeleteAssetModel | Class: validation_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteAssetModel states its in-use rule as a rule and then enforces only two of the three in-use directions this crate itself records. The handler refuses a model while an asset instantiates it and while another model's hierarchy names it as the child model, but it never checks the third dependency the store holds: a component-model-based composite model of ANOTHER asset model whose `CompositeModelLink.composed_asset_model_id` names this model (what `CreateAssetModelCompositeModel` records and what `ListCompositionRelationships` exists to enumerate).

So a model of type COMPONENT_MODEL can be deleted while another asset model's composite model reuses it. The link that named it survives in the holder's record (nothing in `forget_asset_model` touches `composite_model_links`), and because a component-model-based composite model deliberately stores NO properties of its own — they are read through from the component model on every read — the holder's composite model silently loses every property it had: `composite_model_properties` falls through to `.unwrap_or_default()` and answers an EMPTY list. DescribeAssetModelCompositeModel then reports that composite model with `assetModelCompositeModelProperties: []` while still reporting `compositionDetails.compositionRelationship` naming the asset model that no longer exists, and DescribeAssetCompositeModel (which reads the same helper) does the same for every asset instance of it. No refusal, no version bump, no trace that anything was lost.

This is the dependency direction the handler's own comment already claims to cover, and it is the same shape of defect the crate refuses elsewhere (DeleteTimeSeries refuses a series that still has data segments; DeleteAssetModel itself refuses a model another hierarchy names). It is distinct from the already-filed DeleteAssetModel/wrong-behavior issue, which is about an INTERFACE model's relationships and the missing version transition on the applied-to model.

## Evidence

The rule the handler states, and the only two dependencies it checks — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_asset_model.rs:295-316:
```
        // A model still in use cannot go away: neither while assets instantiate it,
        // nor while another model's hierarchy names it as the child model. Either
        // way this is a TERMINAL request error ...
        let referencing_asset = store
            .assets
            .values()
            .any(|asset| asset.asset_model_id == id);
        let referencing_model = store
            .asset_models
            .values()
            .find(|model| {
                model.id != id
                    && model
                        .hierarchies
                        .iter()
                        .any(|hierarchy| hierarchy.child_asset_model_id == id)
            })
```
and then src/ops/delete_asset_model.rs:335 — `store.forget_asset_model(&id);` (whose doc, src/state.rs:5497-5513, drops only the external-id entry and the interface relationships: nothing about `composite_model_links`).

The third dependency the store records — src/ops/create_asset_model_composite_model.rs:341-364: "A component-model-based composite model reuses an existing asset model, and only one of type COMPONENT_MODEL can be reused." … `Some(composed)`, stored at src/ops/create_asset_model_composite_model.rs:508-513 as `crate::state::CompositeModelLink { parent_id, composed_asset_model_id }`.

Why the loss is silent — src/state.rs:4330-4354:
```
    /// A component-model-based composite model stores no properties of its own —
    /// the component model it names provides them, and they are read from that
    /// model's stored record rather than copied at create time ...
    pub fn composite_model_properties(&self, model: &AssetModelRecord, composite_id: &str) -> Vec<AssetModelProperty> {
        match model.composed_asset_model_id(composite_id) {
            Some(composed) => self
                .asset_models
                .get(composed)
                .map(|component| component.properties.clone())
                .unwrap_or_default(),
```

What the holder then reports — src/ops/describe_asset_model_composite_model.rs:232-240: `let composed = record.composed_asset_model_id(&composite_model_id); let properties = store.composite_model_properties(record, &composite_model_id);` … `composition_details = composed.map(|composed| CompositionDetails { composition_relationship: Some(vec![CompositionRelationshipItem { id: Some(composed.to_string()) }]) })` — i.e. an empty property list beside a relationship naming a model the store no longer holds.

That the dependents are enumerable already — src/ops/list_composition_relationships.rs:199: `if holder.composed_asset_model_id(composite_id) != Some(asset_model_id.as_str()) {` (the listing of "which asset models' composite models reuse this component model").

Test coverage: component-model reuse is exercised (tests/all/composition_relationships.rs:139,163,188,202 and tests/all/asset_model_composite_model.rs:567), but no test deletes a reused component model — a grep for the two refusal messages (`had assets when delete`, `referenced by a hierarchy`) over tests/all finds nothing, and the DeleteAssetModel tests cover only assets, conditional headers and clientToken replay.

## Steps to Reproduce

1. CreateAssetModel with assetModelType COMPONENT_MODEL and one property -> component model C.
2. CreateAssetModel (ASSET_MODEL) -> model M.
3. CreateAssetModelCompositeModel(assetModelId=M, composedAssetModelId=C, assetModelCompositeModelType=<the component type>) -> 202; the composite model X is created on M.
4. DescribeAssetModelCompositeModel(M, X) -> 200 with assetModelCompositeModelProperties carrying C's property (read through src/state.rs:4343-4348).
5. DeleteAssetModel(C) -> 202. Observed: accepted, because the handler checks only assets and hierarchy child models (src/ops/delete_asset_model.rs:302-316).
6. DescribeAssetModelCompositeModel(M, X) -> 200 with assetModelCompositeModelProperties: [] and compositionDetails.compositionRelationship: [{id: C}] — a relationship naming a model the store no longer holds, and every property of that composite model gone. DescribeAssetCompositeModel on any asset of M answers the same empty list.
7. Expected, per the handler's own rule at src/ops/delete_asset_model.rs:295-297 ("A model still in use cannot go away") and the way the two checked directions are refused: InvalidRequestException (400) naming the asset model whose composite model reuses C, so the caller deletes the composite model first.
