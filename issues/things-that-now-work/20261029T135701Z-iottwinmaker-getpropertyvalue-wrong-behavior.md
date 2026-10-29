Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0e690eba1a26d730185f2b1cbba6603f666a0a19

# GetPropertyValue ignores a time-series property's stored series, so the "latest property value" is reported as the default (or nothing) after BatchPutPropertyValues

_Filed: 2026-10-29T13:57:01Z_
_Operation: GetPropertyValue | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0e690eba1a26d730185f2b1cbba6603f666a0a19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchPutPropertyValues` stores the values it accepts in `PropertyRecord::time_series`, a map keyed by the instant each value was written for, so the last entry of that map IS the property's latest value. `GetPropertyValue` never looks at it.

The only reader of a property's current value is the shared helper `crate::state::latest_property_value`, which consults exactly two places: `property.value` (which the time-series write never sets) and `definition.default_value`. So after a successful `BatchPutPropertyValues` the latest-value read answers the property's DEFAULT, or omits `propertyValue` entirely when the definition declares none — while `GetPropertyValueHistory`, reading the very same record under the very same lock, reports the values that were written.

That contradicts the modelled meaning of the member the answer travels in: `GetPropertyValueResponse$propertyValues` is documented as the map of "the properties and latest property values in the response", and each entry is a `PropertyLatestValue`, "The latest value of the property." A property whose definition is `isTimeSeries` is precisely the property whose latest value only the series holds, so for the whole class of properties this operation's response member is named after, the handler reports something other than the latest value.

The same shared helper is the one `GetEntity` (`get_entity.rs:417-418`), `ListProperties` (`list_properties.rs:398-401`) and the tabular branch of this operation (`get_property_value.rs:676`, reached from `Table::read`) all build their values from, so a time-series property reads as the default through every one of them; this issue is filed against `GetPropertyValue`, the operation whose own response member documents the word "latest", and the fix belongs in the shared helper.

## Evidence

WHAT THE MODEL SAYS THE MEMBER CARRIES — `smithy_query(query="doc", shape="GetPropertyValueResponse", member="propertyValues")`:
```
"An object that maps strings to the properties and latest property values in the response. Each string in the mapping must be unique to this object."
```
and `smithy_query(query="shape", shape="PropertyLatestValue")`:
```
"doc": "<p>The latest value of the property.</p>"
```

WHERE THE SERIES IS STORED — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs:3776-3786:
```
    /// The values written to this property through the time-series data plane
    /// (`BatchPutPropertyValues`), keyed by the instant each was written FOR in
    /// nanoseconds since the Unix epoch ([`instant_of_time`]).
    ///
    /// A map keyed by the instant, so the series is in time order however it
    /// was written, a value written twice for one instant is stored once, and a
    /// read of a window takes a range of it.
    pub time_series: BTreeMap<i128, TimeSeriesValue>,
```
with `TimeSeriesValue { pub value: crate::ops::types::DataValue, .. }` (state.rs:3410-3411).

THE WRITE THAT ONLY FILLS THE SERIES — src/ops/batch_put_property_values.rs:657-660 (nothing in the file assigns `property.value`):
```
    let property = properties.entry(property_name).or_default();
    for (at, value) in values {
        property.record_time_series_value(at, value);
    }
```

THE READ THAT SKIPS IT — src/state.rs:3107-3114:
```
pub fn latest_property_value(
    property: Option<&PropertyRecord>,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::DataValue> {
    property
        .and_then(|property| property.value.clone())
        .or_else(|| definition.and_then(|definition| definition.default_value.clone()))
}
```
and its one call site in this operation, src/ops/get_property_value.rs:666-680:
```
fn property_value(
    workspace: &crate::state::Workspace,
    component: &ScopedComponent<'_>,
    property_name: &str,
) -> Option<DataValue> {
    let definition = workspace.effective_property_definition(..);
    crate::state::latest_property_value(
        component.properties.get(property_name),
        definition.as_ref(),
    )
}
```

THE SIBLING READER THAT DOES USE THE SERIES — src/ops/get_property_value_history.rs:923-927:
```
            let Some(property) = component.properties.get(property_name) else {
                continue;
            };
            let mut values = Vec::new();
            for (at, value) in &property.time_series {
```

TEST COVERAGE: tests/all/time_series_property_values.rs exercises `BatchPutPropertyValues` -> `GetPropertyValueHistory` only ("The time-series data plane — `BatchPutPropertyValues` and `GetPropertyValueHistory` — end to end over HTTP", lines 3-4); it never calls `GetPropertyValue`. tests/all/property_value_read.rs pins the latest-value read but seeds no time-series write at all, and states the rule as "a property reads as the value written to it, else as the default declared by the definition" (lines 5-8) — a time-series value IS "written to it" in this crate's own words (state.rs:3776), so no existing test pins the current answer and none would change meaning.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}}).
3. CreateEntity(workspaceId="ws", entityId="unit", entityName="unit", components={"Spec": {"componentTypeId": "com.example.sensor"}}).
4. BatchPutPropertyValues(workspaceId="ws", entries=[{"entityPropertyReference": {"entityId": "unit", "componentName": "Spec", "propertyName": "Temp"}, "propertyValues": [{"time": "2026-01-01T00:00:00Z", "value": {"doubleValue": 1.0}}, {"time": "2026-01-01T00:01:00Z", "value": {"doubleValue": 2.0}}]}]) -> 200 with no errorEntries.
5. GetPropertyValueHistory(workspaceId="ws", entityId="unit", componentName="Spec", selectedProperties=["Temp"]) -> 200, both values, the latest being 2.0 at 2026-01-01T00:01:00Z: the series is in the store.
6. GetPropertyValue(workspaceId="ws", entityId="unit", componentName="Spec", selectedProperties=["Temp"]) -> 200 with propertyValues["Temp"].propertyValue ABSENT, although the member is documented to carry the latest property value and the store holds 2.0.
7. Add defaultValue={"doubleValue": 9.9} to the definition and repeat: GetPropertyValue now answers 9.9 — the default — rather than the 2.0 that was last written.
8. GetEntity(workspaceId="ws", entityId="unit") and ListProperties(workspaceId="ws", entityId="unit", componentName="Spec") report the same default/absent value, since all three build it from the one shared helper.
