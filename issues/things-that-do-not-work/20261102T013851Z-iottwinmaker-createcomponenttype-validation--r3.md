Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cd08eede305788d0e1af3dc59e94a11366e984d0

# A property definition's defaultValue is stored without the dataType check every other writer of a property's value applies, so a read reports a value contradicting the dataType beside it

_Filed: 2026-11-02T01:38:51Z_
_Operation: CreateComponentType | Class: validation_

> _Note: this issue was identified against the code base at commit `cd08eede305788d0e1af3dc59e94a11366e984d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateComponentType` stores the entries of `propertyDefinitions` verbatim, including each entry's `defaultValue`. The only rule it asks of a definition is that a `dataType` be resolvable for it (`stranded_property_definition`); nothing judges the `defaultValue` AGAINST that `dataType`.

That matters because this crate's single read-side projection reports a property's `defaultValue` AS the property's value: `crate::state::latest_property_value` falls back to `definition.default_value` for any property nothing has written to, and `GetEntity`, `ListProperties`, `GetPropertyValue` and `BatchPutPropertyValues` all read a property's value through it. So `CreateComponentType(propertyDefinitions={"Count": {"dataType": {"type": "STRING"}, "defaultValue": {"integerValue": 5}}})` is accepted with 200, and a later `GetEntity` on any entity holding a component of that type answers ONE `PropertyResponse` whose `definition.dataType.type` is `STRING` and whose `value` is `{"integerValue": 5}` — a self-contradictory payload.

This is the exact outcome the crate wrote `crate::state::data_value_type_violation` to prevent, and that helper is called on every OTHER path a value reaches a property by: the time-series data plane (`batch_put_property_values.rs:467`), `CreateEntity`'s component properties (`create_entity.rs:669`) and `UpdateEntity`'s `propertyUpdates` (`update_entity.rs:736`). The component type's `defaultValue` — and the `defaultValue` inside a component-level `PropertyDefinitionRequest`, which `CreateEntity`/`UpdateEntity` likewise store unchecked — is the one way into the store that skips it.

The same gap sits in `UpdateComponentType` (`update_component_type.rs:326-328` replaces `property_definitions` whole with no value check), `CreateEntity` (`create_entity.rs:645-680` checks `property.value` but never `property.definition.default_value`) and `UpdateEntity` (`update_entity.rs:702-741`, same). It is filed once, against the create that first admits such a definition.

## Evidence

THE RULE THE CRATE STATES FOR ITSELF — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:3570-3584:
```
/// Why `value` cannot be stored as a value of the property `definition`
/// defines, as the message that earns — `None` when it can.
///
/// A property's `dataType` is "an object that specifies the data type of a
/// property", so a value written to the property carries a value of THAT type:
/// one carrying another type, or no value at all, is refused rather than
/// stored, which would leave a read reporting a value of a type the definition
/// does not declare. ...
/// Shared by every writer of a property's value, whichever plane it writes
/// through: ...
pub fn data_value_type_violation(
```
and src/ops/update_entity.rs:720-729 restates the purpose:
```
                // The value this entry writes has to be a value of the
                // `dataType` the property ENDS UP with: ... the rule
                // `crate::state::Workspace::component_property_value_violation`
                // every writer of a property's value shares, decided before
                // the patch is applied so a mistyped value is refused rather
                // than left for a read to report against a `dataType` it
                // contradicts.
```

THE READ THAT REPORTS defaultValue AS THE VALUE — src/state.rs:3451-3453 and 3472-3479:
```
/// The value a read reports for ONE property: the MOST RECENT value written to
/// the property, else the DEFAULT its definition declares, and nothing at all
/// when neither exists.
...
pub fn latest_property_value(
    property: Option<&PropertyRecord>,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::DataValue> {
    property
        .and_then(|property| property.latest_written_value())
        .or_else(|| definition.and_then(|definition| definition.default_value.clone()))
}
```
and src/ops/get_entity.rs:417-432, which puts that value and the definition's `dataType` in one response:
```
    let definition =
        workspace.effective_property_definition(component_type_id, properties, property_name);
    let value =
        crate::state::latest_property_value(properties.get(property_name), definition.as_ref());
    PropertyResponse {
        definition: definition.as_ref().and_then(|definition| {
            crate::state::property_definition_response(
...
        value,
    }
```

THE PATHS THAT DO CALL THE RULE — src/ops/batch_put_property_values.rs:467-473:
```
        if let Some(violation) = crate::state::data_value_type_violation(
            &property_name,
            definition.as_ref(),
            &value.value,
        ) {
            return Err(invalid_entry(violation));
        }
```
src/ops/create_entity.rs:665-677:
```
        // Asked of the value the create would STORE, before anything is
        // stored: a property with no declaration of its own is still judged
        // against the definition it inherits.
        if let Some(value) = property.value.as_ref() {
            if let Some(violation) = workspace.component_property_value_violation(
```

THE PATH THAT DOES NOT — src/ops/create_component_type.rs:360-379 is the only rule asked of a definition (presence of a `dataType`, not agreement with it):
```
        // `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on
        // the `PropertyDefinitionResponse` a read reports, so a definition may
        // omit it only where it REFINES one the declared inheritance brings for
        // the same property — the rule
        // `crate::state::Workspace::stranded_property_definition` ...
        if let Some(property_name) = workspace.stranded_property_definition(
```
and src/ops/create_component_type.rs:403 stores the map as it arrived:
```
            property_definitions: property_definitions.into_iter().collect(),
```
`defaultValue` is a member of that stored shape — src/ops/types/shared_types.rs:113-124:
```
pub struct PropertyDefinitionRequest {
    pub data_type: Option<DataType>,
...
    pub default_value: Option<DataValue>,
```
and the model documents it as the property's value, nothing narrower (`smithy_query(query="doc", shape="PropertyDefinitionRequest", member="defaultValue")`): `"<p>An object that contains the default value.</p>"`.

TEST COVERAGE: no test sends a `defaultValue` whose type differs from its `dataType`. Every seeded default agrees with its declared type — tests/all/property_value_read.rs:57-60 (`"Tag": {"dataType": {"type": "STRING"}, "defaultValue": {"stringValue": "unset-tag"}}`, `"Count": {"dataType": {"type": "INTEGER"}, "defaultValue": {"integerValue": 7}}`) and tests/all/component_definition_refinement.rs:193-198 (`refinement: json!({"defaultValue": {"longValue": 7}})` against `data_type: "LONG"`). tests/all/component_type_lifecycle.rs:333-337 only asserts the stored default is echoed back:
```
            assert_eq!(
                response["defaultValue"], definition["defaultValue"],
                "case {component_type_id:?}/{property_name}: expected the created defaultValue, \
                 got {response:?}"
            );
```
so nothing pins the acceptance of a mistyped default, and no existing expectation changes when it is refused.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="w").
2. CreateComponentType(workspaceId="w", componentTypeId="ct", propertyDefinitions={"Count": {"dataType": {"type": "STRING"}, "defaultValue": {"integerValue": 5}}}) — observe HTTP 200, although `crate::state::data_value_type_violation` would answer "the value written to property Count is a INTEGER where that property's dataType is STRING" for the same pair.
3. CreateEntity(workspaceId="w", entityId="e", entityName="e", components={"c": {"componentTypeId": "ct"}}) — the component declares no property of its own, so `Count` is inherited.
4. GetEntity(workspaceId="w", entityId="e") and read components.c.properties.Count: the response carries `definition.dataType.type == "STRING"` and `value == {"integerValue": 5}` in one payload — "a read reporting a value of a type the definition does not declare", which src/state.rs:3570-3578 says is refused rather than stored.
5. GetPropertyValue(workspaceId="w", entityId="e", componentName="c", selectedProperties=["Count"]) and ListProperties(workspaceId="w", entityId="e", componentName="c") report the same mistyped value, both reading it through `crate::state::latest_property_value`.
6. Contrast: BatchPutPropertyValues writing `{"integerValue": 5}` to a STRING property is refused at src/ops/batch_put_property_values.rs:467, and UpdateEntity's propertyUpdates likewise at src/ops/update_entity.rs:736.
