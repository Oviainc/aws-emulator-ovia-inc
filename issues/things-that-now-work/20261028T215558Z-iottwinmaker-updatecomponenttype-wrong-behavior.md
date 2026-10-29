Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# Dropping a compositeComponentTypes declaration strands every entity composite component created under it: UpdateEntity can no longer update or delete it, although the delete-reference scan still counts it

_Filed: 2026-10-28T21:55:58Z_
_Operation: UpdateComponentType | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateComponentType` replaces `compositeComponentTypes` WHOLE when the request names the member, with no check against what is already standing on the declarations it drops. The stored composite components of entities that were created under a dropped declaration survive the patch and are still reported by `GetEntity`, `ListComponents` and `ListProperties` — but they become unreachable to the only writer that can remove them: `UpdateEntity`'s `compositeComponentUpdates` resolves the `componentPath` through the component types FIRST, before it ever looks at what the entity holds, so a `DELETE` (or an `UPDATE`) of the stranded composite component is answered HTTP 400 `ValidationException` "componentType <holder> declares no compositeComponentTypes.<segment>" forever. The only ways out are destroying the whole holding component or the whole entity.

This contradicts the stranding discipline the same handler states and implements for the other member of the same patch: the property-group check is run over the whole patched record precisely because "a patch that drops a definition a stored group still names is the call that strands that group". A patch that drops a `compositeComponentTypes` declaration is exactly the same call for the composite components that stand on it, and nothing checks it.

It also contradicts the reference rule the crate relies on in the other direction: `Workspace::component_type_references` deliberately counts an entity's composite component "whatever depth it sits at and whether or not the declaration that produced it is still on the holding component type", so `DeleteComponentType` keeps refusing to delete the component type the stranded composite component instantiates — the store is left holding a reference that can only be cleared by deleting the referencing entity.

Either side of the inconsistency is a fix (refuse the patch that strands, the way the property-group check does; or let `UpdateEntity` reach a composite component the entity actually stores, the way `ListComponents` and `component_at_path` already do), but as it stands a valid request the model allows permanently loses the ability to remove a stored record.

## Evidence

THE PATCH THAT DROPS THE DECLARATION, UNCHECKED — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/update_component_type.rs:340-342:
```
        if let Some(composite_component_types) = input.composite_component_types {
            updated.composite_component_types = composite_component_types.into_iter().collect();
        }
```
under src/ops/update_component_type.rs:312-319: "A member the request NAMES is replaced WHOLE, whatever its shape: ... the entries the request carries are the entries the component type ends up with and one it leaves out is dropped."

THE STRANDING RULE THE SAME HANDLER STATES — src/ops/update_component_type.rs:375-386:
```
        // The property groups, by contrast, are checked over the WHOLE patched
        // record whenever this request touches the set of properties they can
        // resolve against ... because a patch that drops
        // a definition a stored group still names is the call that strands that
        // group, exactly as a patch that clears `extendsFrom` is the call that
        // strands an inherited definition (the `dataType` rule below).
        if declares_property_groups || declares_property_definitions || declares_extends_from {
            if let Some(message) = crate::state::property_groups_violation(workspace, &updated) {
                return Err(validation_exception(message));
            }
        }
```
There is no equivalent guard for `declares_composite_component_types`: src/ops/update_component_type.rs:366-374 only re-resolves the ids the request still DECLARES (`composite_component_types_violation`), never what the dropped entries hold up.

WHY THE STRANDED RECORD BECOMES UNREMOVABLE — src/ops/update_entity.rs:487-494 resolves the path before anything else:
```
        let member = format!("{COMPOSITE_MEMBER}.{component_path}");
        let resolved = crate::state::resolve_composite_component_path(
            &entity.components,
            &workspace.component_types,
            COMPOSITE_MEMBER,
            component_path,
        )
        .map_err(validation)?;
```
and the resolution fails as soon as the declaration is gone — src/state.rs:3562-3571:
```
        let mut declarations = chain
            .iter()
            .filter_map(|parent| parent.composite_component_types.get(*segment))
            .peekable();
        if declarations.peek().is_none() {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} \
                 declares no compositeComponentTypes.{segment}"
            ));
        }
```
so the `Action::Delete` branch at src/ops/update_entity.rs:539-553 is never reached.

THE READERS THAT DO NOT NEED THE DECLARATION — src/ops/list_components.rs:420-439 reads the stored keys instead: "Nothing requires the intermediate steps of a composite component's path to have been created themselves, so the path names one when a composite component is stored AT it or anywhere BENEATH it."

THE REFERENCE RULE THAT KEEPS COUNTING IT — src/state.rs:1213-1221:
```
    /// * an entity's COMPOSITE component, through
    ///   [`CompositeComponentRecord::component_type_id`] ... so the type's definitions are equally what give its
    ///   properties their meaning, whatever depth it sits at and whether or not
    ///   the declaration that produced it is still on the holding component
    ///   type;
```
enforced by src/ops/delete_component_type.rs:233-250.

TEST COVERAGE — the existing suite covers exactly this state and silently avoids the broken removal path. tests/all/component_type_delete_composite_references.rs:142-163 is `drop_declarations` ("`UpdateComponentType` replaces the map WHOLE, so an empty map is what drops the declarations"), and its `Removal` enum (tests/all/component_type_delete_composite_references.rs:91-100) offers `CompositeComponent` ("`UpdateEntity` removing the composite component itself"), `Component` and `Entity` — yet every `dropped == true` row at tests/all/component_type_delete_composite_references.rs:195-221 is paired only with `Removal::Entity` or `Removal::Component`, never `Removal::CompositeComponent`, while the `CompositeComponent` removal appears only on the `dropped == false` row (tests/all/component_type_delete_composite_references.rs:188-193). No test asserts that a path whose declaration is gone is unresolvable for a DELETE; tests/all/composite_component_inherited_declaration.rs:370-412 only pins CREATE refusals for paths no declaration on the chain ever supplied.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.target", propertyDefinitions={"Pressure": {"dataType": {"type": "DOUBLE"}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.holder", compositeComponentTypes={"inner": {"componentTypeId": "com.example.target"}}).
4. CreateEntity(workspaceId="ws", entityId="holderEntity", entityName="holderEntity", components={"Sensor": {"componentTypeId": "com.example.holder"}}, compositeComponents={"Sensor/inner": {}}) -> 200.
5. UpdateComponentType(workspaceId="ws", componentTypeId="com.example.holder", compositeComponentTypes={}) -> 200: the declaration the stored composite component stands on is dropped with no refusal.
6. GetEntity(workspaceId="ws", entityId="holderEntity") -> 200 and still reports components.Sensor.compositeComponents.inner with componentPath "Sensor/inner" and componentTypeId "com.example.target".
7. UpdateEntity(workspaceId="ws", entityId="holderEntity", compositeComponentUpdates={"Sensor/inner": {"updateType": "DELETE"}}) -> HTTP 400 ValidationException "compositeComponentUpdates.Sensor/inner: componentType com.example.holder declares no compositeComponentTypes.inner". The same refusal answers updateType UPDATE, so the stored composite component can no longer be changed or removed.
8. DeleteComponentType(workspaceId="ws", componentTypeId="com.example.target") -> HTTP 400 ValidationException naming "entity holderEntity compositeComponent Sensor/inner" (src/state.rs:1245-1252), so the type stays undeletable too.
9. Compare the sibling member of the same patch: UpdateComponentType(workspaceId="ws", componentTypeId="com.example.target", propertyDefinitions={}) while a propertyGroup of com.example.target names "Pressure" is refused by property_groups_violation, because that patch "is the call that strands that group".
