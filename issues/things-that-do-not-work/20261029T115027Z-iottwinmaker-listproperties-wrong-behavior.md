Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d626157daccf58583369e714487cbead343a069c

# A component property definition that REFINES the one its component type declares is reported isInherited=false, although the crate folds the inherited members into it and its own sibling reader answers the same question the opposite way

_Filed: 2026-10-29T11:50:27Z_
_Operation: ListProperties | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d626157daccf58583369e714487cbead343a069c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PropertyDefinitionResponse.isInherited` is decided twice in this crate, by two rules that disagree for the one case that matters — a declaration that REFINES an inherited one instead of replacing it.

The crate's resolver explicitly supports refinement: `Workspace::effective_property_definition` folds the component's own declaration over the component type's chain rather than taking the nearest one whole, so a component that restates only some members of a property definition still reports the INHERITED values of every member it left alone (`displayName`, `defaultValue`, `isExternalId`, `configuration`, ...).

But the flag that tells a caller where that definition came from is computed by `crate::state::property_definition_is_inherited`, which is a purely LOCAL test: it returns `false` the moment the component's own record carries any definition for the property, refining or not. So the response a caller gets for a refining component carries member values it never declared and claims `isInherited: false`.

The sibling reader of the very same model member answers the same question the other way: `get_component_type.rs::is_inherited` is documented as "some component type on the `extendsFrom` chain declares it, so the entry reaches this component type through its inheritance whether or not the type also refines it" — true for a refined entry. A component type that refines an inherited definition therefore reports `isInherited: true`, while a component that refines an inherited definition reports `isInherited: false`, out of the same crate and for the same flag.

`property_definition_is_inherited`'s own doc states the premise the refinement support breaks: "A component's own record carries a definition only for a property its entity's create or update DECLARED inline; every other property of the component reaches it through the component type, which is exactly what `isInherited` reports." With refinement a declaration is BOTH declared inline and reaching through the type, so that dichotomy no longer holds and the flag is decided on the wrong side of it.

The same local test backs the component-side property-GROUP flag (`property_group_is_inherited`), which has the identical split against `get_component_type.rs::property_group_response`.

This reaches the caller through `ListProperties` (`property_summary`), `GetEntity` (`property_response`) and `ListComponents` (property groups) — one shared helper, so all three report the same wrong flag.

## Evidence

THE FOLD THAT MAKES A COMPONENT DECLARATION A REFINEMENT — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:1455-1488:
```
    /// The definition that gives one property of a component its meaning,
    /// RESOLVED over every declaration that reaches the property: the
    /// component's own declaration where it has one, then the declarations of
    /// the component type it instantiates and of the types that type extends,
    /// nearest first.
    ///
    /// ... A nearer declaration may also REFINE a further one
    /// rather than replace it (every member of `PropertyDefinitionRequest` is
    /// optional, so a declaration can restate only the members it changes),
    /// which is why the whole set of declarations is folded
    /// ([`refined_property_definition`]) instead of the nearest one being taken
    /// whole. `None` when no declaration reaches the property at all.
    pub fn effective_property_definition(
        &self,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
        property_name: &str,
    ) -> Option<crate::ops::types::PropertyDefinitionRequest> {
        let own = properties
            .get(property_name)
            .and_then(|property| property.definition.as_ref());
        ...
        refined_property_definition(own.into_iter().chain(
```

THE LOCAL TEST THAT DECIDES THE FLAG — src/state.rs:2382-2399:
```
/// Whether the definition [`Workspace::effective_property_definition`] resolves
/// for `property_name` is INHERITED: it came from the component type the
/// component instantiates rather than from the component itself.
///
/// A component's own record carries a definition only for a property its
/// entity's create or update DECLARED inline; every other property of the
/// component reaches it through the component type, which is exactly what
/// `isInherited` reports. ...
pub fn property_definition_is_inherited(
    properties: &BTreeMap<String, PropertyRecord>,
    property_name: &str,
) -> bool {
    properties
        .get(property_name)
        .is_none_or(|property| property.definition.is_none())
}
```

THE SIBLING READER OF THE SAME FLAG, ANSWERING THE OPPOSITE WAY — src/ops/get_component_type.rs:372-383:
```
/// Whether the entry reported under `name` is INHERITED: some component type on
/// the `extendsFrom` chain declares it, so the entry reaches this component type
/// through its inheritance whether or not the type also refines it.
fn is_inherited<'a, Entry: 'a>(
    inherited: &[&'a crate::state::ComponentType],
    keyed: impl Fn(&'a crate::state::ComponentType) -> SchemaMap<'a, Entry>,
    name: &str,
) -> bool {
    inherited
        .iter()
        .any(|ancestor| keyed(ancestor).contains_key(name))
}
```

WHERE THE WRONG FLAG IS SERVED — src/ops/list_properties.rs:393-411:
```
    let definition = workspace.effective_property_definition(
        component.component_type_id,
        component.properties,
        &property_name,
    );
    ...
        definition: definition.as_ref().and_then(|definition| {
            crate::state::property_definition_response(
                definition,
                crate::state::property_definition_is_inherited(
                    component.properties,
                    &property_name,
                ),
            )
        }),
```
and identically in src/ops/get_entity.rs:415-425:
```
    let definition =
        workspace.effective_property_definition(component_type_id, properties, property_name);
    ...
        definition: definition.as_ref().and_then(|definition| {
            crate::state::property_definition_response(
                definition,
                crate::state::property_definition_is_inherited(properties, property_name),
            )
        }),
```
The group flag has the same split — src/state.rs:2405-2410:
```
pub fn property_group_is_inherited(
    property_groups: &BTreeMap<String, PropertyGroupRecord>,
    group_name: &str,
) -> bool {
    !property_groups.contains_key(group_name)
}
```

TEST COVERAGE: tests/all/component_type_inherited_schema.rs pins `isInherited` for a component type's refined definitions (the chain-declares rule); tests/all/component_inherited_schema.rs and tests/all/property_listing.rs cover a component that declares NOTHING (isInherited true) and one that declares a WHOLE definition, so no existing case exercises a component declaration that refines an inherited one, and nothing pins the flag for it.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.gauge", propertyDefinitions={"Reading": {"dataType": {"type": "DOUBLE"}, "displayName": "Reading (bar)", "defaultValue": {"doubleValue": 1.0}}}).
3. CreateComponentType(workspaceId="ws", componentTypeId="com.example.tuned", extendsFrom=["com.example.gauge"], propertyDefinitions={"Reading": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}}) — a REFINING declaration on a component type.
4. GetComponentType(workspaceId="ws", componentTypeId="com.example.tuned") -> propertyDefinitions.Reading.isInherited = true, with displayName "Reading (bar)" folded in from the parent.
5. CreateEntity(workspaceId="ws", entityId="unit-1", components={"Gauge": {"componentTypeId": "com.example.gauge", "properties": {"Reading": {"definition": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}}}}}) — the same REFINING declaration on a component.
6. ListProperties(workspaceId="ws", entityId="unit-1", componentName="Gauge") -> propertySummaries[0].definition carries displayName "Reading (bar)" and defaultValue 1.0 folded in from the component type, yet reports isInherited = false.
7. GetEntity(workspaceId="ws", entityId="unit-1") reports the same definition with the same isInherited = false.
