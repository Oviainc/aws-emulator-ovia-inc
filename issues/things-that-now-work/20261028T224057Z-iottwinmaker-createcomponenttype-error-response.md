Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 67d3ef025eb7c6b921e8b895d06c7f32bf688664

# CreateComponentType's dataType refusal picks an arbitrary offender: the propertyDefinitions pass walks the request's HashMap, where the sibling UpdateComponentType walks a sorted map

_Filed: 2026-10-28T22:40:56Z_
_Operation: CreateComponentType | Class: error-response_

> _Note: this issue was identified against the code base at commit `67d3ef025eb7c6b921e8b895d06c7f32bf688664`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateComponentType` checks the "a definition may omit `dataType` only where it refines an inherited one" rule by iterating the request's deserialized `propertyDefinitions` map and returning on the FIRST offender it happens to meet. That map is a `std::collections::HashMap`, whose iteration order is not key order and is randomised per process, so a create that carries two (or more) definitions omitting `dataType` is refused about whichever one the hash order reached first — a different property from one emulator process to the next, and not necessarily the first in name order.

This is the one unordered refusal left in this handler, and the crate states the opposite rule in three places, one of them the sibling writer of the very same member:

* `UpdateComponentType` runs the IDENTICAL `property_definition_needs_data_type` pass over `updated.property_definitions`, which is a `BTreeMap` (the request's map is collected into it before the check), so the update path's refusal always names the first offender in key order;
* inside this very handler, the other reference-carrying map is deliberately normalised first: `compositeComponentTypes` is collected into a `BTreeMap` before `composite_component_types_violation` walks it, and `propertyGroups` likewise goes through the record's `BTreeMap`;
* `UpdateEntity` routes every request map through a `sorted()` helper whose doc-comment states the rule verbatim, and the shared reserved-tag-key rule in `state.rs` sorts its offenders for the same stated reason.

So one CreateComponentType request can be answered with two different `ValidationException` messages on two different runs, while every neighbouring refusal of the same crate is deterministic.

## Evidence

THE UNORDERED PASS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_component_type.rs:325 and :360-372:
```
        let property_definitions = input.property_definitions.clone().unwrap_or_default();
...
        // `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on
        // the `PropertyDefinitionResponse` a read reports, so a definition may
        // omit it only where it refines one it inherits — the rule
        // `crate::state::property_definition_needs_data_type` holds for every
        // writer of a definition.
        for (property_name, definition) in &property_definitions {
            if crate::state::property_definition_needs_data_type(&extends_from, definition) {
                return Err(validation_exception(format!(
                    "propertyDefinitions.{property_name}.dataType is required for a \
                     component type that extends nothing"
                )));
            }
        }
```
the map it walks is the request's HASH map — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/types/create_component_type_types.rs:21:
```
    pub property_definitions: Option<std::collections::HashMap<String, PropertyDefinitionRequest>>,
```

THE DETERMINISTIC SIBLING, SAME RULE, SAME MEMBER — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/update_component_type.rs:323-324 and :392-400:
```
        if let Some(property_definitions) = input.property_definitions {
            updated.property_definitions = property_definitions.into_iter().collect();
        }
...
        for (property_name, definition) in &updated.property_definitions {
            if crate::state::property_definition_needs_data_type(&updated.extends_from, definition)
            {
                return Err(validation_exception(format!(
                    "propertyDefinitions.{property_name}.dataType is required for a component \
                     type that extends nothing"
                )));
```
(`updated.property_definitions` is `BTreeMap<String, PropertyDefinitionRequest>` — src/state.rs:2201.)

THE SAME HANDLER NORMALISING ITS OTHER MAP FIRST — src/ops/create_component_type.rs:326-334:
```
        let composite_component_types: ::std::collections::BTreeMap<
            String,
            CompositeComponentTypeRequest,
        > = input
            .composite_component_types
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
```

THE RULE THE CRATE STATES — src/ops/update_entity.rs:355-361:
```
/// A requested map in KEY order, so which entry a refusal names does not depend
/// on the request map's iteration order.
fn sorted<T>(
    requested: Option<::std::collections::HashMap<String, T>>,
) -> ::std::collections::BTreeMap<String, T> {
    requested.unwrap_or_default().into_iter().collect()
}
```
and src/state.rs:349-351:
```
/// The offending keys
/// are reported in key order, so the refusal does not depend on the iteration
/// order of the map the request was deserialized into.
```

TEST COVERAGE: the one row that reaches this refusal sends exactly ONE offending definition, so it cannot observe the nondeterminism and a sorted walk leaves it passing — tests/all/component_type_lifecycle.rs:727-742:
```
    let definitions = json!({"propertyDefinitions": {"untyped": {"isTimeSeries": true}}});
    let refused = emulator
        .create_component_type("refusalWorkspace", "com.example.untyped", definitions.clone())
        .await;
...
            .is_some_and(|message| message.contains("untyped")),
```
and the only other dataType-less create rows each carry a single property too (tests/all/component_type_create_references.rs:151, :363). No test sends two offenders in one CreateComponentType call.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.t", body={"propertyDefinitions": {"alpha": {"isRequiredInEntity": true}, "zulu": {"isTimeSeries": true}}}) — two definitions, neither carrying dataType, and no extendsFrom.
3. Observe HTTP 400 ValidationException whose message names either "propertyDefinitions.alpha.dataType is required for a component type that extends nothing" or "propertyDefinitions.zulu.dataType is required..." — whichever the randomised HashMap order reached first, so repeating the identical request in a fresh emulator process can report the other one.
4. Compare the deterministic sibling: send the same two offending definitions through UpdateComponentType (after creating the type with dataTypes and then patching propertyDefinitions to drop them) and the refusal always names the first in key order (src/ops/update_component_type.rs:392-400); send two unresolvable compositeComponentTypes ids to CreateComponentType and the refusal is decided over a BTreeMap, so it always names the same one (src/ops/create_component_type.rs:326-334, 353-359).
