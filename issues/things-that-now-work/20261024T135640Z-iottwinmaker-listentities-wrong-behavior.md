Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# The externalId filter only matches entity-local property definitions, so an external id declared on the component type never selects its entity

_Filed: 2026-10-24T13:56:40Z_
_Operation: ListEntities | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real IoT TwinMaker): `ListEntities` with `filters: [{"externalId": "<id>"}]` finds the entity whose component carries that external id. The `isExternalId` flag is part of the PROPERTY DEFINITION, which normally lives on the component TYPE (`CreateComponentType.propertyDefinitions.<name>.isExternalId = true`); an entity's component then supplies only the VALUE for that property (`components.<name>.properties.<name>.value.stringValue`). That is the documented primary-key lookup path ("The external-Id property of a component. The external-Id property is the primary key of an external storage system") and the way SiteWise-synced / imported entities are addressed.

Actual (emulator): the filter is evaluated purely against the entity's OWN stored property records — `ComponentRecord::external_ids()` -> `PropertyRecord::external_id()`, which returns `None` unless the record's locally stored `definition.isExternalId` is `true`. A component created the normal way (componentTypeId + a value for the type's external-id property, no inline definition) has `definition: None`, so the filter matches nothing and `ListEntities` answers an empty page where AWS returns the entity. The same root cause (the handler never resolves `components[*].componentTypeId` against the workspace's component types) makes the filter work only for the unusual case where the caller re-declares `isExternalId` inline on the entity.

## Evidence

Spec: `ListEntitiesFilter.externalId` is documented as "The external-Id property of a component. The external-Id property is the primary key of an external storage system"; `PropertyDefinitionRequest.isExternalId` is a member of the component-type property definition, and `PropertyRequest` on an entity's component carries `definition` as OPTIONAL precisely because the definition normally comes from the type. So an entity whose component only supplies the value must still be selectable by its external id.

Handler/state: src/ops/list_entities.rs `matches_filter()` -> `Some(ListEntitiesFilter::externalId(external_id)) => entity.components.values().any(|component| component.external_ids().any(|id| id == external_id))`; src/state.rs `PropertyRecord::is_external_id()` reads `self.definition.as_ref().and_then(|d| d.is_external_id).unwrap_or(false)` — a stored property with no local definition can never be an external id. src/ops/create_entity.rs stores `componentTypeId` verbatim and never consults `workspace.component_types` for a top-level component, so no definition is ever inherited onto the record.

Tests: tests/all/workspace_entity_lifecycle.rs:1045-1051 exercises the externalId filter only for entities that declare `isExternalId` INLINE on the entity's own property, so the component-type-declared case is unasserted. This is a different operation and observable from the open GetComponentType/GetEntity inheritance issues (which cover what a single read reports, not which entities a listing selects).

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"serial": {"dataType": {"type": "STRING"}, "isExternalId": true}}); wait for ACTIVE.
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="e1", components={"Sensor": {"componentTypeId": "com.example.sensor", "properties": {"serial": {"value": {"stringValue": "EXT-1"}}}}}) — note the entity supplies the VALUE only, as a client does when the definition lives on the type.
4. ListEntities(workspaceId="ws", filters=[{"externalId": "EXT-1"}]).
5. Expected (AWS): entitySummaries reports e1.
6. Actual (emulator): entitySummaries is empty — the stored property has no local definition, so it is not recognised as the external id.
