Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# A batch in which every entry is servable answers errorEntries as a 0-length list, outside the @length(1..10) the modelled required member carries

_Filed: 2026-10-29T09:02:55Z_
_Operation: BatchPutPropertyValues | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchPutPropertyValuesResponse$errorEntries` is `@required` AND carries `@length(min: 1, max: 10)` (through `ErrorEntries`). The handler builds that member as a plain `Vec` that only ever receives one element per UNSERVABLE entry, and returns it as it stands — so the ordinary success path (every entry of the batch stored) answers HTTP 200 with `"errorEntries": []`, a value of length 0 that the member the model marks required does not admit.

The constraint is not something the handler is unaware of: the generated contract block inside the very same file states it (`//! * `errorEntries` (ErrorEntries, len 1..10) **required**`), and the handler's own module doc states only the per-failure side of the rule ("one `BatchPutPropertyError` per failed entry") without saying anything about what the member holds when nothing failed. No comment, constant or doc anywhere in the crate justifies serving the member below its modelled minimum, and the boundary validator does not police output, so the divergence reaches the wire.

The consequence is that a client generated from this model — which is entitled to treat `errorEntries` as a list of at least one element — sees a value the service's own model forbids on every fully successful call, which is the most common outcome of the operation.

## Evidence

THE MODEL — `smithy_query(query="shape", shape="ErrorEntries,BatchPutPropertyValuesResponse")`:
```
"ErrorEntries": {"shape_id": "com.amazonaws.iottwinmaker#ErrorEntries", "type": "list", "length": {"min": 1, "max": 10}, "member": "BatchPutPropertyErrorEntry"}
"BatchPutPropertyValuesResponse": {... "members": {"errorEntries": {"target": "ErrorEntries", "required": true, "length": {"min": 1, "max": 10}, "type": "list", "doc": "<p>Entries that caused errors in the batch put operation.</p>"}}}
```

THE SAME CONSTRAINT RESTATED IN THE HANDLER FILE — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/batch_put_property_values.rs:79:
```
//! * `errorEntries` (ErrorEntries, len 1..10) **required** — Entries that caused errors in
```

THE CODE THAT ANSWERS BELOW THAT MINIMUM — src/ops/batch_put_property_values.rs:195-215:
```
        let mut plans = Vec::new();
        let mut error_entries = Vec::new();
        for entry in &input.entries {
            match plan_entry(workspace, &input.workspace_id, entry) {
                Ok(plan) => plans.push(plan),
                Err(error) => error_entries.push(BatchPutPropertyErrorEntry {
                    errors: vec![BatchPutPropertyError {
...
        Ok(BatchPutPropertyValuesOutput { error_entries })
```
`error_entries` is pushed to ONLY in the `Err(error)` arm, so a batch whose every entry plans successfully returns it empty.

THE HANDLER'S OWN DOC, WHICH STATES ONLY THE PER-FAILURE SIDE — src/ops/batch_put_property_values.rs:4-9:
```
//! WRITES values into the time series of the properties the request's entries
//! name: "sets values for multiple time series properties". Each entry is
//! served on its own — the entries this service can serve are stored and the
//! ones it cannot come back in `errorEntries`, one `BatchPutPropertyError` per
//! failed entry carrying the entry itself, so a batch mixing servable entries
//! with unservable ones is PARTIALLY applied rather than refused whole.
```
Nothing in the file addresses the modelled minimum of 1.

TEST COVERAGE: the empty list is asserted, with a reason that speaks only about errors and never about the modelled minimum — tests/all/time_series_property_values.rs:268-273:
```
        assert_eq!(
            written.member("errorEntries"),
            &json!([]),
            "case {:?}: a servable entry earns no error, got {written:?}",
            row.case
        );
```
same assertion at tests/all/time_series_property_values.rs:777 and 1085. No test anywhere in tests/all/ checks `errorEntries` against its modelled length.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="ws").
2. CreateComponentType(workspaceId="ws", componentTypeId="com.example.sensor", propertyDefinitions={"Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}}).
3. CreateEntity(workspaceId="ws", entityId="e1", entityName="E1", components={"Sensor": {"componentTypeId": "com.example.sensor"}}).
4. BatchPutPropertyValues(workspaceId="ws", entries=[{"entityPropertyReference": {"entityId": "e1", "componentName": "Sensor", "propertyName": "Temp"}, "propertyValues": [{"time": "2024-01-01T00:00:00Z", "value": {"doubleValue": 1.0}}]}]).
5. Observe HTTP 200 with body {"errorEntries": []} — a 0-length value for the member the model declares @required with @length(min 1, max 10) (ErrorEntries), the constraint the handler file itself restates at src/ops/batch_put_property_values.rs:79.
6. Contrast the partially-failing batch (send one servable entry plus one naming a property that is not isTimeSeries): errorEntries then holds 1 entry, inside the modelled bound.
