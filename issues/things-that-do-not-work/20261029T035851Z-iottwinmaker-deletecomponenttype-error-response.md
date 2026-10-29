Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# The in-use refusal lists every stored reference in full, so ValidationException.message runs past the modelled 2048-character bound that the sibling DeleteWorkspace guards against by reporting a count

_Filed: 2026-10-29T03:58:51Z_
_Operation: DeleteComponentType | Class: error-response_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a component type is still referenced, the handler builds the refusal by joining EVERY reference `Workspace::component_type_references` found into the `ValidationException` message. That list is caller-grown: it has one entry per entity component and per entity composite component that instantiates the type, plus one per component type that extends it or names it in `compositeComponentTypes`, bounded only by this crate's own store ceilings (`MAX_ENTITIES_PER_WORKSPACE = 1000`, `MAX_COMPONENTS_PER_ENTITY = 100`). The modelled `ValidationException$message` is `ErrorMessage` @length(0..2048), so a workspace with as few as ~20 referencing entities (entity ids are modelled up to 128 characters) already produces a message past that bound, and a full workspace produces a multi-megabyte error body.

The crate's other "delete is blocked by contents" guard does not do this: `DeleteWorkspace` refuses with a COUNT (`"workspace {} still contains {} entities; delete them first"`) for each of the four kinds of contained resource and names no individual resource, so its message is bounded by construction. `DeleteComponentType` is the only place in the crate that joins a store-derived, caller-grown list into a refusal message (every other `.join(...)` in `src/` joins a fixed modelled value list such as `SOURCE_TYPE_VALUES` or `SUPPORTED_FILTER_COMBINATIONS`). No comment or test justifies the divergence.

## Evidence

MODEL (smithy_query query="shape" shape="ValidationException"):
  "ValidationException": {... "members": {"message": {"target": "ErrorMessage", "length": {"min": 0, "max": 2048}}}}

THE HANDLER — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/delete_component_type.rs:236-250:
  "let references = workspace.component_type_references(&input.component_type_id);
   if !references.is_empty() {
       return Err(DeleteComponentTypeErrorModeled::ValidationException(
           ValidationException {
               message: Some(format!(
                   \"ComponentType {} in workspace {} is still referenced by {}: {}\",
                   input.component_type_id,
                   input.workspace_id,
                   references.len(),
                   references.join(\", \")"

WHAT IT JOINS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:1236-1249:
  "let mut references = Vec::new();
   for entity in self.entities.values() {
       for component in entity.components.values() {
           if component.component_type_id.as_deref() == Some(component_type_id) {
               references.push(format!(
                   \"entity {} component {}\",
                   entity.entity_id, component.component_name"

HOW LARGE THAT CAN GET — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:389,409:
  "pub const MAX_ENTITIES_PER_WORKSPACE: usize = 1000;"
  "pub const MAX_COMPONENTS_PER_ENTITY: usize = 100;"

THE SIBLING GUARD THAT REPORTS A COUNT INSTEAD — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/delete_workspace.rs:185-196:
  "if !workspace.entities.is_empty() {
       return Err(DeleteWorkspaceErrorModeled::ValidationException(
           ValidationException {
               message: Some(format!(
                   \"workspace {} still contains {} entities; delete them first\",
                   input.workspace_id,
                   workspace.entities.len()"
  (and the same count-only construction at delete_workspace.rs:199-208, :210-221, :229-240)

TEST COVERAGE: no test pins this message — `grep -n 'still referenced by' tests/` returns no matches; tests/all/component_type_delete.rs and tests/all/component_type_delete_composite_references.rs assert the status and error type, so bounding the message leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateWorkspace `w1`; CreateComponentType `com.example.pump` in it with one property definition.
2. CreateEntity 30 times, each with a 128-character `entityId` and one component of `com.example.pump`.
3. DELETE /workspaces/w1/component-types/com.example.pump.
4. Observe HTTP 400 ValidationException whose `message` concatenates all 30 `"entity <128 chars> component <name>"` strings and exceeds the 2048-character maximum the model declares for `ValidationException$message`.
5. Compare with the crate's other contents guard: DELETE /workspaces/w1 answers 400 with `"workspace w1 still contains 30 entities; delete them first"`, a bounded message that names no individual entity.
