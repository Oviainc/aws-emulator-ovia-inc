Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# A component's property group is resolved by taking the nearest declaration WHOLE, so a restated group loses the inherited propertyNames the crate's own rule says it keeps

_Filed: 2026-10-28T11:54:15Z_
_Operation: GetEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::Workspace::effective_property_group` is the single resolver every reader of an entity component's property groups goes through (`GetEntity`, `ListComponents`, and `GetPropertyValue`'s `propertyGroupName` resolution). Its own doc comment states that it makes "the same resolution `effective_property_definition` makes for a definition" — and `effective_property_definition`'s doc states explicitly that a nearer declaration REFINES a further one, "which is why the whole set of declarations is folded ... instead of the nearest one being taken whole".

The body of `effective_property_group` does exactly the thing that doc says it does not: it returns the first declaration it finds (the component's own record, else the first component type on the `extendsFrom` chain that declares the group) WHOLE, and never folds the rest in.

The divergence is observable because `propertyNames` is optional on `ComponentPropertyGroupRequest`/`PropertyGroupRequest` and both writers store an absent list AS THE EMPTY ONE (`PropertyGroupRecord::from_request`, `create_entity::property_group_records`). So an entity component that restates an inherited group naming only `groupType` ends up holding a record with `property_names: []`, and:

- `GetEntity` / `ListComponents` report that group with `propertyNames: []`, dropping the names the component type declared for it, where the sibling reader for a component TYPE (`GetComponentType`) folds and keeps them — its code carries the rule in a comment: "the record stores an absent list as the empty one, so a declaration with no names sets nothing and leaves the inherited group's names standing";
- `GetPropertyValue` resolves `propertyGroupName` through the same helper, so the group resolves to NO property names and every `selectedProperties` entry is refused with "selectedProperties <p> is not in propertyGroup <g>" — a 400 for a request whose properties really are in that group.

The same whole-declaration shortcut appears in `get_property_value::property_group_names` for the component-TYPE branch (`find_map` over `component_type_chain`), which disagrees with `GetComponentType`'s fold over the same chain for the same reason.

Fixing the resolver to fold (groupType from the nearest declaration that sets it, propertyNames from the nearest declaration that carries any — exactly what `get_component_type::property_group_response` already does) changes no existing assertion: every test that restates a component's group supplies BOTH `groupType` and `propertyNames`.

## Evidence

THE RULE THE RESOLVER CLAIMS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:1521-1525:
  "/// The property group `group_name` as it applies to a component: the group
   /// the component itself declares, else the one it inherits from the
   /// component type it instantiates — the same resolution
   /// [`effective_property_definition`](Self::effective_property_definition)
   /// makes for a definition."

WHAT THAT RESOLUTION ACTUALLY IS — src/state.rs:1481-1486 (`effective_property_definition`'s doc):
  "A nearer declaration may also REFINE a further one
   /// rather than replace it (every member of `PropertyDefinitionRequest` is
   /// optional, so a declaration can restate only the members it changes),
   /// which is why the whole set of declarations is folded
   /// ([`refined_property_definition`]) instead of the nearest one being taken
   /// whole."
and src/state.rs:1499-1503 calls `refined_property_definition(own.into_iter().chain(chain...))`.

WHAT THE CODE UNDER THE CLAIM DOES — src/state.rs:1526-1538:
  "pub fn effective_property_group<'a>(...) -> Option<&'a PropertyGroupRecord> {
       if let Some(group) = property_groups.get(group_name) {
           return Some(group);
       }
       self.component_type_chain(component_type_id?)
           .into_iter()
           .find_map(|component_type| component_type.property_groups.get(group_name))
   }"
No fold: the first record found is returned whole.

THE PROJECTION THAT THEN REPORTS IT VERBATIM — src/state.rs:2318-2323 (`component_property_group_response`):
  "group_type: group
       .group_type
       .clone()
       .unwrap_or_else(|| TABULAR_GROUP_TYPE.to_string()),
   property_names: group.property_names.clone(),"

WHY AN EMPTY LIST IS REACHABLE — src/state.rs:3561-3571 (`PropertyGroupRecord::from_request`):
  "/// `propertyNames` is stored RESOLVED, so an absent list is an
   /// empty one rather than a missing collection every reader has to guard
   /// against."
  "property_names: request.property_names.unwrap_or_default(),"
and src/ops/create_entity.rs:552-555: "property_names: group.property_names.unwrap_or_default(),".
The model makes the member optional — smithy_query(query="shape",shape="ComponentPropertyGroupRequest"): "propertyNames": {"target": "PropertyNames", "uniqueItems": true, "type": "list"} (no `required`), while `ComponentPropertyGroupResponse.propertyNames` IS "required": true.

THE SIBLING READER THAT APPLIES THE FOLD — src/ops/get_component_type.rs:514-519 and 524-541:
  "/// The same fold the other parts of the schema get: `groupType` is the one the
   /// nearest declaration that carries it names, and `propertyNames` the list of
   /// the nearest declaration that carries any — the record stores an absent list
   /// as the empty one, so a declaration with no names sets nothing and leaves the
   /// inherited group's names standing."
  "if property_names.is_none() && !declaration.property_names.is_empty() {
       property_names = Some(declaration.property_names.clone());
   }"

THE CALL SITES OF THE UNFOLDED RESOLVER:
- src/state.rs:2344-2353 (`component_property_groups_response`, "The ONE projection every reader of a component's property groups shares") -> src/ops/get_entity.rs:330-334 and src/ops/list_components.rs:465-471.
- src/ops/get_property_value.rs:650-656 (`property_group_names`), whose component-TYPE branch at src/ops/get_property_value.rs:634-638 likewise uses ".find_map(|component_type| component_type.property_groups.get(group_name))" instead of the fold `GetComponentType` performs over the same chain.

TEST COVERAGE: no test exercises a restated group that omits `propertyNames`. Every component-level restatement supplies both members — tests/all/component_inherited_schema.rs:151-153 ("Readings": {"groupType": "TABULAR", "propertyNames": ["Serial"]}), tests/all/component_inherited_schema.rs:165-167, tests/all/component_listing.rs:82-83, tests/all/entity_composite_components.rs:207-208, tests/all/property_value_read.rs:63-64 — and tests/all/entity_update.rs:493-509 ("propertyGroup.patched") sends an EXPLICIT `"propertyNames": []` on a group the component declares itself for a component type that declares no group of that name, so it is unaffected by folding.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.gauge", propertyDefinitions={"Serial": {"dataType": {"type": "STRING"}}, "Tag": {"dataType": {"type": "STRING"}}}, propertyGroups={"Readings": {"groupType": "TABULAR", "propertyNames": ["Serial", "Tag"]}}).
3. CreateEntity(workspaceId="ws", entityId="g1", entityName="G1", components={"Gauge": {"componentTypeId": "com.example.gauge", "propertyGroups": {"Readings": {"groupType": "TABULAR"}}}}) — the group is restated with no `propertyNames`, which the model allows.
4. GetEntity(workspaceId="ws", entityId="g1"): components.Gauge.propertyGroups.Readings.propertyNames is [] — the two names the component type declared for that group are gone. ListComponents(workspaceId="ws", entityId="g1") reports the same empty list.
5. Compare the sibling reader for the same kind of restatement one level up: UpdateComponentType on a child type that restates an inherited group with no `propertyNames`, then GetComponentType — propertyGroups.<g>.propertyNames still carries the ancestor's names (get_component_type.rs:534-536).
6. GetPropertyValue(workspaceId="ws", entityId="g1", componentName="Gauge", propertyGroupName="Readings", selectedProperties=["Serial"]) answers HTTP 400 ValidationException "selectedProperties Serial is not in propertyGroup Readings", although Serial is in that group as the component type declares it.
