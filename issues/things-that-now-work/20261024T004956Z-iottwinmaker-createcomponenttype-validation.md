Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# CreateComponentType accepts extendsFrom (and compositeComponentTypes) naming component types that do not exist

_Filed: 2026-10-24T00:49:56Z_
_Operation: CreateComponentType | Class: validation_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateComponentType never resolves the ids its request references against the workspace's component-type registry, so a component type can be created whose declared inheritance is dangling.

Expected (real IoT TwinMaker): every id in `extendsFrom` must name a component type that exists in the workspace — either a custom one created there or one of the service's built-in types — and every `compositeComponentTypes[*].componentTypeId` likewise. Naming an id the service does not hold is refused synchronously with HTTP 400 `ValidationException` and nothing is persisted.

Actual (emulator): the handler only checks (a) the workspace exists, (b) the componentTypeId is not taken, (c) the per-workspace quota, and (d) that each own property definition carries a `dataType` unless `extendsFrom` is non-empty. `extendsFrom` is then stored verbatim (`let extends_from = input.extends_from.clone().unwrap_or_default();` … `extends_from,` on the stored record) and `compositeComponentTypes` is stored verbatim, with no lookup in `workspace.component_types`. `CreateComponentType(ws, "com.example.child", extendsFrom=["com.example.nothing"])` answers 200 and GetComponentType then reports `extendsFrom: ["com.example.nothing"]`.

The missing check is also what makes the accepted-then-dropped definition possible: because a non-empty `extendsFrom` is what licenses a `PropertyDefinitionRequest` with no `dataType` (state.rs `property_definition_needs_data_type`), an unvalidated parent lets the service store a definition whose `dataType` can never be resolved from anywhere.

## Evidence

Spec: the model documents `CreateComponentTypeRequest.extendsFrom` as "Specifies the parent component type to extend" and `CompositeComponentTypeRequest.componentTypeId` as the id of the referenced component type; `PropertyDefinitionResponse.isInherited`/`dataType` are required members, so inheritance is only describable when the parent exists. ValidationException (HTTP 400) is a modeled error of CreateComponentType.

Well-established behavior: IoT TwinMaker enforces referential integrity on component types in both directions — the emulator itself already refuses DeleteComponentType while another type extends it or names it in `compositeComponentTypes` (src/ops/delete_component_type.rs:207-221 via `Workspace::component_type_references`). The store can therefore only hold a dangling reference because the create path never checks, which is an asymmetry introduced by the emulator.

Handler: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_component_type.rs:210-343 — the only validations are the workspace lookup, the id conflict, the quota, and the per-definition dataType rule; neither `extends_from` nor `composite_component_types` is resolved against `workspace.component_types` before the record is inserted.

Tests: existing tests assert the LENIENT behavior rather than covering the gap — tests/all/component_type_lifecycle.rs:140 creates a type with `extendsFrom: ["com.amazonaws.iottwinmaker.alarm.basic", "com.example.base"]` (neither exists in that workspace) and expects 200, and tests/all/component_type_lifecycle.rs:717-726 creates `extendsFrom: ["com.example.present"]` — a type that was never created — and asserts 200. Those assertions would need to change with the fix. No test asserts a refusal for an unknown parent.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.child", extendsFrom=["com.example.nothing"], propertyDefinitions={"Temperature": {"isRequiredInEntity": true}}).
3. Expected (AWS): HTTP 400 ValidationException naming the unknown parent component type; GetComponentType("ws", "com.example.child") is then 404 ResourceNotFoundException.
4. Actual (emulator): HTTP 200 with state CREATING, and GetComponentType reports the dangling extendsFrom (with the dataType-less definition silently dropped).
5. Repeat with compositeComponentTypes={"inlet": {"componentTypeId": "com.example.nothing"}} — expected 400 ValidationException, actual 200.
