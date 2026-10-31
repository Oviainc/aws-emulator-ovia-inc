Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# A propertyUpdates entry writes a value whose type contradicts the property's declared dataType, skipping the data_value_type_violation rule the crate states for every value written to a property

_Filed: 2026-10-29T02:01:20Z_
_Operation: UpdateEntity | Class: validation_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEntity`'s `apply_property_updates` is the second writer of an entity property's `value`, and it carries the same omission as `CreateEntity`: on the `CREATE` / `UPDATE` / no-updateType arm it checks only that a definition the entry carries has a `dataType` present, then assigns `property.value = Some(value)` with no comparison against that type.

`crate::state::data_value_type_violation` is the crate's single statement of this rule, and its doc gives the reason in terms of the read it would corrupt. The handler writes into the very `PropertyRecord`s `BatchPutPropertyValues` refuses mistyped values for, and into the records `GetEntity` / `ListProperties` / `GetPropertyValue` read back verbatim through `latest_property_value`.

This arm is also reachable without any definition at all: an `UPDATE` entry that names only `value` leaves the stored (or inherited) definition in place, so `propertyUpdates.Temperature = {"value": {"booleanValue": true}}` on a property the component type declares `DOUBLE` replaces a correctly typed value with a wrongly typed one, and every later read reports it.

## Evidence

THE RULE — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:3191-3196:
```
/// A property's `dataType` is "an object that specifies the data type of a
/// property", so a value written to the property carries a value of THAT type:
/// one carrying another type, or no value at all, is refused rather than
/// stored, which would leave a read reporting a value of a type the definition
/// does not declare.
```

THE WRITER THAT SKIPS IT — src/ops/update_entity.rs:616-638:
```
            // `CREATE` and `UPDATE` both describe the property the entry
            // defines, so both write the definition and the value it carries;
            // an entry that names no update type does the same.
            None | Some(UPDATE_TYPE_CREATE) | Some(UPDATE_TYPE_UPDATE) => {
                // A definition the request carries REPLACES the property's
                // definition, so it must be one a read can report: every
                // reported `PropertyDefinitionResponse` requires a `dataType`,
                // which is the rule `CreateEntity` applies to the definitions
                // it stores.
                if update
                    .definition
                    .as_ref()
                    .is_some_and(|definition| definition.data_type.is_none())
                {
                    return Err(validation(format!("{member}.definition.dataType is required")));
                }
                let property = properties.entry(property_name).or_default();
                if let Some(definition) = update.definition {
                    property.definition = Some(definition);
                }
                if let Some(value) = update.value {
                    property.value = Some(value);
                }
```
The only check is on the definition's own `dataType` being present ("it must be one a read can report"); the value assigned two lines later is never measured against it.

THE SIBLING WRITER OF THE SAME RECORDS THAT DOES APPLY THE RULE — src/ops/batch_put_property_values.rs:333-338:
```
        if let Some(violation) =
            crate::state::data_value_type_violation(property_name, definition.as_ref(), &value.value)
        {
            return Err(invalid_entry(violation));
        }
```

HOW IT SURFACES — src/state.rs:3094-3096 returns the stored `value` unchanged, and src/ops/get_entity.rs:415-429 reports it next to the resolved `PropertyDefinitionResponse` whose `dataType` it contradicts.

NOT AN ACKNOWLEDGED GAP: the handler's module doc lists every refusal it owns — src/ops/update_entity.rs:38-49, which names "an `updateType` outside its modelled value list, an entry whose action disagrees with what the entity holds ..., a `componentPath` that does not resolve, a component of an ABSTRACT component type ..., a property definition carrying no `dataType`, and a `parentEntityId` that names no entity of the workspace" — and the value's type is not among them.

TEST COVERAGE: the rows that exercise this arm all write matching types, so enforcing the rule leaves them passing — tests/all/entity_update.rs:272-275 pairs `"definition": {"dataType": {"type": "DOUBLE"}}` with `"value": {"doubleValue": 1.5}`, and :376-386 / :416-424 / :616-634 do the same for `Sensor/Temperature`.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}}}).
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Sensor": {"componentTypeId": "com.example.sensor", "properties": {"Temperature": {"value": {"doubleValue": 21.5}}}}}).
4. UpdateEntity(workspaceId="ws", entityId="e1", componentUpdates={"Sensor": {"updateType": "UPDATE", "propertyUpdates": {"Temperature": {"updateType": "UPDATE", "value": {"booleanValue": true}}}}}).
5. Observe HTTP 200.
6. GetEntity(workspaceId="ws", entityId="e1") -> components.Sensor.properties.Temperature reports definition.dataType.type = "DOUBLE" with value = {"booleanValue": true}; the correctly typed value that was there has been replaced by one the definition does not admit.
7. ListProperties(workspaceId="ws", entityId="e1", componentName="Sensor") -> the same mistyped value is reported in the propertySummaries row beside the DOUBLE definition.
