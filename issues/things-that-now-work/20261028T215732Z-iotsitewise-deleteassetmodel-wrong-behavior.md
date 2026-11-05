Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# Deleting an INTERFACE asset model silently unimplements it on every model that applied it, skipping the new version and UPDATING transition its sibling states that change owes

_Filed: 2026-10-28T21:57:32Z_
_Operation: DeleteAssetModel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteAssetModel refuses a model that is still "in use" by an asset or by another model's hierarchy, but an interface relationship is neither, so a model of type INTERFACE can be deleted while other asset models still implement it. The removal goes through `forget_asset_model`, which drops every `interface_relationships` entry the deleted model stood on either side of. Each applied-to model therefore STOPS implementing the interface — `DescribeAssetModelInterfaceRelationship` and `ListInterfaceRelationships` no longer report the pair — yet that model's own record is never touched: it keeps the same version/ETag, never reports UPDATING, and its lastUpdate instant does not move.

The crate's own sibling for exactly this state change does the opposite. `DeleteAssetModelInterfaceRelationship` removes the same map entry and then calls `record.begin_update(superseded, now)` on the applied-to model, with the comment stating this is a rule, not a choice. So one and the same observable change — "the model no longer implements the interface" — mints a new version when reached through the relationship delete and mints nothing when reached through the interface model's delete. A caller that read the applied-to model's ETag before the interface model was deleted can still use it afterwards for a conditional write, even though the model's implemented-interface set changed underneath.

## Evidence

The sibling path, which states the rule:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/delete_asset_model_interface_relationship.rs:256-264 —
    "        store.interface_relationships.remove(&key);
        // The model no longer implements the interface, so it carries a new version
        // and owes a transition back to `ACTIVE` — the rule every mutating call on
        // a record follows, kept on the record itself, and what the reported status
        // is derived from. ...
        let superseded = record.definition();
        record.begin_update(superseded, now);"

The path that skips it:
  src/ops/delete_asset_model.rs:295-301 — "A model still in use cannot go away: neither while assets instantiate it, nor while another model's hierarchy names it as the child model." (the only two dependencies checked, at :302-333)
  src/ops/delete_asset_model.rs:335 — "store.forget_asset_model(&id);"
  src/state.rs:5530-5545 — "/// Forget an asset model, its external-id entry and every interface
  /// relationship it stood on either side of.
  /// A relationship names two models, so a model that is gone leaves no
  /// relationship behind: the entries are dropped here ..."
    "        self.interface_relationships
            .retain(|(applied_to, interface), _| {
                applied_to != asset_model_id && interface != asset_model_id
            });"
  — no `asset_models.get_mut(applied_to)` / `begin_update` for the models whose relationships were just dropped.

The write the application itself performed on the applied-to model, for symmetry:
  src/ops/put_asset_model_interface_relationship.rs:492-495 — "record.properties.extend(created);
        // Applying an interface changes the model's definition, so it carries a new
        // version and reports UPDATING until the transition this call owes falls due.
        record.begin_update(superseded, now);"

Test coverage: the sibling's rule is pinned — tests/all/asset_model_interface_relationship_delete.rs:16-20 "the delete is a write to the ASSET MODEL, so that model carries a new version and owes the transition back to `ACTIVE` whose state the delete reports". Nothing exercises DeleteAssetModel against an interface model that is still applied: a grep for DeleteAssetModel over tests/all finds only asset_lifecycle.rs (assets referencing a model), hierarchies_etags_and_refusals.rs (conditional headers) and client_token_replay.rs.

## Steps to Reproduce

1. CreateAssetModel with assetModelType INTERFACE and one property -> interface model I.
2. CreateAssetModel (ASSET_MODEL) -> model M.
3. PutAssetModelInterfaceRelationship(assetModelId=M, interfaceAssetModelId=I, propertyMappingConfiguration={createMissingProperty:true}) -> 200; M reports UPDATING and a new version.
4. Wait for M to settle, then DescribeAssetModel(M) and note assetModelVersion / the returned ETag.
5. DeleteAssetModel(I) -> 202 (no refusal: I is instantiated by no asset and is no hierarchy's child model).
6. DescribeAssetModelInterfaceRelationship(M, I) -> 404: the relationship is gone, so M no longer implements I.
7. DescribeAssetModel(M) -> same assetModelVersion and ETag as step 4, state ACTIVE, and M never passed through UPDATING.
8. Expected, per delete_asset_model_interface_relationship.rs:256-264 ("The model no longer implements the interface, so it carries a new version and owes a transition back to ACTIVE — the rule every mutating call on a record follows"): M carries a new version and owes the UPDATING -> ACTIVE transition, exactly as when the relationship is deleted directly.
