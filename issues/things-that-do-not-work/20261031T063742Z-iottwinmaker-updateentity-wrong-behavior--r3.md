Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 17ac1af90a04a7606e0e89dfd0e6d0f65a50bfd0

# Re-pointing a component's componentTypeId strands the definitions the component already holds, so a read silently drops them

_Filed: 2026-10-31T06:37:42Z_
_Operation: UpdateEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `17ac1af90a04a7606e0e89dfd0e6d0f65a50bfd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEntity`'s `componentUpdates.<name>.componentTypeId` REPLACES the component type a stored component instantiates — which is the only place a component-local property definition without a `dataType` inherits one from. The handler then asks the crate's dataType rule only about the definitions THIS REQUEST carries (`apply_property_updates` walks `update.property_updates`), never about the definitions the component already holds. A component property whose stored declaration legitimately omits `dataType` because it refined one declared by the OLD component type is therefore left, after a 200, as a definition no read can report: `property_definition_response` returns `None` and logs a warning, so `GetEntity` and `ListProperties` silently drop the whole `definition` member of a property that previously reported one.

The crate states the rule the other way: `Workspace::component_property_definition_needs_data_type` is documented as the check "shared by every writer of a component's definition so a create and an update hold one definition to one rule", whose point is that "a writer refuses the request rather than storing a definition a reader would have to drop". And the sibling writer that faces the identical situation — a patch that removes the inheritance an older, already-stored declaration was relying on — does re-check the WHOLE patched record: `UpdateComponentType` loops over `&updated.property_definitions` with the comment "Every definition the patched record would hold has to be reportable, not just the ones this request carried". `UpdateEntity` is the one writer that checks only what the request carried, even though the member it patches (`componentTypeId`) is exactly the input of the rule.

This is distinct from the two already-filed UpdateEntity issues (the `propertyUpdates` value/dataType mismatch, and the fixed composite-path and RESET_VALUE issues): the stranded definitions here are ones the request never mentions, and the trigger is the `componentTypeId` patch itself.

## Evidence

THE RULE THE CRATE STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:1596-1619:
```
    /// A
    /// property nothing on that chain declares with a `dataType` — a component
    /// naming no component type, naming one this workspace does not hold, or
    /// declaring a property its chain never mentions — inherits none, so a
    /// writer refuses the request rather than storing a definition a reader
    /// would have to drop.
    ///
    /// The component-level counterpart of
    /// [`property_definition_needs_data_type`] ... shared by every writer of a
    /// component's definition so a create and an update hold one definition to
    /// one rule.
    pub fn component_property_definition_needs_data_type(
        &self,
        component_type_id: Option<&str>,
        property_name: &str,
        definition: &crate::ops::types::PropertyDefinitionRequest,
    ) -> bool {
```

THE HANDLER THAT PATCHES THE RULE'S INPUT AND CHECKS ONLY THE REQUEST — src/ops/update_entity.rs:470-486:
```
        if let Some(component_type_id) = update.component_type_id {
            component.component_type_id = Some(component_type_id);
        }
...
        // The component type the component instantiates AFTER this patch is what
        // the definitions it carries inherit from, so the `componentTypeId` the
        // entry just wrote is the one the property rule below resolves against.
        let component_type_id = component.component_type_id.clone();
        apply_property_updates(
            workspace,
            component_type_id.as_deref(),
            &mut component.properties,
            &member,
            update.property_updates,
        )?;
```
and the loop that limits the check to the request's own entries — src/ops/update_entity.rs:630 and :672-680:
```
    for (property_name, update) in crate::state::requested_in_key_order(updates) {
...
                if update.definition.as_ref().is_some_and(|definition| {
                    workspace.component_property_definition_needs_data_type(
                        component_type_id,
                        &property_name,
                        definition,
                    )
                }) {
                    return Err(validation(format!("{member}.definition.dataType is required")));
```
(`component.properties`, the definitions the component already holds, is never passed through that rule.)

THE SIBLING WRITER THAT RE-CHECKS THE WHOLE PATCHED RECORD — src/ops/update_component_type.rs:397-409:
```
        // Every definition the patched record would hold has to be reportable,
        // not just the ones this request carried: an update that clears
        // `extendsFrom` takes the inheritance an older definition was relying
        // on away with it.
        for (property_name, definition) in &updated.property_definitions {
            if crate::state::property_definition_needs_data_type(&updated.extends_from, definition)
            {
                return Err(validation_exception(format!(
                    "propertyDefinitions.{property_name}.dataType is required for a component \
                     type that extends nothing"
                )));
```

WHAT THE READ THEN DOES WITH THE STRANDED DEFINITION — src/state.rs:3284-3291:
```
    let Some(data_type) = definition.data_type.clone() else {
        // Logged rather than dropped silently so a regression in a writer that
        // is supposed to refuse a definition without a `dataType`, or a
        // resolution that failed to inherit one, is visible instead of quietly
        // costing the member.
        ::tracing::warn!("a resolved property definition carries no dataType; omitting it");
        return None;
```

TEST COVERAGE: the create-side acceptance of a declaration that omits `dataType` is pinned by tests/all/component_refined_definition.rs:6-14 ("the definition reported carries the inherited value of every member the component's declaration left alone"); the request-side refusal is pinned by tests/all/requested_map_refusal_order.rs:438 (`componentUpdates.{component}.propertyUpdates.{property}.definition.dataType`). No test re-points an existing component's `componentTypeId` to a type that does not declare its properties — tests/all/entity_update.rs:299 only restates the same `componentTypeId` the component already had — so adding the whole-record check leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateComponentType `com.example.gauge` in `w` with `propertyDefinitions: {"Reading": {"dataType": {"type": "DOUBLE"}}}`.
3. CreateComponentType `com.example.plain` in `w` with `propertyDefinitions: {"Other": {"dataType": {"type": "STRING"}}}` (it declares no `Reading`).
4. CreateEntity `e` in `w` with `components: {"Spec": {"componentTypeId": "com.example.gauge", "properties": {"Reading": {"definition": {"isTimeSeries": false}}}}}` — accepted, because the declaration REFINES the inherited one.
5. GET /workspaces/w/entities/e — `/components/Spec/properties/Reading/definition/dataType/type` is `DOUBLE`.
6. PUT /workspaces/w/entities/e with `{"componentUpdates": {"Spec": {"updateType": "UPDATE", "componentTypeId": "com.example.plain"}}}` — answered 200.
7. GET /workspaces/w/entities/e again: `Reading` is still reported as a property of `Spec` but now carries NO `definition` member at all (and the server logs "a resolved property definition carries no dataType; omitting it"). Expected: the update is refused by the rule src/state.rs:1596-1619 says every writer of a component's definition shares, exactly as UpdateComponentType refuses the analogous patch that takes away the inheritance an already-stored definition relied on.
