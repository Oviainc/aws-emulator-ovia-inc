Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Component-type abstractness is hardcoded false, so the isAbstract filter can never select an abstract type

_Filed: 2026-10-24T13:56:40Z_
_Operation: ListComponentTypes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real IoT TwinMaker): a component type is ABSTRACT when it cannot be instantiated — the service's built-in abstract base types, and (per the behavior already recorded for this service) a user-created type whose only property is a time-series property with `isStoredExternally=true` that declares no backing `dataReader`/`dataWriter` function. `ListComponentTypes` with `filters: [{"isAbstract": true}]` must report exactly those types, `filters: [{"isAbstract": false}]` must exclude them, and `GetComponentType` must report `isAbstract: true` for such a type.

Actual (emulator): abstractness is not derived from the stored record at all — `crate::state::ComponentType::is_abstract()` is a literal `false` (state.rs, `pub fn is_abstract(&self) -> bool { false }`). `ListComponentTypes`' filter arm `ListComponentTypesFilter::isAbstract(is_abstract) => component_type.is_abstract() == *is_abstract` therefore makes `isAbstract: true` return an ALWAYS-EMPTY page and `isAbstract: false` return every component type in the workspace, including the ones AWS classifies as abstract. `GetComponentType` reports `isAbstract: Some(false)` for every type for the same reason.

This is distinct from the already-open issues on this service: the open GetComponentType issue covers `extendsFrom` inheritance (inherited property definitions / isInherited), and the open CreateWorkspace issue covers the missing built-in component types; neither asks for abstractness to be computed, and neither mentions the `isAbstract` filter or the `isAbstract` output member.

## Evidence

Spec: the Smithy model defines `ListComponentTypesFilter` with an `isAbstract` variant ("whether the component type is abstract") and `GetComponentTypeResponse.isAbstract`, so abstractness is a property of a stored component type that both a read and a listing must answer. Well-established behavior: the abstractness rule is already recorded against this very service — open issue 20261023T123634Z-iottwinmaker-iot-twinmaker-createentity-accepts-abstract-component-types (cause 1) documents a real-AWS trace in which a type whose only property is an externally-stored time-series property with no dataReader/dataWriter is treated as ABSTRACT and refused for component creation ('Abstract ComponentType <id> can not be used for creating a component'); such a type is exactly what AWS reports as `isAbstract: true` and what the `isAbstract: true` filter selects.

Handler/state: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs `ComponentType::is_abstract()` returns the literal `false` ("no component type created through this API is abstract"); src/ops/list_component_types.rs `matches_filter()` isAbstract arm compares against that constant; src/ops/get_component_type.rs reports `is_abstract: Some(component_type.is_abstract())`.

Tests: tests/all/component_type_listing.rs:685-692 asserts `isAbstract: false` selects EVERY created type and `isAbstract: true` selects NOTHING — but only over a workspace of plain concrete types, so the abstract case (externally-stored time-series property, no functions) is never created and the divergence is unasserted. tests/all/component_type_lifecycle.rs:238 likewise only reads back `isAbstract` for concrete types.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.abstract", propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true, "isStoredExternally": true}}) with NO `functions` — i.e. no dataReader/dataWriter to serve the externally stored values.
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.concrete", propertyDefinitions={"Label": {"dataType": {"type": "STRING"}}}).
4. ListComponentTypes(workspaceId="ws", filters=[{"isAbstract": true}]). Expected (AWS): the page reports com.example.abstract (plus the workspace's built-in abstract types). Actual: componentTypeSummaries is empty.
5. ListComponentTypes(workspaceId="ws", filters=[{"isAbstract": false}]). Expected (AWS): com.example.concrete only. Actual: both ids.
6. GetComponentType(workspaceId="ws", componentTypeId="com.example.abstract"). Expected (AWS): isAbstract true. Actual: isAbstract false.
