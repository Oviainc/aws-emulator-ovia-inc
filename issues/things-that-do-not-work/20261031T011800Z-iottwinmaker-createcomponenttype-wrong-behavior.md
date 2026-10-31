Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 64b86d5e14a7a25edd6b553817d01f64764c1ea9

# isSingleton is stored and reported but enforced by nothing, so one entity may hold two components of a singleton component type

_Filed: 2026-10-31T01:17:59Z_
_Operation: CreateComponentType | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `64b86d5e14a7a25edd6b553817d01f64764c1ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateComponentType` (and `UpdateComponentType`) accept `isSingleton`, store it on the component-type record and report it back through `GetComponentType`, but no writer of an entity's components ever consults it. The flag is therefore inert: a workspace can hold a component type created with `isSingleton: true`, and `CreateEntity` (or `UpdateEntity`) can then put TWO top-level components on the same entity whose `componentTypeId` both name that type, with no refusal — `GetEntity`/`ListComponents` report both.

The model states the rule on the member itself: the value "specifies whether an entity can have more than one component of this type", and this crate's own state module restates that sentence verbatim over the stored field. The crate already implements the sibling component-type-level instantiation rule in exactly the place this one belongs: `crate::state::abstract_component_type_violation` is called from `create_entity.rs` and `update_entity.rs` for every top-level and composite component before anything is stored, and the state doc for `is_abstract` says it is stored precisely so that "the component types `CreateEntity` refuses to instantiate cannot drift apart" from what `GetComponentType` reports. `is_singleton` sits two fields above it, stored the same way, and no handler reads it.

This is not a gap the writers declare: `create_entity.rs` enumerates its deliberate gaps ("Two deliberate gaps: a `componentTypeId` the workspace does not hold at all is still stored without being refused ... and authorization is not emulated") and the singleton multiplicity rule is not among them, nor is it mentioned anywhere in the crate or its tests.

## Evidence

Model (smithy_query(query="doc", shape="CreateComponentTypeRequest", member="isSingleton")) — the member states the rule:
```
"documentation": "<p>A Boolean value that specifies whether an entity can have more than one component of\n         this type.</p>"
```
(the identical sentence is on `UpdateComponentTypeRequest$isSingleton` and `GetComponentTypeResponse$isSingleton`, quoted in the handler headers at `src/ops/create_component_type.rs:119`, `src/ops/update_component_type.rs:121`, `src/ops/get_component_type.rs:98`).

The crate restates the same rule over the stored field — `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:2381-2384`:
```
    /// "Whether an entity can have more than one component of this type" — the
    /// member's own documentation. Stored resolved: a create that names none
    /// leaves it `false`.
    pub is_singleton: bool,
```

It is written on create — `src/ops/create_component_type.rs:391`:
```
            is_singleton: input.is_singleton.unwrap_or(false),
```
patched on update — `src/ops/update_component_type.rs:309-311`:
```
        if let Some(is_singleton) = input.is_singleton {
            updated.is_singleton = is_singleton;
        }
```
and reported — `src/ops/get_component_type.rs:255`:
```
            is_singleton: Some(component_type.is_singleton),
```

Nothing else reads it. `grep -rn "is_singleton" src/` matches ONLY those three handlers plus the field declaration (`state.rs:2384`) and the built-in catalogue (`state.rs:3001`, `is_singleton: false`). Neither `create_entity.rs` nor `update_entity.rs` contains the identifier at all.

The sibling rule it is missing beside — `src/ops/create_entity.rs:368-374`:
```
        for component in component_records.values() {
            if let Some(refusal) =
                abstract_component_type_refusal(workspace, component.component_type_id.as_deref())
            {
                return Err(refusal.into());
            }
        }
```
and `src/ops/update_entity.rs:455-460` calls `crate::state::abstract_component_type_violation` for the same purpose, whose doc says it is "Shared by every writer of an entity's components, top-level and composite alike, so a create and an update hold one path to one rule" (`src/state.rs:2789-2791`). The equivalent `is_abstract` field is documented as stored so that "the component types `CreateEntity` refuses to instantiate cannot drift apart" (`src/state.rs:2388-2391`) — the singleton flag beside it drives no refusal anywhere.

The writers declare their gaps and this is not one — `src/ops/create_entity.rs:43-51`:
```
//! Two deliberate gaps: a `componentTypeId` the workspace does not hold at all
//! is still stored without being refused (the real service refuses an unknown
//! one, ...), and
//! authorization is not emulated, so the modelled `AccessDeniedException` is
//! never returned.
```

Test coverage: `tests/all/component_type_lifecycle.rs:86-87` creates `"singleton-type"` with `json!({"isSingleton": true, ...})` and `:231-233` only asserts the flag reads back; `tests/all/component_type_update.rs:197-200` asserts the flag flips. No test in `tests/all/` creates an entity with two components of a singleton type, so no existing expectation pins the current acceptance.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateComponentType in `w` with componentTypeId `com.example.motor` and body `{"isSingleton": true, "propertyDefinitions": {"rpm": {"dataType": {"type": "DOUBLE"}}}}` — accepted.
3. GetComponentType on `com.example.motor` — the response reports `"isSingleton": true`, so the service claims the type is a singleton.
4. CreateEntity in `w` with `{"entityId": "pump", "entityName": "Pump", "components": {"motorA": {"componentTypeId": "com.example.motor"}, "motorB": {"componentTypeId": "com.example.motor"}}}`.
5. Observe HTTP 200: the entity is created holding TWO components of the singleton component type, and GetEntity on `pump` reports both `motorA` and `motorB` with `componentTypeId: com.example.motor` — the multiplicity the member documents ("whether an entity can have more than one component of this type") is never enforced, where the sibling abstractness rule in the same loop (create_entity.rs:368-374) would have refused the request.
