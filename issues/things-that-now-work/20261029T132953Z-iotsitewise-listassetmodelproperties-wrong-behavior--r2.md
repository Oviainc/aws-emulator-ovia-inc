Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9644ddf994ccf47d3ab59a1dffbb24b4a89d5fe4

# Every property summary reports interfaceSummaries as absent, although the store holds the exact (interface asset model id, interface property id) pair the member is documented to carry

_Filed: 2026-10-29T13:29:53Z_
_Operation: ListAssetModelProperties | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9644ddf994ccf47d3ab59a1dffbb24b4a89d5fe4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAssetModelProperties` builds each `AssetModelPropertySummary` in its `summarize` helper and hard-codes `interface_summaries: None`, on the stated premise that "Which interfaces a property belongs to is reported by no member this enumeration derives" and that the per-property coverage of a mapping "is the subject of the interface reads, not of this list". The modelled shape says the opposite: `AssetModelPropertySummary` is documented as "Contains a summary of a property associated with a model. This includes information about which interfaces the property belongs to, if any", and its `interfaceSummaries` member as "A list of interface summaries that describe which interfaces this property belongs to, including the interface asset model ID and the corresponding property ID in the interface."

That is precisely what this crate already stores: `InterfaceRelationshipRecord` holds `asset_model_id`, `interface_asset_model_id` and `property_mappings`, each mapping pairing one property of the applied-to model with the interface property it answers for — and the generated `InterfaceSummary` type carries exactly those two ids. So for any property of a model that implements an interface, the summary this listing must report is a lookup over `store.interface_relationships`, the same record `DescribeAssetModelInterfaceRelationship` already reports verbatim. As written, a property mapped by an applied interface is listed as belonging to no interface, and a client has no way to learn from this listing which of a model's properties an interface covers.

(Distinct from the already-fixed composite-model-properties gap on this operation: that one was about WHICH rows the walk emits; this is a member of every row that is never computed.)

## Evidence

The handler's summary projection — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_asset_model_properties.rs:302-320:
```
    AssetModelPropertySummary {
        id: property.id.clone(),
...
        path: Some(path),
        // Which interfaces a property belongs to is reported by no member this
        // enumeration derives: the store records an applied interface as a
        // relationship between two asset models, and which of a model's
        // properties each mapping covers is the subject of the interface reads,
        // not of this list. Left ABSENT rather than reported as an empty list:
        // the emulator has nothing to say here, which is not the same claim as
        // "this property belongs to no interface".
        interface_summaries: None,
    }
```

The modelled contract (smithy_query query="shape" shape="AssetModelPropertySummary"):
* shape doc: "Contains a summary of a property associated with a model. This includes information about which interfaces the property belongs to, if any."
* member `interfaceSummaries`: "A list of interface summaries that describe which interfaces this property belongs to, including the interface asset model ID and the corresponding property ID in the interface."

The stored record that holds exactly that pairing — src/state.rs:1318-1330:
```
pub struct InterfaceRelationshipRecord {
    /// The stored `assetModelId` of the model the interface is applied to.
    pub asset_model_id: String,
    /// The stored `assetModelId` of the interface asset model.
    pub interface_asset_model_id: String,
    /// Each property of the interface asset model, paired with the property of
    /// [`Self::asset_model_id`] it maps to.
    pub property_mappings: Vec<PropertyMapping>,
```
and the target shape this listing would fill, src/ops/types/list_asset_model_properties_types.rs:46-51:
```
#[smithy_macros::smithy(shape = "com.amazonaws.iotsitewise#InterfaceSummary")]
pub struct InterfaceSummary {
    pub interface_asset_model_id: String,
    pub interface_asset_model_property_id: String,
}
```
The sibling read proving the mappings are reachable and reported elsewhere — src/ops/describe_asset_model_interface_relationship.rs:191-194 (`property_mappings: record.property_mappings.clone(),`).

Test coverage: tests/all/asset_model_interface_relationship.rs:258 `applying_an_interface_resolves_one_mapping_per_interface_property` applies an interface and asserts `read["propertyMappings"]` (line 361) on DescribeAssetModelInterfaceRelationship, so the per-property pairing is pinned as stored. A repo-wide grep for `interfaceSummaries` in tests/ matches nothing, so no test pins the current absence.

## Steps to Reproduce

1. CreateAssetModel A with a property named `Temperature`; CreateAssetModel I with assetModelType=INTERFACE and a property named `Temperature`.
2. PUT the interface relationship (PutAssetModelInterfaceRelationship) applying I to A with `matchByPropertyName=true`.
3. GET /asset-models/{A}/interface-relationships/{I} (DescribeAssetModelInterfaceRelationship): 200, `propertyMappings` pairs A's `Temperature` property id with I's.
4. GET /asset-models/{A}/properties?filter=ALL (ListAssetModelProperties).
5. Observed: 200, and the summary for A's `Temperature` property carries no `interfaceSummaries` at all.
6. Expected, per the member's own modelled sentence ("A list of interface summaries that describe which interfaces this property belongs to, including the interface asset model ID and the corresponding property ID in the interface"): the summary carries one `InterfaceSummary` naming I and I's `Temperature` property id, read from the `InterfaceRelationshipRecord.property_mappings` the apply already stored.
