// SPDX-License-Identifier: CC0-1.0

//! The LATEST value of a time series property, end to end over HTTP.
//!
//! RULE under test: a property reads as the MOST RECENT value written to it
//! through either plane — the value of the latest instant in its time series
//! where the data plane has written one, else the single value the control
//! plane set on it — and only where nothing has been written to it does it read
//! as the default its definition declares, or as nothing where there is no
//! default either. Every reader of a property's current value answers that one
//! way: `GetPropertyValue` (both the keyed map and the tabular branch),
//! `ListProperties`, `GetEntity` and `ExecuteQuery`.
//!
//! The rows of each table below differ along what the rule generalises over:
//! the DATA TYPE the value travels in (`DOUBLE`, `INTEGER`, `LONG`, `STRING`,
//! `BOOLEAN`), HOW MANY values the series holds and in which order they were
//! written (one, several in time order, several out of time order, the same
//! instant written twice), WHERE the value comes from (the series, the control
//! plane, the definition's default, nowhere), whether the property is a time
//! series property at all, and WHICH component carries it (a top-level one and
//! a composite one addressed by its `componentPath`).

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "tslatest";

/// The component type whose definitions mark one property of each data type as
/// a time series property, beside the properties that pin the rest of the rule:
/// one with a default the series has to win over, one with a default and no
/// series, one with neither, and two that are not time series properties.
const SENSOR: &str = "com.example.sensor";
/// A component type declaring a COMPOSITE component type of [`SENSOR`], so an
/// entity component of this type carries a composite component addressed by the
/// path `Rig/inner`.
const RIG: &str = "com.example.rig";

/// Instants one minute apart, as the ISO 8601 `Time` a write carries. Written
/// out rather than computed so a failing assertion names the instant it is
/// about.
const T0: &str = "2026-03-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-03-01T00:01:00Z";
/// See [`T0`].
const T2: &str = "2026-03-01T00:02:00Z";
/// See [`T0`].
const T3: &str = "2026-03-01T00:03:00Z";

/// The workspace, component types and entities every case below uses: one
/// entity carrying a top-level component of [`SENSOR`], and one carrying a
/// component of [`RIG`] with a composite component of [`SENSOR`] inside it.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    "Count": {"dataType": {"type": "INTEGER"}, "isTimeSeries": true},
                    "Ticks": {"dataType": {"type": "LONG"}, "isTimeSeries": true},
                    "Status": {"dataType": {"type": "STRING"}, "isTimeSeries": true},
                    "Online": {"dataType": {"type": "BOOLEAN"}, "isTimeSeries": true},
                    "Load": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    // A series has to win over a default the definition declares.
                    "Calibration": {
                        "dataType": {"type": "DOUBLE"},
                        "isTimeSeries": true,
                        "defaultValue": {"doubleValue": 9.9},
                    },
                    // A time series property nothing has written to: its
                    // default is still what it reads as.
                    "Baseline": {
                        "dataType": {"type": "DOUBLE"},
                        "isTimeSeries": true,
                        "defaultValue": {"doubleValue": 1.25},
                    },
                    // Neither a series nor a default: the property reads as
                    // nothing at all.
                    "Residual": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    // Not a time series property: written by the control plane,
                    // or left to its default.
                    "Zone": {"dataType": {"type": "STRING"}},
                    "Label": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset-label"},
                    },
                },
                // The write connector the time-series writes below are set
                // THROUGH.
                "functions": data_writer_functions(),
            }),
        ),
        (
            RIG,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": SENSOR}}}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let entities: Vec<Value> = vec![
        json!({
            "entityId": "unit-1",
            "entityName": "unit-1",
            "components": {"Sensor": {
                "componentTypeId": SENSOR,
                "properties": {"Zone": {"value": {"stringValue": "north"}}},
            }},
        }),
        json!({
            "entityId": "rig-1",
            "entityName": "rig-1",
            "components": {"Rig": {"componentTypeId": RIG}},
            "compositeComponents": {"Rig/inner": {}},
        }),
    ];
    for body in entities {
        let created = emulator.create_entity(WORKSPACE, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {body:?}: expected 200, got {created:?}"
        );
    }
}

/// Write `values` to one property through the time-series data plane, asserting
/// the write was served in full.
async fn put(emulator: &Emulator, reference: Value, values: Value, case: &str) {
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{"entityPropertyReference": reference, "propertyValues": values}]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: BatchPutPropertyValues expected 200, got {written:?}"
    );
    let errors = written
        .member("errorEntries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        errors.iter().all(|entry| entry["errors"]
            .as_array()
            .is_none_or(|errors| errors.is_empty())),
        "case {case:?}: BatchPutPropertyValues expected no errorEntries, got {written:?}"
    );
}

/// `GetPropertyValue` for one property of one addressed component.
async fn read_one(
    emulator: &Emulator,
    entity_id: &str,
    component_name: Option<&str>,
    component_path: Option<&str>,
    property: &str,
) -> Resp {
    let mut body = json!({"entityId": entity_id, "selectedProperties": [property]});
    if let Some(component_name) = component_name {
        body["componentName"] = json!(component_name);
    }
    if let Some(component_path) = component_path {
        body["componentPath"] = json!(component_path);
    }
    emulator.get_property_value(WORKSPACE, body).await
}

/// One row of the latest-value table: what to write, where, and the
/// `DataValue` the read has to report.
#[derive(Debug)]
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    entity_id: &'static str,
    /// The addressing member: a top-level component by name, or a composite
    /// component by path.
    component_name: Option<&'static str>,
    component_path: Option<&'static str>,
    property: &'static str,
    /// The `(time, value)` pairs written through `BatchPutPropertyValues`
    /// before the read, in the order they are written — empty for a property
    /// the data plane never touches.
    writes: Vec<(&'static str, Value)>,
    /// The `DataValue` member the reported value travels in and the value
    /// itself, or `None` where the property reads as nothing.
    expected: Option<(&'static str, Value)>,
}

/// RULE: a property reads as the most recent value written to it — the value of
/// the latest instant in its series where the data plane wrote one, else the
/// value the control plane set — else as its definition's default, else as
/// nothing.
#[tokio::test]
async fn a_property_reads_as_the_latest_value_written_to_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let cases = vec![
        Case {
            what: "a DOUBLE series written in time order reads as its last value",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Temp",
            writes: vec![
                (T0, json!({"doubleValue": 1.0})),
                (T1, json!({"doubleValue": 2.0})),
            ],
            expected: Some(("doubleValue", json!(2.0))),
        },
        Case {
            what: "an INTEGER series written OUT of time order reads as its latest instant",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Count",
            writes: vec![
                (T2, json!({"integerValue": 30})),
                (T0, json!({"integerValue": 10})),
                (T1, json!({"integerValue": 20})),
            ],
            expected: Some(("integerValue", json!(30))),
        },
        Case {
            what: "a LONG series of one value reads as that value",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Ticks",
            writes: vec![(T0, json!({"longValue": 5}))],
            expected: Some(("longValue", json!(5))),
        },
        Case {
            what: "a STRING series reads as the value of its latest instant",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Status",
            writes: vec![
                (T0, json!({"stringValue": "idle"})),
                (T3, json!({"stringValue": "running"})),
            ],
            expected: Some(("stringValue", json!("running"))),
        },
        Case {
            what: "a BOOLEAN series reads as the value of its latest instant",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Online",
            writes: vec![
                (T1, json!({"booleanValue": false})),
                (T2, json!({"booleanValue": true})),
            ],
            expected: Some(("booleanValue", json!(true))),
        },
        Case {
            what: "the same instant written twice reads as the value written last",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Load",
            writes: vec![
                (T1, json!({"doubleValue": 1.0})),
                (T1, json!({"doubleValue": 4.0})),
            ],
            expected: Some(("doubleValue", json!(4.0))),
        },
        Case {
            what: "a series value wins over the default the definition declares",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Calibration",
            writes: vec![(T0, json!({"doubleValue": 3.3}))],
            expected: Some(("doubleValue", json!(3.3))),
        },
        Case {
            what: "a time series property nothing wrote to reads as its default",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Baseline",
            writes: vec![],
            expected: Some(("doubleValue", json!(1.25))),
        },
        Case {
            what: "a property with no series, no value and no default reads as nothing",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Residual",
            writes: vec![],
            expected: None,
        },
        Case {
            what: "a property that is not a time series reads as its control-plane value",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Zone",
            writes: vec![],
            expected: Some(("stringValue", json!("north"))),
        },
        Case {
            what: "a property that is not a time series and was never written reads as its default",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Label",
            writes: vec![],
            expected: Some(("stringValue", json!("unset-label"))),
        },
        Case {
            what: "a composite component's series, addressed by its componentPath",
            entity_id: "rig-1",
            component_name: None,
            component_path: Some("Rig/inner"),
            property: "Temp",
            writes: vec![
                (T0, json!({"doubleValue": -7.5})),
                (T2, json!({"doubleValue": 12.25})),
            ],
            expected: Some(("doubleValue", json!(12.25))),
        },
    ];

    for case in &cases {
        if !case.writes.is_empty() {
            let mut reference = json!({
                "entityId": case.entity_id,
                "propertyName": case.property,
            });
            if let Some(component_name) = case.component_name {
                reference["componentName"] = json!(component_name);
            }
            if let Some(component_path) = case.component_path {
                reference["componentPath"] = json!(component_path);
            }
            // Written one call at a time, so the row that writes the same
            // instant twice really is two writes in the order it lists them.
            for (time, value) in &case.writes {
                put(
                    &emulator,
                    reference.clone(),
                    json!([{"time": time, "value": value}]),
                    case.what,
                )
                .await;
            }
        }

        let read = read_one(
            &emulator,
            case.entity_id,
            case.component_name,
            case.component_path,
            case.property,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: GetPropertyValue expected 200, got {read:?}",
            case.what
        );
        let latest = &read.member("propertyValues")[case.property];
        assert_eq!(
            latest["propertyReference"]["propertyName"],
            json!(case.property),
            "case {:?}: expected the reference to name the property, got {latest:?}",
            case.what
        );
        match &case.expected {
            Some((member, value)) => assert_eq!(
                latest["propertyValue"][member],
                *value,
                "case {:?}: expected propertyValue.{member} to be {value}, got {latest:?}",
                case.what
            ),
            None => assert!(
                latest["propertyValue"].is_null(),
                "case {:?}: expected no propertyValue, got {latest:?}",
                case.what
            ),
        }
    }
}

/// The value one reader reports for `property` of `unit-1`'s `Sensor`
/// component, as the `DataValue` object every reader carries it in.
async fn through_each_reader(emulator: &Emulator, property: &str) -> Vec<(&'static str, Value)> {
    let mut reported = Vec::new();

    let read = read_one(emulator, "unit-1", Some("Sensor"), None, property).await;
    assert_eq!(
        read.status, 200,
        "GetPropertyValue {property}: expected 200, got {read:?}"
    );
    reported.push((
        "GetPropertyValue",
        read.member("propertyValues")[property]["propertyValue"].clone(),
    ));

    // The TABULAR branch of the same operation: a row per instance of the
    // component type, each carrying the selected property's latest value.
    let table = emulator
        .get_property_value(
            WORKSPACE,
            json!({
                "componentTypeId": SENSOR,
                "selectedProperties": [property],
                "tabularConditions": {},
            }),
        )
        .await;
    assert_eq!(
        table.status, 200,
        "the tabular GetPropertyValue for {property}: expected 200, got {table:?}"
    );
    let rows = table
        .member("tabularPropertyValues")
        .as_array()
        .and_then(|tables| tables.first())
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // `unit-1`'s component is the one that carries a written value; the
    // composite component of the same type carries its own.
    let row = rows
        .iter()
        .find(|row| !row[property].is_null())
        .cloned()
        .unwrap_or(Value::Null);
    reported.push(("the tabular GetPropertyValue", row[property].clone()));

    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": "unit-1", "componentName": "Sensor"}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "ListProperties: expected 200, got {listed:?}"
    );
    let summary = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["propertyName"] == json!(property))
        .unwrap_or_else(|| panic!("ListProperties reported no {property}: {listed:?}"));
    reported.push(("ListProperties", summary["value"].clone()));

    let entity = emulator.get_entity(WORKSPACE, "unit-1").await;
    assert_eq!(
        entity.status, 200,
        "GetEntity: expected 200, got {entity:?}"
    );
    reported.push((
        "GetEntity",
        entity.member("components")["Sensor"]["properties"][property]["value"].clone(),
    ));

    reported
}

/// RULE: every reader of a property's current value builds it the one way, so
/// the value a series last carries is what `GetPropertyValue` (keyed and
/// tabular), `ListProperties`, `GetEntity` and `ExecuteQuery` each report —
/// while the series itself is left whole for `GetPropertyValueHistory`.
#[tokio::test]
async fn every_reader_reports_the_latest_value_of_the_series() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let reference = json!({
        "entityId": "unit-1",
        "componentName": "Sensor",
        "propertyName": "Temp",
    });
    put(
        &emulator,
        reference.clone(),
        json!([
            {"time": T0, "value": {"doubleValue": 1.0}},
            {"time": T1, "value": {"doubleValue": 2.0}},
        ]),
        "a two-value series",
    )
    .await;

    let expected = json!({"doubleValue": 2.0});
    for (reader, value) in through_each_reader(&emulator, "Temp").await {
        assert_eq!(
            value["doubleValue"], expected["doubleValue"],
            "reader {reader}: expected the latest series value {expected}, got {value:?}"
        );
    }

    // A later write at a later instant moves every reader's answer with it.
    put(
        &emulator,
        reference.clone(),
        json!([{"time": T3, "value": {"doubleValue": 8.5}}]),
        "a later value",
    )
    .await;
    for (reader, value) in through_each_reader(&emulator, "Temp").await {
        assert_eq!(
            value["doubleValue"],
            json!(8.5),
            "reader {reader} after a later write: expected 8.5, got {value:?}"
        );
    }

    // A write at an EARLIER instant than the latest one does not move it: the
    // latest value is the value of the latest instant, not of the last call.
    put(
        &emulator,
        reference,
        json!([{"time": T2, "value": {"doubleValue": -1.0}}]),
        "a value at an earlier instant",
    )
    .await;
    for (reader, value) in through_each_reader(&emulator, "Temp").await {
        assert_eq!(
            value["doubleValue"],
            json!(8.5),
            "reader {reader} after a write at an earlier instant: expected 8.5, got {value:?}"
        );
    }

    // The query language reads the same stored property, so it reports the same
    // latest value.
    let queried = emulator
        .execute_query(json!({
            "workspaceId": WORKSPACE,
            "queryStatement": "SELECT e FROM EntityGraph MATCH (e)",
        }))
        .await;
    assert_eq!(
        queried.status, 200,
        "ExecuteQuery: expected 200, got {queried:?}"
    );
    let node = queried
        .member("rows")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row["rowData"].as_array().and_then(|data| data.first()).cloned())
        .find(|node| node["entityId"] == json!("unit-1"))
        .unwrap_or_else(|| panic!("ExecuteQuery reported no unit-1 node: {queried:?}"));
    let property = node["components"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|component| component["properties"].as_array().cloned())
        .flatten()
        .find(|property| property["propertyName"] == json!("Temp"))
        .unwrap_or_else(|| panic!("ExecuteQuery reported no Temp property: {queried:?}"));
    assert_eq!(
        property["propertyValue"],
        json!(8.5),
        "ExecuteQuery: expected the latest series value 8.5, got {property:?}"
    );

    // Reading the latest value changes nothing: the whole series is still
    // there, every value of it, and a second read answers the same.
    let history = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({
                "entityId": "unit-1",
                "componentName": "Sensor",
                "selectedProperties": ["Temp"],
            }),
        )
        .await;
    assert_eq!(
        history.status, 200,
        "GetPropertyValueHistory: expected 200, got {history:?}"
    );
    let series: Vec<Value> = history
        .member("propertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|one| one["values"].as_array().cloned())
        .flatten()
        .map(|value| value["value"].clone())
        .collect();
    assert_eq!(
        series,
        vec![
            json!({"doubleValue": 1.0}),
            json!({"doubleValue": 2.0}),
            json!({"doubleValue": -1.0}),
            json!({"doubleValue": 8.5}),
        ],
        "GetPropertyValueHistory after the latest-value reads: expected every written value in \
         time order, got {history:?}"
    );

    let first = read_one(&emulator, "unit-1", Some("Sensor"), None, "Temp").await;
    let second = read_one(&emulator, "unit-1", Some("Sensor"), None, "Temp").await;
    assert_eq!(
        first.body, second.body,
        "two latest-value reads of unchanged state: expected the same answer, got {first:?} then \
         {second:?}"
    );
}
