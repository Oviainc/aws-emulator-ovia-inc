Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3b2aca3b7a37a5147381645d3a0bbbf0f4b15f02

# A propertyUpdates RESET_VALUE (and a value write) only touches PropertyRecord::value, which no read consults once the data plane has written a series — so the call answers 200 and changes nothing

_Filed: 2026-10-29T15:57:14Z_
_Operation: UpdateEntity | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3b2aca3b7a37a5147381645d3a0bbbf0f4b15f02`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEntity`'s `apply_property_updates` writes a property's value into `crate::state::PropertyRecord::value` only. The crate single-homes the READ of a property's value in `crate::state::latest_property_value`, and that reader PREFERS the time series `BatchPutPropertyValues` wrote, explicitly because "a property the data plane has written to carries its values there and nowhere else". For a property the data plane has written to, therefore:

* `propertyUpdates.{P} = {"updateType": "RESET_VALUE"}` is answered 200 and the handler's own stated rule ("a reset takes the property's VALUE away") does not hold: `property.value` was already `None` or is set to `None`, the series is untouched, and `GetEntity` / `ListProperties` / `GetPropertyValue` keep reporting the same value as before the reset. Nothing a later caller can observe changed.
* the `CREATE` / `UPDATE` / no-updateType arm stores `property.value = Some(value)` into the same shadowed field, so the value the request carried is dropped from every read — the reads keep reporting the series' latest value instead.

The property record is reachable in exactly this state through the public API: `BatchPutPropertyValues`' `apply` CREATES the component's `PropertyRecord` (`properties.entry(property_name).or_default()`) when it records a series value, so `RESET_VALUE` finds a record to reset (it is not refused as "not there") and resets the one half of it no reader looks at.

This is not the open `UpdateEntity` validation issue (a value whose type contradicts the declared `dataType`); here the value is well typed and the write is simply invisible.

## Evidence

THE WRITER AND ITS OWN STATED RULE — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/update_entity.rs:609-644:
```
            Some(UPDATE_TYPE_RESET_VALUE) => {
                let Some(property) = properties.get_mut(&property_name) else {
                    return Err(absent(
                        &member,
                        Action::Update,
                        "property",
                        &property_name,
                    ));
                };
                // The definition stays: a reset takes the property's VALUE
                // away, not the declaration that gives it meaning.
                property.value = None;
            }
...
                let property = properties.entry(property_name).or_default();
                if let Some(definition) = update.definition {
                    property.definition = Some(definition);
                }
                if let Some(value) = update.value {
                    property.value = Some(value);
                }
```
and the operation's own constant doc, src/ops/update_entity.rs:387-389:
```
/// See [`UPDATE_TYPE_CREATE`]. `PropertyUpdateType` only: a property keeps its
/// definition and loses its value.
```

THE READER THAT CONTRADICTS IT — src/state.rs:3175-3202 (the ONE reader of a property's value, used by GetEntity, ListProperties and GetPropertyValue):
```
/// The value a read reports for ONE property: the MOST RECENT value written to
/// the property, else the DEFAULT its definition declares, and nothing at all
/// when neither exists.
///
/// A property is written to through either plane, so both are consulted here:
/// the time-series data plane writes values FOR an instant
/// ([`PropertyRecord::time_series`], keyed by that instant, so its LAST entry
/// is the value written for the latest instant), and the control plane writes
/// one current value ([`PropertyRecord::value`]). The series is preferred where
/// it holds anything, because a property the data plane has written to carries
/// its values there and nowhere else;
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
and src/state.rs:3894-3907:
```
    /// The most recent value WRITTEN to this property, through either plane —
    /// `None` for a property nothing has written a value to.
...
    pub fn latest_written_value(&self) -> Option<crate::ops::types::DataValue> {
        if let Some((_, latest)) = self.time_series.last_key_value() {
            return Some(latest.value.clone());
        }
        self.value.clone()
    }
```

HOW THE RECORD GETS INTO THAT STATE — src/ops/batch_put_property_values.rs:608-659:
```
/// The property record is CREATED where the component carries none: a time
/// series property normally takes its definition from the component type the
/// component instantiates, so the component itself holds nothing for it until a
/// value is written.
...
    let property = properties.entry(property_name).or_default();
    for (at, value) in values {
        property.record_time_series_value(at, value);
    }
```

WHERE THE SHADOWED VALUE IS REPORTED — src/ops/get_entity.rs:415-429 (`property_response` calls `crate::state::latest_property_value`) and src/ops/list_properties.rs:399-402 (`property_summary` does the same).

TEST COVERAGE: the only RESET_VALUE rows exercise a property with NO series — tests/all/entity_update.rs:460-474 resets `Sensor/Serial`, a `STRING` property the data plane never wrote to, and asserts `/components/Sensor/properties/Serial/value` is absent afterwards; tests/all/entity_update.rs:614 does the same inside a multi-entry request. Nothing pins the behaviour of a reset (or a value write) on a property carrying a time series, so closing this gap leaves those rows passing.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}}).
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Sensor": {"componentTypeId": "com.example.sensor"}}).
4. BatchPutPropertyValues(workspaceId="ws", entries=[{"entityPropertyReference": {"entityId": "e1", "componentName": "Sensor", "propertyName": "Temp"}, "propertyValues": [{"time": "2026-01-01T00:00:00Z", "value": {"doubleValue": 21.5}}]}]) -> 200, and GetEntity reports components.Sensor.properties.Temp.value = {"doubleValue": 21.5}.
5. UpdateEntity(workspaceId="ws", entityId="e1", componentUpdates={"Sensor": {"updateType": "UPDATE", "propertyUpdates": {"Temp": {"updateType": "RESET_VALUE"}}}}) -> 200 (the property record exists, so the "which is not there" refusal does not fire).
6. GetEntity(workspaceId="ws", entityId="e1") -> components.Sensor.properties.Temp.value is STILL {"doubleValue": 21.5}: the reset the handler's own comment says "takes the property's VALUE away" took nothing away. ListProperties(workspaceId="ws", entityId="e1", componentName="Sensor") and GetPropertyValue(selectedProperties=["Temp"]) report the same unchanged value.
7. Second symptom: UpdateEntity(..., propertyUpdates={"Temp": {"updateType": "UPDATE", "value": {"doubleValue": 99.0}}}) -> 200, then GetEntity still reports 21.5 — the written value is stored in a field no read consults.
