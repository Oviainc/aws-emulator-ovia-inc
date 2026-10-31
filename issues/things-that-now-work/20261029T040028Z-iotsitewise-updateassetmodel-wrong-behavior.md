Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# UpdateAssetModel drops properties an applied interface maps to, leaving interface relationships whose propertyMappings name properties the model no longer declares

_Filed: 2026-10-29T04:00:28Z_
_Operation: UpdateAssetModel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateAssetModel replaces the model's whole property list with the one the request states (`record.properties = properties;`), and `propagate_asset_model_definitions` then drops the matching asset properties. What it never touches is `store.interface_relationships`: the `PropertyMapping` entries recorded by PutAssetModelInterfaceRelationship keep naming the `assetModelPropertyId` of properties the update just removed.

The same handler, ten lines earlier, prunes exactly this kind of dangling reference for composite-model links, with a comment stating the rule — "a link pointing at a composite model this asset model no longer holds would name nothing". Interface property mappings get no such treatment, so after an update DescribeAssetModelInterfaceRelationship (and the summaries of ListInterfaceRelationships) report a relationship whose propertyMappings point at property ids that resolve to nothing: DescribeAssetProperty answers 404 for them and they appear in no ListAssetModelProperties page.

That stored mappings can go stale this way is not hypothetical to the crate: PutAssetModelInterfaceRelationship explicitly filters a previous mapping by "while the model still declares that property" before reusing it, which is only needed because some other write can remove a mapped property. The crate's state module states the opposite rule for the removal case — a deleted model "leaves no relationship behind" — so a removed PROPERTY leaving a mapping behind is the same inconsistency one level down.

## Evidence

The dangling reference this handler DOES prune, src/ops/update_asset_model.rs:496-528:
```
        // The links a composite model carries beside its own definition ... are kept
        // for every composite model the new definition still holds and dropped with
        // the ones it does not. A kept composite model whose PARENT is gone becomes
        // top-level: a link pointing at a composite model this asset model no longer
        // holds would name nothing.
        let composite_model_links: ... = record
            .composite_model_links
            .iter()
            .filter(|(composite_id, _)| composite_models.iter().any(|kept| kept.id.as_deref() == Some(composite_id.as_str())))
```

The write that drops the properties, with no equivalent for the relationships — src/ops/update_asset_model.rs:543-546:
```
        record.properties = properties;
        record.hierarchies = hierarchies;
        record.composite_models = composite_models;
        record.composite_model_links = composite_model_links;
```
(grep for `interface_relationship` over src/ops shows no occurrence in update_asset_model.rs; the only writers are put_asset_model_interface_relationship.rs:503 and delete_asset_model_interface_relationship.rs:256.)

The crate's own acknowledgement that a mapping can name a property the model no longer declares — src/ops/put_asset_model_interface_relationship.rs:349-365:
```
            // What the previous application of THIS relationship mapped the
            // property to, while the model still declares that property.
            let mapped_before = applied_before
                ...
                .filter(|id| {
                    applied_to
                        .properties
                        .iter()
                        .any(|held| held.id.as_deref() == Some(id.as_str()))
                });
```

What the read then serves, unfiltered — src/ops/describe_asset_model_interface_relationship.rs:191-196: `property_mappings: record.property_mappings.clone(),`.

The removal rule the state module states for the model-level case — src/state.rs:5497-5513: "A relationship names two models, so a model that is gone leaves no relationship behind: the entries are dropped here rather than left to consume the [`MAX_INTERFACE_RELATIONSHIPS`] budget for a pair nothing can address any more."

Test coverage: tests/all/asset_model_interface_relationship.rs pins the mappings a fresh and a re-application report (lines 581-610 and 632-648) but never updates the applied-to model's property list afterwards; no test in tests/all reads a relationship after an UpdateAssetModel that removes a mapped property.

## Steps to Reproduce

1. CreateAssetModel with assetModelType INTERFACE and one property P_i -> interface model I.
2. CreateAssetModel (ASSET_MODEL) with one property named the same as P_i -> model M (property P_m).
3. PutAssetModelInterfaceRelationship(assetModelId=M, interfaceAssetModelId=I, propertyMappingConfiguration={"matchByPropertyName": true}) -> 200. DescribeAssetModelInterfaceRelationship(M, I) reports propertyMappings [{interfaceAssetModelPropertyId: P_i, assetModelPropertyId: P_m}].
4. UpdateAssetModel(M) with assetModelProperties OMITTING P_m (e.g. a single, differently named property) -> 202. Per src/ops/update_asset_model.rs:543 the stored property list no longer holds P_m, and propagate_asset_model_definitions drops it from every asset of M.
5. DescribeAssetModelInterfaceRelationship(M, I) -> 200, still reporting assetModelPropertyId: P_m.
6. DescribeAssetProperty(assetModelId=M, propertyId=P_m) / ListAssetModelProperties(M) -> P_m resolves to nothing, so the reported mapping names a property that does not exist.
7. Expected, per the rule this same handler applies to composite-model links at src/ops/update_asset_model.rs:499-502 ("a link pointing at a composite model this asset model no longer holds would name nothing") and per src/state.rs:5497-5513: the update either re-resolves/drops the mappings that name removed properties, or is refused while an applied interface maps to them — not a relationship reporting a mapping to a property the model no longer declares.
