Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c7aba966c24a77c77027501c39437bde6fd5ec40

# A component type that extends anything may declare a brand-new property definition with no dataType, which the crate's own rule says must be refused and which every read then silently drops

_Filed: 2026-10-30T19:10:28Z_
_Operation: CreateComponentType | Class: validation_

> _Note: this issue was identified against the code base at commit `c7aba966c24a77c77027501c39437bde6fd5ec40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::property_definition_needs_data_type` is the crate's single statement of when a `PropertyDefinitionRequest` may omit `dataType`, and its own doc states the rule twice: "a definition may omit it only where it REFINES one it inherits", and "a writer refuses the request rather than storing a definition a reader would have to drop".

The body implements something much weaker: it asks only whether `extendsFrom` is EMPTY. So the moment a component type names any parent at all, every one of its own definitions is licensed to omit `dataType` — including a property name that no component type on the `extendsFrom` chain declares, i.e. one that refines nothing and inherits nothing.

The consequence is a stored definition no read can report. `property_definition_response` returns `None` for a resolved definition with no `dataType`, and logs a warning whose wording names exactly this situation as a writer regression. So `GetComponentType` accepts the create with HTTP 200, stores the property definition, and then omits that property entirely from `propertyDefinitions` on every subsequent read — as does every component reader that resolves through the same projection (`GetEntity`, `ListProperties`, `GetPropertyValue`) for a component instantiating the type. The property is in the store and invisible on the wire.

The crate already has the helper that answers the question the rule asks. `Workspace::inherited_property_names(extends_from)` returns exactly the property names the inheritance brings, together with an `inherits_unknown_definitions` flag for a chain that reaches a built-in the emulator does not model. The sibling rule `property_groups_violation` calls it for precisely this purpose (and uses the flag to stand down where the chain is unknowable); `property_definition_needs_data_type`, which needs the same answer, does not call it.

NOTE: this is a different gap from the already-fixed `CreateComponentType` validation issue about `extendsFrom`/`compositeComponentTypes` naming component types that do not exist. That one is about dangling references; this one is about the `dataType` licence, and the same helper is also the one `UpdateComponentType` applies over its whole patched record.

## Evidence

THE RULE THE CRATE STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:2551-2558:
```
/// `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on the
/// `PropertyDefinitionResponse` a read reports (see
/// [`property_definition_response`]), so a definition may omit it only where it
/// refines one it inherits. A component type that extends nothing inherits
/// nothing, so each of its own definitions must carry one, and a writer refuses
/// the request rather than storing a definition a reader would have to drop.
/// Shared by every writer of a definition so a create and an update hold one
/// definition to one rule.
```

THE CODE UNDER IT — src/state.rs:2559-2564:
```
pub fn property_definition_needs_data_type(
    extends_from: &[String],
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> bool {
    extends_from.is_empty() && definition.data_type.is_none()
}
```
"refines one it inherits" is never asked: the only thing tested is that the component type extends SOMETHING. A property name nothing on the chain declares is licensed exactly as a genuine refinement is.

THE EXISTING HELPER THAT ANSWERS THE QUESTION — src/state.rs:1440-1449:
```
    pub fn inherited_property_names(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (ancestors, holds_unstored) = self.ancestor_component_type_ids(extends_from);
        let mut names: BTreeSet<String> = BTreeSet::new();
        for ancestor in &ancestors {
            if let Some(stored) = self.component_types.get(ancestor) {
                names.extend(stored.property_definitions.keys().cloned());
            }
        }
        (names, holds_unstored)
    }
```

THE SIBLING RULE IN THE SAME FILE THAT DOES CALL IT — src/state.rs:3106-3111:
```
pub fn property_groups_violation(workspace: &Workspace, record: &ComponentType) -> Option<String> {
    let (inherited, inherits_unknown_definitions) =
        workspace.inherited_property_names(&record.extends_from);
    if inherits_unknown_definitions {
        return None;
    }
```

THE READER THAT DROPS WHAT THE WRITER STORED — src/state.rs:3245-3252:
```
    let Some(data_type) = definition.data_type.clone() else {
        // Logged rather than dropped silently so a regression in a writer that
        // is supposed to refuse a definition without a `dataType`, or a
        // resolution that failed to inherit one, is visible instead of quietly
        // costing the member.
        ::tracing::warn!("a resolved property definition carries no dataType; omitting it");
        return None;
    };
```
Its own doc, src/state.rs:3226-3231, says the case is unreachable: "`None` when even the resolved definition carries no `dataType` ... a writer refuses a definition without one wherever nothing is inherited, and a declaration that omits it takes its parent's, so what is left is a chain reaching a built-in of the real service this emulator does not model".

THE CALLERS — src/ops/create_component_type.rs:366-373:
```
        for (property_name, definition) in &property_definitions {
            if crate::state::property_definition_needs_data_type(&extends_from, definition) {
                return Err(validation_exception(format!(
                    "propertyDefinitions.{property_name}.dataType is required for a \
                     component type that extends nothing"
                )));
            }
        }
```
and src/ops/update_component_type.rs:401-409, which applies the same predicate over the whole patched record.

WHERE THE OMISSION SURFACES — src/ops/get_component_type.rs:396-411: `property_definitions_response` is a `filter_map` over `property_definition_response(...)`, so the `None` above costs the whole map entry, and src/ops/get_component_type.rs:260 reports that map as `propertyDefinitions`.

TEST COVERAGE (enforcing the stated rule leaves them passing): tests/all/component_type_inherited_schema.rs:116-129 is the only row that omits a `dataType`, and it is a genuine refinement — "Temperature carries NO dataType: it refines the definition the chain already gives it. Humidity is the child's own." — with `Temperature` declared by `BASE` (:85-89) and reached through `MIDDLE` (:109-114). The row whose parent is unmodelled, :132-138, declares `"Own": {"dataType": {"type": "STRING"}}`, so the `inherits_unknown_definitions` escape is not needed to keep it green. tests/all/component_type_lifecycle.rs:291-343 reads every created definition's `dataType` back, and every definition it creates carries one.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.base", propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.child", extendsFrom=["com.example.base"], propertyDefinitions={"Humidity": {"isTimeSeries": true}}) — `Humidity` is declared nowhere on the chain and carries no dataType.
4. Observe HTTP 200: the definition is accepted and stored, although `property_definition_needs_data_type` states a writer must refuse it.
5. Wait for the component type to settle, then GetComponentType(workspaceId="ws", componentTypeId="com.example.child").
6. Observe that `propertyDefinitions` reports only `Temperature`: `Humidity` is missing from the response entirely, with a `a resolved property definition carries no dataType; omitting it` warning in the log.
7. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"C": {"componentTypeId": "com.example.child"}}) then GetEntity / ListProperties: the same stored property is absent from the component's reported properties.
