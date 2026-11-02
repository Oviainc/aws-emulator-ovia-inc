Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: bdbe8baa9177a8db6293f043e773851dfd709fdf

# ExecuteQuery projects only entity-LOCAL property records, so an inherited property is absent from a node's components and an inherited relationship is no edge

_Filed: 2026-10-31T11:56:01Z_
_Operation: ExecuteQuery | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `bdbe8baa9177a8db6293f043e773851dfd709fdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ExecuteQuery is the one property reader in this crate that does not go through the two shared projections the crate states every reader shares. Both the graph's EDGE derivation and the components[].properties array of a NODE row are built by walking component.properties() -- the property records the ENTITY stores locally -- and, for the edges, by reading PropertyRecord::value directly. Two consequences of the one omission: (1) a property the component only INHERITS from the component type it instantiates is not reported at all, and the value it reads as (the definition's defaultValue) is never reported, although GetEntity and ListProperties enumerate the component's EFFECTIVE property set through Workspace::component_property_names and resolve each value through latest_property_value, which falls back to the defaultValue declared only on the component TYPE -- so a component's property Tag has value 'unset-tag' for GetEntity/ListProperties/GetPropertyValue and does not exist at all for a query over the same stored state, and a relationship property whose value comes from the type's defaultValue is no edge, so -[:Link]-> selects nothing; (2) the edge loop reads property.value -- the member whose own doc says it is only the one current value the CONTROL plane set -- where components_document, 150 lines down the same file, reads property.latest_written_value(), so a relationship whose latest write came through the time-series data plane is no edge and one the data plane has superseded is still an edge built from the stale control-plane value.

## Evidence

THE HANDLER'S OWN CLAIM -- src/ops/execute_query.rs:36-38: '//!   * a property of ANY of the entity's components, at any path, whose value carries a `relationshipValue` naming a target entity of the same workspace, labelled with the property's name.'

WHAT THE EDGE LOOP READS -- src/ops/execute_query.rs:1484-1492: 'for component in entity.components_at_every_path() { for (property_name, property) in component.properties() { let Some(relationship) = property .value .as_ref() .and_then(|value| value.relationship_value.as_ref()) else { continue; };' -- entity-local records only, and the raw `value` member.

WHAT THE NODE'S PROPERTY ARRAY READS -- src/ops/execute_query.rs:1636-1654: 'members.insert("properties".to_string(), Document::Array(component .properties() .iter() .map(|(property_name, property)| { ... // The most recent value written to the property through either plane, so a time series property is queried on the value its series last carries rather than on nothing. if let Some(value) = property.latest_written_value() {' -- same file, two different readings of one property; and neither path enumerates inherited names nor applies the defaultValue fallback.

THE CRATE'S OWN RULES -- src/state.rs:1740-1759: '/// Naming a `componentTypeId` is what gives a component that type's properties, so a component commonly stores only the VALUE of a property whose definition lives on its type, and a property whose definition it inherits is as much a property of the component as one it declares inline. ... Shared by every reader that has to enumerate a component's properties rather than resolve one of them' above 'pub fn component_property_names('.
src/state.rs:3307-3334: '/// The value a read reports for ONE property: the MOST RECENT value written to the property, else the DEFAULT its definition declares, and nothing at all when neither exists. ... Shared by every reader of a property's current value, so one property reads the same way wherever it is reported; `definition` is the one [`Workspace::effective_property_definition`] resolved, which is what makes a default declared only on the component TYPE apply to a component that restates nothing.' above 'pub fn latest_property_value('.
src/state.rs:4196-4202: '/// The one current value the CONTROL plane set on this property, `None` where it has set none or where its last write RESET the value away.' above 'pub value: Option<crate::ops::types::DataValue>,'.

THE SIBLING READERS THAT DO USE BOTH -- src/ops/get_entity.rs:307-312: '// Every property the component HAS -- the ones it declared inline and the ones the component type it instantiates gives it -- in name order ... let property_names = workspace.component_property_names(component.component_type_id.as_deref(), &component.properties);' and src/ops/list_properties.rs:262-266 plus 407-410: 'let value = crate::state::latest_property_value(component.properties.get(&property_name), definition.as_ref());'.

TEST COVERAGE: the 'one property reads the same way wherever it is reported' rule is pinned for four readers and ExecuteQuery is not among them -- tests/all/property_value_write_order.rs:3-15 exercises GetPropertyValue, ListProperties, GetEntity and GetPropertyValueHistory only. The one ExecuteQuery assertion touching properties, tests/all/execute_query.rs:639-661, runs on a fixture whose component types are never created (tests/all/execute_query.rs:137-163 declares every property INLINE and calls no CreateComponentType), so local and effective sets coincide there and that assertion stays unchanged; tests/all/execute_query_composite_components.rs:519-549 compares only which COMPONENTS a node reports against GetEntity, not their properties.

## Steps to Reproduce

1. CreateWorkspace(workspaceId=ws).
2. CreateComponentType(workspaceId=ws, componentTypeId=com.example.sensor, propertyDefinitions={Tag: {dataType: {type: STRING}, defaultValue: {stringValue: unset-tag}}, Link: {dataType: {type: RELATIONSHIP, relationship: {relationshipType: Link}}, defaultValue: {relationshipValue: {targetEntityId: e2}}}}).
3. CreateEntity(workspaceId=ws, entityId=e2, entityName=E2).
4. CreateEntity(workspaceId=ws, entityId=e1, entityName=E1, components={Sensor: {componentTypeId: com.example.sensor}}) -- the component restates nothing.
5. GetEntity(workspaceId=ws, entityId=e1) -> 200 reporting components.Sensor.properties.Tag with value.stringValue 'unset-tag' and components.Sensor.properties.Link with the relationship value (src/ops/get_entity.rs:311 + src/state.rs:3328); ListProperties(workspaceId=ws, entityId=e1, componentName=Sensor) reports both names.
6. ExecuteQuery(workspaceId=ws, queryStatement=SELECT e FROM EntityGraph MATCH (e) WHERE e.entityId = 'e1') -> 200, but the NODE column's components[0].properties is an EMPTY array: neither Tag nor Link is reported and no propertyValue is reported for either.
7. ExecuteQuery(workspaceId=ws, queryStatement=SELECT e FROM EntityGraph MATCH (e)-[:Link]->(f)) -> 200 with ZERO rows, although e1's Sensor component's Link property reads as a relationship to e2 through every other reader.
8. Second symptom: give the component type a dataWriter function and a property with dataType RELATIONSHIP and isTimeSeries true, then BatchPutPropertyValues a relationshipValue for an instant. GetEntity/GetPropertyValue report that relationship (PropertyRecord::latest_written_value, src/state.rs:4275-4283) while Graph::build reads PropertyRecord::value -- still None -- so a hop over that relationship selects nothing.
