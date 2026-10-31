Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# ExecuteQuery's EntityGraph ignores an entity's composite components: their componentTypeId is no node label, their relationshipValue is no edge, and e.components omits them

_Filed: 2026-10-28T17:34:58Z_
_Operation: ExecuteQuery | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Graph::build` derives an entity's node labels, its relationship edges and its `components` row document from `entity.components` ONLY — the entity's top-level components. The composite components stored inside them (`ComponentRecord::composite_components`) are never walked.

Three consequences, all from the same omission:

1. `(e:com.example.type)` does not select an entity whose only instance of `com.example.type` is a COMPOSITE component. `labels` is built from `component.component_type_id` of top-level components alone (execute_query.rs:1428-1437), so `has_label` answers false for a composite component's type.
2. `-[:someRelationship]->` does not traverse a relationship held on a composite component's property: the edge loop iterates `entity.components.values()` and then only that component's own `properties` (execute_query.rs:1459-1460), so a `relationshipValue` stored on a composite component produces no edge and a hop over it selects nothing.
3. `e.components` reports only the top-level components (`components_document`, execute_query.rs:1588-1592), so `WHERE e.components...` can never read a composite component.

This contradicts the rule the rest of this crate states and applies: a composite component IS a component of the entity, carries its own `componentTypeId` and its own properties, and every other component-type-scoped reader of this crate walks composite components for exactly that stated reason — `GetPropertyValue`'s cross-entity scope, `GetPropertyValueHistory`'s scope, `DeleteComponentType`'s reference scan, and `ListEntities`' `externalId` filter. ExecuteQuery is the one reader that does not, so one entity is "carrying a component of that type" for `GetPropertyValue` and not for the knowledge-graph query over the same stored state.

## Evidence

THE HANDLER'S OWN CLAIM — src/ops/execute_query.rs:26-35:
```
//! * a NODE is a stored entity of the workspace, labelled with the `componentTypeId` of
//!   each of its components, so `(e:com.example.type)` selects the entities carrying a
//!   component of that type;
...
//!   * a component property whose value carries a `relationshipValue` naming a target
//!     entity of the same workspace, labelled with the property's name.
```

WHAT THE CODE ACTUALLY WALKS — src/ops/execute_query.rs:1410-1411 and 1428-1437:
```
    /// Each entity's node labels: the `componentTypeId` of each of its components.
    labels: Vec<BTreeSet<&'a str>>,
...
        let labels: Vec<BTreeSet<&str>> = entities
            .iter()
            .map(|entity| {
                entity
                    .components
                    .values()
                    .filter_map(|component| component.component_type_id.as_deref())
                    .collect()
            })
            .collect();
```
edges, src/ops/execute_query.rs:1459-1461:
```
            for component in entity.components.values() {
                for (property_name, property) in &component.properties {
                    let Some(relationship) = property
```
the node document, src/ops/execute_query.rs:1587-1592:
```
/// The entity's stored components, each with the properties stored on it.
fn components_document(entity: &crate::state::Entity) -> Document {
    Document::Array(
        entity
            .components
            .values()
```
None of the three reaches `component.composite_components`.

THE CRATE'S OWN RULE, STATED TWICE — src/state.rs:1213-1220 (`Workspace::component_type_references`):
```
    /// * an entity's COMPOSITE component, through
    ///   [`CompositeComponentRecord::component_type_id`] — a composite
    ///   component is a component of the entity too (`GetEntity` reports it as
    ///   one) and is an instance of the type the declaration its path resolved
    ///   to named, so the type's definitions are equally what give its
    ///   properties their meaning, whatever depth it sits at ...
```
and src/ops/list_entities.rs:312-317 (`carries_external_id`):
```
/// Whether `component`, or any composite component inside it, carries
/// `external_id` as the value of its external-id property.
///
/// A composite component is a component of the entity too — `GetEntity` reports
/// it as one — and it carries a `componentTypeId` and properties of its own, so
/// the external id it names is resolved the same way the holding component's is
```

THE SIBLING READERS THAT DO WALK THEM — src/ops/get_property_value.rs:512-516:
```
            for composite in component.composite_components.values() {
                if instance_type_ids.contains(composite.component_type_id.as_str()) {
                    components.push(scoped_composite_component(&entity.entity_id, composite));
                }
            }
```
and src/ops/get_property_value_history.rs:728-732:
```
            for composite in component.composite_components.values() {
                if instance_type_ids.contains(composite.component_type_id.as_str()) {
                    components.push(scoped_composite(&entity.entity_id, composite));
                }
            }
```

TEST COVERAGE: no ExecuteQuery test exercises a composite component — `grep -n 'composite' tests/all/execute_query.rs tests/all/execute_query_hierarchy.rs` returns no match. The label cases are built from top-level components only (tests/all/execute_query.rs:29 "One fixture entity's component: its name, its type (which is the node label it gives", :159 `"componentTypeId": component.type_id`, :336 `("a node label", &by_label, ids(&["mixer_0", "mixer_1"]))`), so widening the graph leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.leaf", propertyDefinitions={"Target": {"dataType": {"type": "RELATIONSHIP"}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.rig", compositeComponentTypes={"inner": {"componentTypeId": "com.example.leaf"}}).
4. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Rig": {"componentTypeId": "com.example.rig"}}, compositeComponents={"Rig/inner": {}}).
5. GetEntity(workspaceId="ws", entityId="e1") -> 200 reporting components.Rig.compositeComponents.inner with componentTypeId "com.example.leaf" (src/ops/get_entity.rs:377-395).
6. ExecuteQuery(workspaceId="ws", queryStatement="SELECT e FROM EntityGraph MATCH (e:com.example.leaf)") -> 200 with ZERO rows, although e1 holds an instance of com.example.leaf.
7. Compare the sibling reader over the same state: GetPropertyValue(workspaceId="ws", componentTypeId="com.example.leaf", selectedProperties=["Target"], tabularConditions={}) builds a row for that composite component (src/ops/get_property_value.rs:512-516).
8. Now store a relationship on the composite component: UpdateEntity(workspaceId="ws", entityId="e1", compositeComponentUpdates={"Rig/inner": {"updateType": "UPDATE", "propertyUpdates": {"Target": {"value": {"relationshipValue": {"targetEntityId": "e1"}}}}}}).
9. ExecuteQuery(workspaceId="ws", queryStatement="SELECT e FROM EntityGraph MATCH (e)-[:Target]->(f)") -> 200 with ZERO rows: the composite component's relationship is no edge.
