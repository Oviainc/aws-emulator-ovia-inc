Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# UpdateComponentType accepts extendsFrom / compositeComponentTypes naming component types that do not exist

_Filed: 2026-10-24T03:47:17Z_
_Operation: UpdateComponentType | Class: validation_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateComponentType applies the request's `extendsFrom`, `compositeComponentTypes` and `propertyGroups` to the stored record without resolving any id or property name it references, so an update can leave a component type whose declared inheritance and composition are dangling — and can do it to a component type that was created correctly.

Expected (real IoT TwinMaker): every id an update puts in `extendsFrom` must name a component type that exists in the workspace (a custom one or a built-in type), and every `compositeComponentTypes[*].componentTypeId` likewise; a `propertyGroups[*].propertyNames` entry must name a property the component type defines (its own or an inherited one). An update that references an id or property the service does not hold is refused synchronously with HTTP 400 `ValidationException` and the stored record is left exactly as it was.

Actual (emulator): the handler's only validations are the workspace lookup (404), the component-type lookup (404) and the per-definition `dataType` rule (`property_definition_needs_data_type` over the PATCHED record). `extendsFrom`, `compositeComponentTypes` and `propertyGroups` are then moved verbatim onto the record and committed with 200 / state `UPDATING`. So `UpdateComponentType(ws, existingType, extendsFrom=["com.example.nothing"])` answers 200 and `GetComponentType` reports the dangling parent. Because a non-empty `extendsFrom` is also what licenses a `PropertyDefinitionRequest` with no `dataType`, an unvalidated parent additionally lets the update store a definition whose `dataType` can never be resolved — which `GetComponentType` then silently drops from `propertyDefinitions`.

The same lack of a lookup also makes an inheritance CYCLE reachable through this operation only (create A extendsFrom B, then update B to extendsFrom A), a state no sequence of real CreateComponentType/UpdateComponentType calls can produce.

## Evidence

Spec: the model documents `UpdateComponentTypeRequest.extendsFrom` as the parent component type to extend and `CompositeComponentTypeRequest.componentTypeId` as the id of the referenced component type; `PropertyDefinitionResponse.dataType`/`isInherited` are required members, so inheritance is only describable when the parent exists. `ValidationException` (HTTP 400) is a modeled error of UpdateComponentType.

Well-established behavior / internal asymmetry: the emulator already enforces referential integrity in the other direction — DeleteComponentType refuses while another type `extendsFrom` it or names it in `compositeComponentTypes` (src/ops/delete_component_type.rs:207-221 via `Workspace::component_type_references`). The identical create-path gap is already filed and accepted as real for CreateComponentType (open issue 20261024T004956Z-iottwinmaker-createcomponenttype-validation); this issue is the UPDATE path, which that issue does not touch and which can additionally corrupt a component type that was created valid.

Handler: src/ops/update_component_type.rs:225-322 — `if let Some(extends_from) = input.extends_from { updated.extends_from = extends_from; }`, the same for `composite_component_types` and `property_groups`, with no lookup in `workspace.component_types` and no check that a property group's `propertyNames` exist; the only refusal is the patched-record `dataType` rule at lines 303-316.

Tests: existing tests assert the LENIENT behavior — tests/all/component_type_update.rs:189-193 updates `extendsFrom` to `["com.example.parent", "com.example.other"]` (neither created in that workspace) and asserts 200 plus the replaced value, and :318-329 replaces `compositeComponentTypes` with `{"outlet": {...}}` the same way. No test asserts a refusal for an unknown parent, an unknown composite type, or a property group naming an undefined property.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.child", propertyDefinitions={"serial": {"dataType": {"type": "STRING"}}}) -> 200.
3. UpdateComponentType(workspaceId="ws", componentTypeId="com.example.child", extendsFrom=["com.example.nothing"]).
4. Expected (AWS): HTTP 400 ValidationException naming the unknown parent, and GetComponentType still reports the component type exactly as it was created.
5. Actual (emulator): HTTP 200 with state UPDATING, and GetComponentType reports extendsFrom ["com.example.nothing"].
6. Repeat with compositeComponentTypes={"inlet": {"componentTypeId": "com.example.nothing"}} — expected 400, actual 200.
7. Repeat with propertyGroups={"readings": {"groupType": "TABULAR", "propertyNames": ["notAProperty"]}} — expected 400, actual 200 and GetComponentType reports the group.
8. Cycle: CreateComponentType("com.example.a", extendsFrom=["com.example.b"]) after creating "com.example.b", then UpdateComponentType("com.example.b", extendsFrom=["com.example.a"]) — expected 400, actual 200.
