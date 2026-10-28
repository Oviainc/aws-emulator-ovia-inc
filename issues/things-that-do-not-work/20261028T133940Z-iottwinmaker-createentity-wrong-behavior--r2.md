Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# A component property definition that refines the one its component type declares is refused for want of a dataType, although the crate's resolver folds the inherited dataType in and state.rs says one rule governs every writer of a definition

_Filed: 2026-10-28T13:39:40Z_
_Operation: CreateEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEntity` hand-rolls its own `dataType` rule (`require_data_types`) and refuses ANY `PropertyRequest.definition` whose `dataType` is absent, whatever the component's `componentTypeId` declares for that property. It does not call `crate::state::property_definition_needs_data_type`, the helper whose own documentation says it is "Shared by every writer of a definition so a create and an update hold one definition to one rule" and that a definition "may omit it only where it refines one it inherits".

The crate's reader for an entity component explicitly supports exactly that refinement. `Workspace::effective_property_definition` folds the component's own declaration over the declarations of the component type it instantiates, and `refined_property_definition`'s doc states that this fold exists because "a declaration may REFINE an inherited definition — restate only the members it changes, `dataType` included — rather than replace it, which is exactly why a writer accepts a definition without a `dataType` wherever one is inherited". `refined_property_definition` even has a dedicated `resolved.data_type = resolved.data_type.take().or_else(|| inherited.data_type.clone())` step for this case.

So a request that declares, on a component of type `T` which defines `Serial: STRING`, the component-local refinement `properties.Serial.definition = {"isRequiredInEntity": true}` is refused with HTTP 400 `ValidationException` "components.<name>.properties.Serial.definition.dataType is required" — even though the crate's own resolver would have reported that property with `dataType: STRING` taken from `T` and `isRequiredInEntity: true` taken from the refinement. The sibling writer one level up (`CreateComponentType`) accepts the same shape of refinement through the shared helper.

The narrower rule the helper encodes still refuses every case any existing test asserts: a property that NO declaration reaching it supplies a `dataType` for (tests/all/workspace_entity_lifecycle.rs:1380-1396 names property "p" on a component type the workspace does not hold; tests/all/entity_composite_components.rs:399-406 names property "p" on `com.example.inner`, which declares only "Pressure"). `UpdateEntity` carries a byte-equivalent copy of the same over-strict check at src/ops/update_entity.rs:624-630; the one row that touches it, tests/all/entity_update.rs:1308-1317, is introduced by the doc comment "a definition that no read could report" (entity_update.rs:1218) — a reason that does not hold for that row, because its property `Serial` IS declared with `dataType: STRING` by `SENSOR_TYPE` (entity_update.rs:107) and so the crate's resolver can report it.

## Evidence

THE RULE state.rs STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:2392-2405:
```
/// `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on the
/// `PropertyDefinitionResponse` a read reports (see
/// [`property_definition_response`]), so a definition may omit it only where it
/// refines one it inherits. ...
/// Shared by every writer of a definition so a create and an update hold one
/// definition to one rule.
pub fn property_definition_needs_data_type(
    extends_from: &[String],
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> bool {
    extends_from.is_empty() && definition.data_type.is_none()
}
```

WHY THE READER EXISTS FOR THE REFINEMENT — src/state.rs:2922-2933:
```
/// Every member of `PropertyDefinitionRequest` is optional, so a declaration
/// may REFINE an inherited definition — restate only the members it changes,
/// `dataType` included — rather than replace it, which is exactly why a writer
/// accepts a definition without a `dataType` wherever one is inherited
/// ([`property_definition_needs_data_type`]). A reader therefore folds the whole
/// set of declarations that reach a property instead of taking the nearest one
/// whole ...
/// Shared by every resolver of a definition — a component's
/// ([`Workspace::effective_property_definition`]) and a component type's own —
```
and the fold step itself, src/state.rs:2940-2943:
```
        resolved.data_type = resolved
            .data_type
            .take()
            .or_else(|| inherited.data_type.clone());
```

THE COMPONENT-LEVEL RESOLVER THAT USES IT — src/state.rs:1481-1503 (`effective_property_definition`):
```
    /// A nearer declaration may also REFINE a further one
    /// rather than replace it (every member of `PropertyDefinitionRequest` is
    /// optional, so a declaration can restate only the members it changes),
    /// which is why the whole set of declarations is folded
    /// ([`refined_property_definition`]) instead of the nearest one being taken
    /// whole.
...
        refined_property_definition(own.into_iter().chain(
            chain
                .iter()
                .filter_map(|component_type| component_type.property_definitions.get(property_name)),
        ))
```

THE WRITER THAT BREAKS THE RULE — src/ops/create_entity.rs:586-606:
```
fn require_data_types(
    member_path: &str,
    properties: Option<&::std::collections::HashMap<String, PropertyRequest>>,
) -> Result<(), CreateEntityErrorModeled> {
    for (property_name, property) in properties.into_iter().flatten() {
        if property
            .definition
            .as_ref()
            .is_some_and(|definition| definition.data_type.is_none())
        {
            return Err(CreateEntityErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "{member_path}.properties.{property_name}.definition.dataType is required"
                    )),
```
called unconditionally for every component and composite component, src/ops/create_entity.rs:331-342, under the comment "Every property a `CreateEntity` declares is a NEW property" (create_entity.rs:327-330) — which assumes away the refinement case the resolver above implements. The identical copy lives at src/ops/update_entity.rs:618-630 ("which is the rule `CreateEntity` applies to the definitions it stores").

THE SIBLING WRITER THAT APPLIES THE SHARED RULE — src/ops/create_component_type.rs:360-372:
```
        // `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on
        // the `PropertyDefinitionResponse` a read reports, so a definition may
        // omit it only where it refines one it inherits — the rule
        // `crate::state::property_definition_needs_data_type` holds for every
        // writer of a definition.
        for (property_name, definition) in &property_definitions {
            if crate::state::property_definition_needs_data_type(&extends_from, definition) {
```
(the same at src/ops/update_component_type.rs:392-400).

MODEL — `smithy_query(query="shape", shape="PropertyDefinitionRequest")`: every member is optional, `"dataType": {"target": "DataType", "doc": "<p>An object that contains information about the data type.</p>"}` — the request shape does not require it.

TEST COVERAGE: no test sends a component-level definition without `dataType` for a property the component type declares. tests/all/component_inherited_schema.rs:143-158 ("theEntitysOwnDeclarationWinsOverTheInheritedOne") restates `Serial` WITH an explicit `dataType`, so a narrower rule leaves it passing; the two refusal rows that exist name properties nothing declares a `dataType` for anywhere (tests/all/workspace_entity_lifecycle.rs:1384-1396 with componentTypeId "com.example.sensor", which that file never creates, and tests/all/entity_composite_components.rs:403-405 with property "p" against `com.example.inner`, seeded at entity_composite_components.rs:43 with only "Pressure").

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Serial": {"dataType": {"type": "STRING"}}}).
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Sensor": {"componentTypeId": "com.example.sensor", "properties": {"Serial": {"definition": {"isRequiredInEntity": true}}}}}).
4. Observe HTTP 400 ValidationException "components.Sensor.properties.Serial.definition.dataType is required" and GetEntity(workspaceId="ws", entityId="e1") -> 404: nothing was created.
5. Compare the sibling writer for the same shape of refinement one level up: CreateComponentType(workspaceId="ws", componentTypeId="com.example.child", extendsFrom=["com.example.sensor"], propertyDefinitions={"Serial": {"isRequiredInEntity": true}}) returns 200, and GetComponentType reports Serial with dataType STRING folded in from the parent (src/ops/get_component_type.rs:391-412).
6. Compare what the crate's reader would have reported for the refused request: create the entity with the explicit dataType instead, and GetEntity resolves the component's definition through `effective_property_definition` (src/ops/get_entity.rs:415-418), which folds the component type's members into the component's own declaration — the very path a dataType-less refinement would have travelled.
