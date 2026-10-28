Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# A composite component path through an INHERITED compositeComponentTypes declaration never resolves, although GetComponentType reports that declaration as one the component type effectively has

_Filed: 2026-10-28T17:34:58Z_
_Operation: UpdateEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resolve_composite_component_path` — the one path rule `CreateEntity` and `UpdateEntity` share — resolves each segment of a `componentPath` against the component type's OWN `composite_component_types` map only (`parent.composite_component_types.get(*segment)`). It never walks the type's `extendsFrom` chain.

So for a component type CHILD that extends BASE, where BASE declares `compositeComponentTypes.inner`, `UpdateEntity` with `compositeComponentUpdates = {"Comp/inner": {"updateType": "CREATE"}}` on a component of type CHILD is refused HTTP 400 `ValidationException` "componentType CHILD declares no compositeComponentTypes.inner" — and `CreateEntity` with `compositeComponents = {"Comp/inner": {}}` is refused the same way, since both go through this helper.

That contradicts what the crate reports for CHILD. `GetComponentType`'s `composite_component_types_response` unions the declarations of the type AND of its `extendsFrom` chain and marks the inherited ones `isInherited: true`, so the very entry whose path cannot be resolved is published as part of CHILD's effective schema. The crate's two sibling resolutions over the same inheritance chain — `Workspace::effective_property_definition` and `Workspace::effective_property_group` — both walk `component_type_chain`; only the composite-path resolution does not.

The justification the helper's own doc gives for looking at the own map only does not cover this case: it is about "a path through a component type this workspace does not hold", i.e. a built-in the emulator does not model. Here the parent IS held and its declaration IS reported.

## Evidence

THE WRITER THAT RESOLVES ONLY THE OWN MAP — src/state.rs:3522-3534:
```
    for segment in &segments[1..] {
        let Some(parent) = component_types.get(&component_type_id) else {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} does \
                 not exist in this workspace, so {segment} cannot be resolved against it"
            ));
        };
        let Some(declaration) = parent.composite_component_types.get(*segment) else {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} \
                 declares no compositeComponentTypes.{segment}"
            ));
        };
```
and the reason its doc gives, src/state.rs:3487-3493:
```
    /// Two limits are deliberate, both inherited from how this service stores
    /// component types. A path of a single segment names a top-level component and
    /// no composite component, so it does not resolve; and resolution walks only
    /// the component type's OWN `compositeComponentTypes`, because this service
    /// does not model every built-in component type a real workspace carries — so a
    /// path through a component type this workspace does not hold cannot be
    /// resolved, and is refused rather than guessed at.
```
(the stated reason is an UNHELD parent; an inherited declaration of a HELD parent is not that case)

THE READER THAT PUBLISHES THE INHERITED DECLARATION — src/ops/get_component_type.rs:455-477:
```
/// The component type's EFFECTIVE composite component types, resolved the same
/// way its property definitions are: the id reported is the one the nearest
/// declaration that names any refers to, and the entry is marked inherited when
/// the `extendsFrom` chain declares it.
fn composite_component_types_response(
    component_type: &crate::state::ComponentType,
    inherited: &[&crate::state::ComponentType],
) -> ::std::collections::HashMap<String, CompositeComponentTypeResponse> {
    let keyed = composite_component_type_map;
    declared_names(component_type, inherited, keyed)
```
with `declared_names`, src/ops/get_component_type.rs:343-352:
```
    let mut names: ::std::collections::BTreeSet<&String> = keyed(component_type).keys().collect();
    for ancestor in inherited {
        names.extend(keyed(ancestor).keys());
    }
```

THE TWO SIBLING RESOLUTIONS THAT DO WALK THE CHAIN — src/state.rs:1496-1503 (`effective_property_definition`):
```
        let chain = component_type_id
            .map(|component_type_id| self.component_type_chain(component_type_id))
            .unwrap_or_default();
        refined_property_definition(own.into_iter().chain(
            chain
                .iter()
                .filter_map(|component_type| component_type.property_definitions.get(property_name)),
        ))
```
and src/state.rs:1541-1548 (`effective_property_group`), identical shape over `property_groups`.

THE CALLERS AFFECTED — src/ops/update_entity.rs:488-494:
```
    let resolved = crate::state::resolve_composite_component_path(
        &entity.components,
        &workspace.component_types,
        COMPOSITE_MEMBER,
        component_path,
    )
    .map_err(validation)?;
```
and src/ops/create_entity.rs:372-383, the same call under `compositeComponents`.

EXISTING TEST THAT PINS THE READER'S SIDE — tests/all/component_type_inherited_schema.rs:221-232, where MIDDLE extends BASE and BASE declares the composite entry (tests/all/component_type_inherited_schema.rs:105 `"compositeComponentTypes": {"inner": {"componentTypeId": LEAF}},`):
```
        Case {
            what: "oneHopInheritsEveryPartOfTheParentsSchema",
            component_type_id: MIDDLE,
            ...
            composites: vec![("inner", true, LEAF)],
        },
```

TEST COVERAGE OF THE WRITER: no test stands a composite component up through an inherited declaration — `grep -n 'extendsFrom|inherit' tests/all/entity_composite_components.rs` returns "No matches", and every composite path in tests/all/entity_update.rs:49-110 and tests/all/component_type_delete_composite_references.rs:119-123 resolves through the component type's OWN `compositeComponentTypes`. Walking the chain therefore leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.leaf").
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.base", compositeComponentTypes={"inner": {"componentTypeId": "com.example.leaf"}}).
4. CreateComponentType(workspaceId="ws", componentTypeId="com.example.child", extendsFrom=["com.example.base"]).
5. GetComponentType(workspaceId="ws", componentTypeId="com.example.child") -> 200 with compositeComponentTypes = {"inner": {"componentTypeId": "com.example.leaf", "isInherited": true}}.
6. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Comp": {"componentTypeId": "com.example.child"}}).
7. UpdateEntity(workspaceId="ws", entityId="e1", compositeComponentUpdates={"Comp/inner": {"updateType": "CREATE"}}) -> HTTP 400 ValidationException "compositeComponentUpdates.Comp/inner: componentType com.example.child declares no compositeComponentTypes.inner", and GetEntity shows no composite component was created.
8. Repeat with CreateEntity(..., components={"Comp": {"componentTypeId": "com.example.child"}}, compositeComponents={"Comp/inner": {}}) -> the same refusal from the shared helper.
9. Compare the same entity against a component of type com.example.base (the declarer): the identical path resolves and the composite component is stored.
