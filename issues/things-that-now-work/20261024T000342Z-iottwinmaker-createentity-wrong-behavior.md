Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CreateEntity refuses every request that carries compositeComponents, including valid ones

_Filed: 2026-10-24T00:03:42Z_
_Operation: CreateEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEntity rejects any request whose `compositeComponents` map is non-empty with a blanket 400 ValidationException ("compositeComponents names a component path that does not exist"), without looking at the request at all. The check is unconditional: it never resolves the composite path against the parent component's component type.

Real IoT TwinMaker accepts `compositeComponents` when the path is valid — key `"<componentName>/<compositeComponentName>"` where the component named by the first segment instantiates a component type whose `compositeComponentTypes` declares the second segment. That configuration is constructible in this emulator today (CreateComponentType stores and reports `compositeComponentTypes`), so a caller that models a composite component type, instantiates it on an entity and passes the matching `compositeComponents` entry gets a 400 from the emulator and a 200 from AWS, and the entity is never created.

The same hole makes the read side permanently hollow: GetEntity reports `components.<name>.compositeComponents` as an always-empty map with `areAllCompositeComponentsReturned: true`, because nothing can ever be stored there.

Note: the blanket refusal does catch the one invalid case a test exercises, so the fix is to validate the path (refuse only when the component or the declared composite component type does not resolve) rather than to drop the check.

## Evidence

Spec: `CreateEntityRequest.compositeComponents` (CompositeComponentsMapRequest of CompositeComponentRequest{description, properties, propertyGroups}) is a modeled, documented input member, and `CompositeComponentTypesRequest` on CreateComponentType/UpdateComponentType is what declares the composite component types an entity may instantiate — so a valid combination exists and the model provides every piece of it. `ComponentResponse.compositeComponents` / `areAllCompositeComponentsReturned` exist to report them back.

Handler: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_entity.rs:279-293 — `if input.composite_components.as_ref().is_some_and(|c| !c.is_empty()) { return Err(ValidationException{...}) }`, i.e. refused on non-emptiness alone, before any lookup of the component or its type; the input is then never stored (`crate::state::Entity` has no composite-component field). src/ops/get_entity.rs sets `composite_components: Some(HashMap::new())` and `are_all_composite_components_returned: Some(true)` with the comment "Composite components cannot exist on a stored entity".

Tests: tests/all/workspace_entity_lifecycle.rs:1368-1379 asserts the 400 for `compositeComponents = {"sensor/inner": {...}}` on an entity with NO component named `sensor` — an invalid path AWS would also refuse — so no test covers the valid path, and nothing asserts that a resolvable composite path is accepted.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.inner", propertyDefinitions={"Pressure": {"dataType": {"type": "DOUBLE"}}}) and wait until ACTIVE.
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", compositeComponentTypes={"inner": {"componentTypeId": "com.example.inner"}}) and wait until ACTIVE.
4. CreateEntity(workspaceId="ws", entityId="pump", entityName="Pump", components={"Sensor": {"componentTypeId": "com.example.sensor"}}, compositeComponents={"Sensor/inner": {"description": "nested"}}).
5. Expected (AWS): 200, the entity is created, and GetEntity reports components.Sensor.compositeComponents containing the "inner" entry.
6. Actual (emulator): 400 ValidationException "compositeComponents names a component path that does not exist" and no entity is created, even though the path resolves against the stored component type.
