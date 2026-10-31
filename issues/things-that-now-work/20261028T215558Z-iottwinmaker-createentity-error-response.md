Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 47570b2a3c1f0eaf73d0ddb55e43ce0698703588

# The dataType refusal picks an arbitrary offender: the property-definition pass walks the request's HashMaps, so two identical CreateEntity calls can be refused about different components and properties

_Filed: 2026-10-28T21:55:58Z_
_Operation: CreateEntity | Class: error-response_

> _Note: this issue was identified against the code base at commit `47570b2a3c1f0eaf73d0ddb55e43ce0698703588`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEntity`'s `dataType` pass iterates the request's deserialized `HashMap`s and returns on the FIRST offender it happens to meet: the component loop at src/ops/create_entity.rs:331-336 walks `components` (a `HashMap<String, ComponentRequest>`), the composite loop at src/ops/create_entity.rs:337-342 walks `composite_components` (also still a `HashMap` at that point), and `require_data_types` itself walks `properties.into_iter().flatten()` over a `HashMap<String, PropertyRequest>`. Rust's `HashMap` iteration order is not the key order and is randomised per process, so a request that carries two components (or two properties) whose definitions omit `dataType` is refused about whichever one the hash order reached first — a different one from one emulator process to the next, and not necessarily the first in key order.

This is the exact nondeterminism the crate states it must not have, in three places, two of them inside this very handler: the abstract-component-type walk 10 lines below is run over the already-sorted `BTreeMap` "so which component a refusal names does not depend on the request map's iteration order", the composite-path walk sorts the same `composite_components` map by key before resolving it, `UpdateEntity` routes every request map through a `sorted()` helper carrying the same sentence, and the shared tag-key rule in `state.rs` sorts its offenders "so the refusal does not depend on the iteration order of the map the request was deserialized into". Only the `dataType` pass skips it, so this one refusal is the one place a caller cannot predict which member the service will name.

## Evidence

THE UNORDERED PASS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_entity.rs:325-342:
```
        let components = input.components.unwrap_or_default();
        let composite_components = input.composite_components.unwrap_or_default();
        // Every property a `CreateEntity` declares is a NEW property, so its
        // definition must carry the `dataType` the modelled
        // `PropertyDefinitionResponse` requires back — in a component and in a
        // composite component alike.
        for (component_name, component) in &components {
            require_data_types(
                &format!("components.{component_name}"),
                component.properties.as_ref(),
            )?;
        }
        for (component_path, composite) in &composite_components {
            require_data_types(
                &format!("compositeComponents.{component_path}"),
                composite.properties.as_ref(),
            )?;
        }
```
(`components` / `composite_components` are the request's `std::collections::HashMap`s — `components_from_request(components: ::std::collections::HashMap<String, ComponentRequest>)`, src/ops/create_entity.rs:491-493.)

and the inner walk, src/ops/create_entity.rs:586-604:
```
fn require_data_types(
    member_path: &str,
    properties: Option<&::std::collections::HashMap<String, PropertyRequest>>,
) -> Result<(), CreateEntityErrorModeled> {
    for (property_name, property) in properties.into_iter().flatten() {
        if property
            .definition
            .as_ref()
            .is_some_and(|definition| definition.data_type.is_none())
        {
            return Err(CreateEntityErrorModeled::ValidationException(
```

THE RULE THE SAME HANDLER STATES, TEN LINES BELOW — src/ops/create_entity.rs:346-360:
```
        // A component INSTANTIATES the component type its `componentTypeId`
        // names ... Walked in
        // component-name order (the records are keyed by it), so which
        // component a refusal names does not depend on the request map's
        // iteration order, and walked BEFORE anything is stored.
        for component in component_records.values() {
```
and, for the very map the second loop above walked unsorted, src/ops/create_entity.rs:362-371:
```
        // ... Walked in path
        // order, so which unresolvable path a refusal names does not depend on
        // the request map's iteration order.
        let mut composite_components: Vec<(String, CompositeComponentRequest)> =
            composite_components.into_iter().collect();
        composite_components.sort_by(|(left, _), (right, _)| left.cmp(right));
```

THE SIBLING WRITER THAT APPLIES IT TO EVERY MAP — src/ops/update_entity.rs:355-361:
```
/// A requested map in KEY order, so which entry a refusal names does not depend
/// on the request map's iteration order.
fn sorted<T>(
    requested: Option<::std::collections::HashMap<String, T>>,
) -> ::std::collections::BTreeMap<String, T> {
    requested.unwrap_or_default().into_iter().collect()
}
```
used for `componentUpdates`, `compositeComponentUpdates`, `propertyUpdates` and `propertyGroupUpdates` (src/ops/update_entity.rs:290-295, 589, 659) — so the equivalent `dataType` refusal on the update path IS deterministic (src/ops/update_entity.rs:624-630).

THE SHARED RULE IN state.rs — src/state.rs:349-351 and 361-369:
```
/// The offending keys
/// are reported in key order, so the refusal does not depend on the iteration
/// order of the map the request was deserialized into.
...
    reserved.sort_unstable();
    reserved.dedup();
```

TEST COVERAGE: the two rows that reach this refusal each send exactly ONE offending property, so neither can observe the nondeterminism and a sorted walk leaves both passing — tests/all/workspace_entity_lifecycle.rs:1380-1396 (one property "p") and tests/all/entity_composite_components.rs:399-406 (one property "p"). No test sends two offenders in one request.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.t", propertyDefinitions={}).
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Alpha": {"componentTypeId": "com.example.t", "properties": {"p": {"definition": {"isRequiredInEntity": true}}}}, "Zulu": {"componentTypeId": "com.example.t", "properties": {"q": {"definition": {"isRequiredInEntity": true}}}}}).
4. Observe HTTP 400 ValidationException whose message names either "components.Alpha.properties.p.definition.dataType is required" or "components.Zulu.properties.q.definition.dataType is required" — whichever the HashMap's randomised order reached first, so repeating the identical request in a fresh emulator process can report the other one.
5. Compare the deterministic sibling refusals for the same request shape: add an abstract componentTypeId to both components and the refusal always names the component first in NAME order (src/ops/create_entity.rs:350-360); send two unresolvable compositeComponents paths and the refusal always names the first in PATH order (src/ops/create_entity.rs:366-371); send the same two offending definitions through UpdateEntity's componentUpdates and the refusal always names the first in key order (src/ops/update_entity.rs:290-295 with the `sorted` helper at src/ops/update_entity.rs:355-361).
