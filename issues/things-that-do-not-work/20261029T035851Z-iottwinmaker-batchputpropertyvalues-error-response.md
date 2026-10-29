Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# A per-entry errorMessage is built from caller-supplied componentPath / propertyName, so it runs past the 256-character bound the modelled BatchPutPropertyError$errorMessage declares

_Filed: 2026-10-29T03:58:51Z_
_Operation: BatchPutPropertyValues | Class: error-response_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every per-entry refusal this handler reports is pushed into the response verbatim as `BatchPutPropertyError.errorMessage`. The model bounds that member at 256 characters, but the messages the handler builds interpolate request identifiers whose own modelled bounds are larger or nearly as large: `EntityPropertyReference$propertyName` is `Name` @length(1..256) and `EntityPropertyReference$componentPath` is `ComponentPath` @length(1..2048). A single legal request therefore produces a response member that cannot fit its own modelled shape:

* a 256-character `propertyName` naming a property that is not a time-series property yields `"<256 chars> is not marked as a time-series property."` — 297 characters;
* a 2048-character `componentPath` that resolves to nothing yields `"Entity ... in workspace ... has no component at componentPath <2048 chars>."` — well over 2200 characters.

Nothing clamps or summarises the message before it is stored on the error entry, and no comment in the handler mentions the member's bound (the handler's own comment block about `errorCode`/`errorMessage` at batch_put_property_values.rs:229-243 discusses only which code to report). The refusal is the one part of this operation that answers 200, so an over-long message is not caught by any error path either.

## Evidence

MODEL (smithy_query query="shape" shape="BatchPutPropertyError"):
  "errorMessage": {"target": "String", "required": true, "length": {"min": 1, "max": 256}, "pattern": ".*", "doc": "<p>The error message.</p>"}

MODEL (smithy_query query="shape" shape="EntityPropertyReference"):
  "componentPath": {"target": "ComponentPath", "length": {"min": 1, "max": 2048}, ...}
  "propertyName": {"target": "Name", "required": true, "length": {"min": 1, "max": 256}, ...}

THE HANDLER, copying the message straight onto the modelled member —
services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/batch_put_property_values.rs:200-208:
  "Err(error) => error_entries.push(BatchPutPropertyErrorEntry {
       errors: vec![BatchPutPropertyError {
           error_code: error.code.to_string(),
           error_message: error.message,"

THE MESSAGES IT CARRIES:
- services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:3147-3149 (reached from batch_put_property_values.rs:320-324):
  "pub fn property_not_time_series_message(property_name: &str) -> String {
       format!(\"{property_name} is not marked as a time-series property.\")"
  -> 41 fixed characters + a `propertyName` the model allows 256 of = up to 297.
- services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/batch_put_property_values.rs:426-431:
  "(Some(component_path), _) => at_path(entity, component_path).ok_or_else(|| {
       absent_entry(format!(
           \"Entity {entity_id} in workspace {workspace_id} has no component at \\
            componentPath {component_path}.\""
  -> fixed text + an `entityId` (128) + a `workspaceId` (128) + a `componentPath` the model allows 2048 of.
- services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/batch_put_property_values.rs:314-319 does the same with `propertyName`, the component name (`Name`, 256) and the entity id.

TEST COVERAGE: tests/all/time_series_property_refusal.rs:304-319 pins the construction only for SHORT names (`expected_message(&property.name)` with the seeded names), and tests/all/component_path_resolution.rs:468-472 only counts the reported entries — so clamping an over-long message leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateWorkspace `w1`; CreateComponentType `com.example.gauge` in it with a non-time-series property definition whose name is 256 characters long (the `Name` maximum).
2. CreateEntity `e1` with a component `Gauge` of `com.example.gauge`.
3. POST /workspaces/w1/entity-properties with one entry whose `entityPropertyReference` is `{"entityId":"e1","componentName":"Gauge","propertyName":"<the 256-character name>"}` and one `propertyValues` element.
4. Observe HTTP 200 with `errorEntries[0].errors[0].errorMessage` of 297 characters, past the `@length(1..256)` the model declares for that member.
5. Repeat with `{"entityId":"e1","componentPath":"<2048 characters of [a-zA-Z_/0-9]>"}` — the reported `errorMessage` is over 2200 characters.
