Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# A component property value whose type contradicts the property's own declared dataType is stored and read back, skipping the crate's one data_value_type_violation rule that BatchPutPropertyValues applies to the same records

_Filed: 2026-10-29T02:01:20Z_
_Operation: CreateEntity | Class: validation_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEntity` stores every `PropertyRequest.value` verbatim. It checks only that a definition the request carries HAS a `dataType` (`require_data_types`, which looks at `definition.data_type.is_none()`); it never compares the VALUE it stores against that `dataType`.

The crate already owns that rule — `crate::state::data_value_type_violation` — and its own documentation states it as a rule about any value written to a property, giving as the reason exactly the outcome this path produces. Today the only caller of that helper is `BatchPutPropertyValues` (src/ops/batch_put_property_values.rs:334-338), which refuses a time-series value of the wrong type with a `ValidationException` entry error. `CreateEntity` writes into the SAME `PropertyRecord`s that handler writes into, and skips the check.

The consequence is not confined to storage: `crate::state::latest_property_value` hands the stored `value` straight back, so `GetEntity` (src/ops/get_entity.rs:409-430), `ListProperties` (src/ops/list_properties.rs:398-401) and `GetPropertyValue` all report a `stringValue` for a property whose reported `PropertyDefinitionResponse.dataType` is `DOUBLE` — the very "read reporting a value of a type the definition does not declare" the helper's documentation says is refused rather than stored. Worse, `GetPropertyValue`'s tabular filters then compare that value through `compare_data_values`, which answers `None` for two values of different kinds, so a `propertyFilters` entry on that property silently selects no row instead of comparing.

The same gap exists for a value supplied against a definition the component INHERITS from its component type: `require_data_types` only looks at the definition the request itself carries, so `components.<name>.properties.Serial.value = {"doubleValue": 1.0}` on a component instantiating a type that declares `Serial: STRING` is stored unchecked too.

## Evidence

THE RULE THE CRATE STATES — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:3188-3217:
```
/// Why `value` cannot be stored as a value of the property `definition`
/// defines, as the message that earns — `None` when it can.
///
/// A property's `dataType` is "an object that specifies the data type of a
/// property", so a value written to the property carries a value of THAT type:
/// one carrying another type, or no value at all, is refused rather than
/// stored, which would leave a read reporting a value of a type the definition
/// does not declare. A definition declaring no `dataType` constrains nothing,
/// so only the presence of a value is required there.
pub fn data_value_type_violation(
    property_name: &str,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
    value: &crate::ops::types::DataValue,
) -> Option<String> {
```

THE ONE PATH THAT APPLIES IT — src/ops/batch_put_property_values.rs:333-338:
```
    for value in requested {
        if let Some(violation) =
            crate::state::data_value_type_violation(property_name, definition.as_ref(), &value.value)
        {
            return Err(invalid_entry(violation));
        }
```

THE PATH THAT SKIPS IT — src/ops/create_entity.rs:516-536, which stores the value with no check at all:
```
fn property_records(
    properties: ::std::collections::HashMap<String, PropertyRequest>,
) -> ::std::collections::BTreeMap<String, crate::state::PropertyRecord> {
    properties
        .into_iter()
        .map(|(property_name, property)| {
            (
                property_name,
                crate::state::PropertyRecord {
                    definition: property.definition,
                    value: property.value,
```
and the only property check the handler does make, src/ops/create_entity.rs:586-600, which is about the definition's dataType being PRESENT, not about the value's type:
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
```
(called for every component and composite component at src/ops/create_entity.rs:331-342).

HOW THE MISTYPED VALUE REACHES A READ — src/state.rs:3090-3097:
```
pub fn latest_property_value(
    property: Option<&PropertyRecord>,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::DataValue> {
    property
        .and_then(|property| property.value.clone())
```
used by src/ops/get_entity.rs:417-418 ("the value the property reads as") and src/ops/list_properties.rs:398-401.

NOT AN ACKNOWLEDGED GAP: the handler's own module doc enumerates what it refuses and what it deliberately does not — src/ops/create_entity.rs:10-11 ("Five refusals this handler owns") and :43-51 ("Two deliberate gaps: a `componentTypeId` the workspace does not hold at all is still stored without being refused ... and authorization is not emulated"). The value/dataType rule is in neither list.

TEST COVERAGE: no test sends a value whose type disagrees with the declared dataType, and every test that sets a value sets a matching one, so enforcing the existing rule leaves them all passing — tests/all/property_value_read.rs:58-59 declares `"Threshold": {"dataType": {"type": "DOUBLE"}}` and :119 writes `{"Threshold": {"value": {"doubleValue": 9.5}}}`; tests/all/entity_composite_components.rs:202-205 pairs `"dataType": {"type": "DOUBLE"}` with `"value": {"doubleValue": 12.5}`; tests/all/execute_query.rs:141-144 and :148-155 pair DOUBLE with `doubleValue` and RELATIONSHIP with `relationshipValue`.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Temperature": {"dataType": {"type": "DOUBLE"}}}).
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Sensor": {"componentTypeId": "com.example.sensor", "properties": {"Temperature": {"definition": {"dataType": {"type": "DOUBLE"}}, "value": {"stringValue": "hot"}}}}}).
4. Observe HTTP 200: the STRING value was stored against a DOUBLE property.
5. GetEntity(workspaceId="ws", entityId="e1") -> components.Sensor.properties.Temperature reports definition.dataType.type = "DOUBLE" together with value = {"stringValue": "hot"} — a read reporting a value of a type the definition does not declare.
6. Contrast the same mistyped write through the data plane on a time-series property: BatchPutPropertyValues with a value whose type differs from the declared dataType comes back in errorEntries with errorCode "ValidationException" and message "the value written to property <name> is a STRING where that property's dataType is DOUBLE" (src/ops/batch_put_property_values.rs:334-338).
7. GetPropertyValue(workspaceId="ws", entityId="e1", componentName="Sensor", selectedProperties=["Temperature"], tabularConditions={"propertyFilters": [{"propertyName": "Temperature", "operator": ">", "value": {"doubleValue": 0.0}}]}) -> the row is silently dropped, because compare_data_values answers None for two values of different kinds.
