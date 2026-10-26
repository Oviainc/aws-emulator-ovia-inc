Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# GetEntity does not resolve a component's componentTypeId, so inherited property definitions, definedIn and isInherited are never reported

_Filed: 2026-10-24T00:03:42Z_
_Operation: GetEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For an entity whose component names a `componentTypeId` that exists in the workspace, real IoT TwinMaker resolves the component type and reports the component's FULL property schema: every property the component type defines appears in `components.<name>.properties` with `definition.isInherited = true` (and `definition.dataType`/`isTimeSeries`/`isRequiredInEntity`/`displayName` taken from the component type), and `components.<name>.definedIn` names where the definition set came from. Properties the entity itself declared are merged over the inherited ones.

The emulator reports ONLY the entity-local properties the CreateEntity call happened to carry. A caller that creates a component type with `propertyDefinitions` and then creates an entity with a component of that type (the canonical TwinMaker modelling workflow, and the only way the property data plane is addressed) reads the component back with `properties` EMPTY, `definedIn` absent and `isInherited` hard-coded to `false` for anything it does report.

Concretely, three response members are produced from a constant rather than from the component type the component instantiates:
- `components.<name>.properties` omits every definition inherited from the component type;
- `components.<name>.definedIn` is always `None` (`defined_in: None` in get_entity.rs);
- `properties.<name>.definition.isInherited` is always `false` (`is_inherited: false` in state.rs `property_definition_response`).

The same root cause makes GetEntity report `compositeComponents` as an always-empty map with `areAllCompositeComponentsReturned: true`.

## Evidence

Spec: the Smithy model makes `PropertyDefinitionResponse.isInherited` a REQUIRED member documented as "A Boolean value that specifies whether the property definition is inherited from a parent entity", and `ComponentResponse.definedIn` is documented as "The name of the property definition set in the request" — both exist precisely to describe definitions that come from the component type, so a response that can only ever say `isInherited: false` / `definedIn: null` cannot describe a typed component. Well-established service behavior: a component instantiating a component type exposes that type's property definitions on the entity (this is how GetEntity/GetPropertyValue address properties the entity never re-declared).

Handler: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_entity.rs — `components_response()` maps only `entity.components[*].properties` (the stored request shapes) and sets `defined_in: None` and `composite_components: Some(HashMap::new())`; src/state.rs `ComponentRecord` doc states outright "Inheritance from a component *type* is not represented: component types are a later slice, so every definition stored here is the entity-local one", and `property_definition_response()` hard-codes `is_inherited: false`. That comment is now stale: CreateComponentType/GetComponentType are implemented and `Workspace::component_types` exists in the same state struct.

Tests: no coverage. tests/all/workspace_entity_lifecycle.rs asserts only that the component members supplied to CreateEntity round-trip (lines 267-413), and the component type id it uses (`com.example.sensor`) is never actually created, so no test exercises a component whose type exists. `grep -r 'definedIn|is_inherited' tests/` returns nothing.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}, "Serial": {"dataType": {"type": "STRING"}}}) and wait until it reports ACTIVE.
3. CreateEntity(workspaceId="ws", entityId="pump", entityName="Pump", components={"Sensor": {"componentTypeId": "com.example.sensor"}}).
4. GetEntity(workspaceId="ws", entityId="pump").
5. Expected (AWS): components.Sensor.properties carries Temperature and Serial, each with definition.dataType from the component type and definition.isInherited = true, and components.Sensor.definedIn names the definition set (the component type).
6. Actual (emulator): components.Sensor.properties is {} , definedIn is absent, and nothing is ever reported with isInherited = true.
