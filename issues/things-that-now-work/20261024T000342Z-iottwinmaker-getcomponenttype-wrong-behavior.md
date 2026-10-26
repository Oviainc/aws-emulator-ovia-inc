Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# GetComponentType ignores extendsFrom: inherited property definitions are never reported and a refining definition stored without dataType is silently dropped

_Filed: 2026-10-24T00:03:42Z_
_Operation: GetComponentType | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetComponentType reports only the component type's OWN stored members; the `extendsFrom` parents are echoed back but never resolved. Two observable divergences follow.

1. Inherited definitions are missing. Real IoT TwinMaker reports, for a component type that extends another, the parent's `propertyDefinitions` / `functions` / `propertyGroups` in the response with `isInherited: true` (this is why the alarm pattern — extendsFrom com.amazon.iottwinmaker.alarm.basic — shows alarm_status/alarm_reason on the child type). The emulator returns only definitions the child itself declared and hard-codes `isInherited: false` on every definition, function and property group it reports.

2. A definition the service ACCEPTED disappears on read. CreateComponentType/UpdateComponentType deliberately allow a `PropertyDefinitionRequest` with no `dataType` when `extendsFrom` is non-empty (the "refines an inherited definition" case, state.rs `property_definition_needs_data_type`). GetComponentType then runs every stored definition through `property_definition_response()`, which returns `None` when `dataType` is absent, and the `filter_map` DROPS the entry. So the property definition the caller created is persisted, answered 200, and is then invisible in every read — the handler even logs "a stored property definition carries no dataType; omitting it" when it happens. AWS resolves the dataType from the parent and reports the property.

## Evidence

Spec: `PropertyDefinitionResponse.dataType` and `.isInherited` are both REQUIRED members (`isInherited` = "whether the property definition is inherited from a parent entity"); `CreateComponentTypeRequest.extendsFrom` is documented as "Specifies the parent component type to extend", and inheritance is the entire purpose of the member — a read that never reports an inherited definition cannot describe an extending type. The internal inconsistency is self-evidencing: the write path accepts a definition whose dataType is omitted because it is inherited, and the read path has no inheritance resolution to supply it, so it omits the member instead.

Handler: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/get_component_type.rs — `property_definitions` is built with `.filter_map(|…| crate::state::property_definition_response(definition).map(…))` over the component type's own map only; `function_response()` sets `is_inherited: Some(false)` and `property_group_response()` sets `is_inherited: false`; `extends_from` is echoed verbatim and never followed. src/state.rs `property_definition_response()` returns `None` (with a tracing::warn) for a definition without `dataType`, and hard-codes `is_imported/is_final/is_inherited = false`.

Tests: no coverage of either symptom. tests/all/component_type_lifecycle.rs:717-726 creates exactly this case (a definition with no dataType under `extendsFrom: ["com.example.present"]`) and asserts ONLY that the create returns 200 — it never reads the component type back, so the silent drop is unasserted. `grep is_inherited tests/` returns nothing.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.base", propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}}}) and wait until ACTIVE.
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.child", extendsFrom=["com.example.base"], propertyDefinitions={"Temperature": {"isRequiredInEntity": true}}) — returns 200.
4. GetComponentType(workspaceId="ws", componentTypeId="com.example.child").
5. Expected (AWS): propertyDefinitions carries Temperature with dataType DOUBLE resolved from the parent, isRequiredInEntity true and isInherited reported for the inherited part of the schema.
6. Actual (emulator): propertyDefinitions is {} — the stored definition is dropped because it carries no dataType — and no inherited definition from com.example.base appears at all.
