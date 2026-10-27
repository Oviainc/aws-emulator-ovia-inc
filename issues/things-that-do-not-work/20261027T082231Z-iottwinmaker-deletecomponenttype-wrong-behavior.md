Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# DeleteComponentType's reference scan ignores an entity's composite components, so a component type they instantiate can be deleted

_Filed: 2026-10-27T08:22:31Z_
_Operation: DeleteComponentType | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteComponentType` refuses a delete while anything stored still references the component type, and the handler states that rule as an invariant over the whole store. The scan it uses (`Workspace::component_type_references`) walks only an entity's TOP-LEVEL components (`entity.components.values()` and `component.component_type_id`). It never looks at `ComponentRecord::composite_components`, each of which stores a required `component_type_id` of its own and is treated by every other part of this crate as an instance of that component type (the create applies the abstract-type rule to it, and `GetEntity`/`ListComponents`/`ListProperties` resolve its `definedIn`, inherited property definitions and property groups against it).

So a component type an entity's composite component instantiates is not counted as a reference. Once the declaration that produced the composite component is gone from the holding component type (an `UpdateComponentType` replaces `compositeComponentTypes` WHOLE, by design in this crate), nothing counts the type at all and the delete succeeds — leaving a stored composite component naming a component type the workspace no longer holds. That is exactly the state the handler's comment says can never arise, and the consequence is silent read loss: the composite component's inherited property definitions, `definedIn` and inherited property groups disappear from `GetEntity`, `ListComponents` and `ListProperties`, because the record they were resolved from is gone.

## Evidence

The rule the handler states, and the scan it relies on:

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/delete_component_type.rs:232-236`
```
        // What is built on the component type is deleted before the component
        // type itself, so nothing stored is left instantiating or extending a
        // type the store no longer holds.
        let references = workspace.component_type_references(&input.component_type_id);
        if !references.is_empty() {
```

The scan, which enumerates what counts as a stored reference and then walks only top-level components:

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:1187-1216`
```
    /// A stored reference to a component type is one of:
    ///
    /// * an entity's component, through `ComponentRequest::componentTypeId` —
    ///   the component IS an instance of the type, so the type's definitions are
    ///   what give the component's properties their meaning;
...
    pub fn component_type_references(&self, component_type_id: &str) -> Vec<String> {
        let mut references = Vec::new();
        for entity in self.entities.values() {
            for component in entity.components.values() {
                if component.component_type_id.as_deref() == Some(component_type_id) {
```
(no arm reads `component.composite_components`, whose records each carry a `component_type_id`).

The same crate insists a composite component IS a component of the entity and instantiates its own type:

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_entities.rs:315-317`
```
/// A composite component is a component of the entity too — `GetEntity` reports
/// it as one — and it carries a `componentTypeId` and properties of its own, so
/// the external id it names is resolved the same way the holding component's
```

`services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_entity.rs:384-391`
```
            // A composite component instantiates the component type the
            // declaration its path resolved to names, so the same abstractness
            // rule holds for it as for a top-level component.
            if let Some(refusal) =
                abstract_component_type_refusal(workspace, Some(&resolved.component_type_id))
```

Test coverage: `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/tests/all/component_type_delete.rs:39-70` enumerates the reference kinds the suite generalises over — `EntityComponent`, `ExtendsFrom`, `CompositeComponentType` (another component TYPE's `compositeComponentTypes`), `EntityComponentAndExtendsFrom`, `RelationshipTarget`. A composite component OF AN ENTITY is not among them, so no existing test pins the current behaviour.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateComponentType `com.example.inner` in `w`.
3. CreateComponentType `com.example.outer` in `w` with `compositeComponentTypes: {"inner": {"componentTypeId": "com.example.inner"}}`.
4. CreateEntity in `w` with `components: {"Sensor": {"componentTypeId": "com.example.outer"}}` and `compositeComponents: {"Sensor/inner": {}}` — the stored `CompositeComponentRecord` now carries `component_type_id = com.example.inner`.
5. UpdateComponentType `com.example.outer` with `compositeComponentTypes: {}` (the map member is replaced whole, dropping the declaration).
6. DeleteComponentType `com.example.inner` — it is accepted (200, state DELETING) even though the entity's composite component still instantiates it; `component_type_references` returned nothing because it never looked at `composite_components`.
7. GetEntity the entity: the composite component still reports `componentTypeId: com.example.inner`, but its inherited property definitions, `definedIn` and inherited property groups are gone — the dangling reference the handler's comment says cannot exist.
