Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# A created workspace carries none of IoT TwinMaker's built-in component types

_Filed: 2026-10-24T00:49:56Z_
_Operation: CreateWorkspace | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real IoT TwinMaker): every workspace is provisioned with the service's built-in component types (e.g. `com.amazon.iotsitewise.connector`, `com.amazon.iottwinmaker.alarm.basic`, `com.amazon.iottwinmaker.documents`, and the 3D/scene component types). They exist the instant CreateWorkspace returns: GetComponentType resolves them (reporting `isAbstract`/inherited schema), ListComponentTypes reports them for a brand-new workspace, and CreateComponentType/CreateEntity may legitimately reference them from `extendsFrom` and `components[*].componentTypeId`.

Actual (emulator): CreateWorkspace inserts a `crate::state::Workspace` whose `component_types` map is `BTreeMap::new()` (src/ops/create_workspace.rs:200-215), and nothing else ever seeds it — only CreateComponentType writes into it. So in a fresh workspace GetComponentType("ws", "com.amazon.iottwinmaker.alarm.basic") answers 404 ResourceNotFoundException where AWS answers 200, the standard alarm pattern (`extendsFrom: ["com.amazon.iottwinmaker.alarm.basic"]`) has no parent to inherit from, and a workspace that AWS reports as non-empty of component types is reported empty here.

This is the state-model half of the two open inheritance issues: GetComponentType/GetEntity cannot resolve an inherited schema for the built-in parents a real client extends until the built-ins are present in the store.

## Evidence

Spec/well-established behavior: the Smithy model exposes `extendsFrom`, `isAbstract` and `isSchemaInitialized` precisely to describe the service's predefined abstract component types, and AWS documents a fixed set of built-in component types available in every IoT TwinMaker workspace (the IoT SiteWise connector type, the basic alarm type, the document type, and the scene/3D types). None of them is created through this API, so an emulator must seed them at workspace creation.

Handler/state: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_workspace.rs:200-215 stores `component_types: ::std::collections::BTreeMap::new()`; src/state.rs `Workspace::component_types` is documented as "the component types created in this workspace" and only CreateComponentType inserts into it. GetComponentType (src/ops/get_component_type.rs:218-229) 404s for anything not in that map, and GetComponentType's own comment asserts "the service's own abstract base types are built in, none of which this emulator holds".

Tests: no test creates a workspace and reads a built-in component type back; tests/all/component_type_lifecycle.rs:140 only uses a built-in-looking id as an unresolved `extendsFrom` string. The gap is unasserted.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. GetComponentType(workspaceId="ws", componentTypeId="com.amazon.iottwinmaker.alarm.basic").
3. Expected (AWS): HTTP 200 describing the built-in alarm component type (abstract, with its alarm_status/alarm_reason schema).
4. Actual (emulator): HTTP 404 ResourceNotFoundException — the workspace holds no component type it did not create.
5. Same for com.amazon.iotsitewise.connector and com.amazon.iottwinmaker.documents; once ListComponentTypes is implemented, a fresh workspace will likewise report an empty list where AWS reports the built-ins.
