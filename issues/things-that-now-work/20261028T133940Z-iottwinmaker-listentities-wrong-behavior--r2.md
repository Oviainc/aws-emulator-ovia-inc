Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# The componentTypeId filter skips an entity's composite components, although the externalId filter beside it walks them for the stated reason that a composite component IS a component of the entity

_Filed: 2026-10-28T13:39:40Z_
_Operation: ListEntities | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListEntities`' `matches_filter` answers the three modelled filters of `ListEntitiesFilter`. Two of them resolve what "a component of the entity" means, and they disagree inside the same function:

- `externalId` walks the entity's top-level components AND the composite components stored inside each of them, and the helper it calls carries the rule as a doc comment: "A composite component is a component of the entity too — `GetEntity` reports it as one — and it carries a `componentTypeId` and properties of its own".
- `componentTypeId` tests only `entity.components.values()` — the top-level components — and never looks at `component.composite_components`.

So an entity whose only component of type X is a COMPOSITE component (created through `CreateEntity`'s `compositeComponents` / `UpdateEntity`'s `compositeComponentUpdates`, whose records carry a `component_type_id` of their own) is not selected by `filters: [{"componentTypeId": "X"}]`, while the very same entity IS selected by `filters: [{"externalId": ...}]` for an external id declared on that composite component. `GetEntity` and `ListComponents` both report that composite component with `componentTypeId: X`, so the listing's filter contradicts what every reader of the same entity says the entity holds.

The same crate resolves the identical question ("which stored components are instances of this component type") over composite components in two sibling operations — `GetPropertyValue` and `GetPropertyValueHistory` both extend their `componentTypeId` scope with `component.composite_components.values()`. `ListEntities` is the only place that stops at the top level.

No test pins the current behaviour: the only `componentTypeId`-filter rows seed entities with top-level components alone, so extending the walk changes no existing assertion.

## Evidence

THE TWO SIDES, IN ONE FUNCTION — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_entities.rs:300-308:
```
        Some(ListEntitiesFilter::componentTypeId(component_type_id)) => {
            entity.components.values().any(|component| {
                component.component_type_id.as_deref() == Some(component_type_id.as_str())
            })
        }
        Some(ListEntitiesFilter::externalId(external_id)) => entity
            .components
            .values()
            .any(|component| carries_external_id(workspace, component, external_id)),
```

THE RULE THE SIBLING FILTER STATES — src/ops/list_entities.rs:312-320:
```
/// Whether `component`, or any composite component inside it, carries
/// `external_id` as the value of its external-id property.
///
/// A composite component is a component of the entity too — `GetEntity` reports
/// it as one — and it carries a `componentTypeId` and properties of its own, so
/// the external id it names is resolved the same way the holding component's
/// is: ...
```
and the walk it performs, src/ops/list_entities.rs:329-336:
```
        || component.composite_components.values().any(|composite| {
            workspace
                .component_external_ids(
                    Some(composite.component_type_id.as_str()),
                    &composite.properties,
                )
                .contains(external_id)
        })
```

THE SAME QUESTION, ANSWERED WITH COMPOSITE COMPONENTS BY TWO SIBLING OPERATIONS —
src/ops/get_property_value.rs:514-526:
```
    for entity in workspace.entities.values() {
        for component in entity.components.values() {
            if component
                .component_type_id
                .as_deref()
                .is_some_and(|instantiated| instance_type_ids.contains(instantiated))
            {
                components.push(scoped_component(&entity.entity_id, component));
            }
            for composite in component.composite_components.values() {
                if instance_type_ids.contains(composite.component_type_id.as_str()) {
                    components.push(scoped_composite_component(&entity.entity_id, composite));
                }
            }
```
src/ops/get_property_value_history.rs:728-731:
```
            for composite in component.composite_components.values() {
                if instance_type_ids.contains(composite.component_type_id.as_str()) {
                    components.push(scoped_composite(&entity.entity_id, composite));
                }
            }
```

WHAT THE READERS REPORT FOR THE SKIPPED COMPONENT — src/ops/list_components.rs:385-388 builds the composite's summary with `component_type_id: composite.component_type_id.clone()`, and src/ops/get_entity.rs:382-384 reports the same id on `ComponentSummary`.

MODEL — `smithy_query(query="shape", shape="ListEntitiesFilter")`: `"componentTypeId": {"target": "ComponentTypeId", ..., "doc": "<p>The ID of the component type in the entities in the list.</p>"}` — the filter is about the component type in the entities, with no restriction to top-level components.

TEST COVERAGE: the filter rows are tests/all/workspace_entity_lifecycle.rs:1029-1043 ("componentTypeId=com.example.sensor" -> ["child1"], "componentTypeId=com.example.alarm" -> ["child2"], "componentTypeId matches nothing" -> []). The entities they seed (workspace_entity_lifecycle.rs:961-1010) carry only top-level components and no `compositeComponents`, so no assertion depends on composite components being skipped.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.inner", propertyDefinitions={"Pressure": {"dataType": {"type": "DOUBLE"}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", compositeComponentTypes={"inner": {"componentTypeId": "com.example.inner"}}).
4. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Sensor": {"componentTypeId": "com.example.sensor"}}, compositeComponents={"Sensor/inner": {}}).
5. GetEntity(workspaceId="ws", entityId="e1") — components.Sensor.compositeComponents.inner reports componentTypeId "com.example.inner"; ListComponents(workspaceId="ws", entityId="e1", componentPath="Sensor") reports the same.
6. ListEntities(workspaceId="ws", filters=[{"componentTypeId": "com.example.inner"}]) — entitySummaries is empty, although the entity holds a component of that type.
7. Contrast the sibling filter: declare an isExternalId property on the composite component and ListEntities(filters=[{"externalId": "..."}]) does select e1, because that filter walks the same composite components (list_entities.rs:329-336).
