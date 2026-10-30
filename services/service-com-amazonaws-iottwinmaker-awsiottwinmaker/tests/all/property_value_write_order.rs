// SPDX-License-Identifier: CC0-1.0

//! The ORDER of the writes a property's value receives, end to end over HTTP.
//!
//! RULE under test: a property reads as the value of the MOST RECENT write it
//! received, whichever plane made it — the plane that wrote last decides, the
//! time-series data plane's value being the one of the latest INSTANT in its
//! series (`BatchPutPropertyValues`) and the control plane's the single value it
//! set (`CreateEntity`, `UpdateEntity`'s `propertyUpdates`), with a
//! `RESET_VALUE` update a write of NO value that leaves the property reading as
//! one nothing has written to — and only a property no write has reached reads
//! as the default its definition declares, or as nothing where there is no
//! default either. Every reader of a property's current value answers that one
//! way, and a control-plane write leaves the series itself whole for
//! `GetPropertyValueHistory`.
//!
//! The rows of the table below differ along what the rule generalises over: the
//! ORDER and NUMBER of the writes across the two planes (a series then a value,
//! a series then a reset, a value then a series, a series then a reset then a
//! series, a value then a reset, two values), the DATA TYPE the value travels
//! in (`DOUBLE`, `STRING`, `INTEGER`, `LONG`, `BOOLEAN`), whether the property
//! is a time series property at all, whether its definition declares a default
//! for a reset to fall back to, and WHICH component carries it (a top-level one
//! and a composite one addressed by its `componentPath`).

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator};

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "writeorder";

/// The component type whose property definitions carry one property per case,
/// so the cases are independent of each other.
const SENSOR: &str = "com.example.sensor";
/// A component type declaring a COMPOSITE component type of [`SENSOR`], so an
/// entity component of this type carries a composite component addressed by the
/// path `Rig/inner`.
const RIG: &str = "com.example.rig";

/// Instants one minute apart, as the ISO 8601 `Time` a data-plane write
/// carries. Written out rather than computed so a failing assertion names the
/// instant it is about.
const T0: &str = "2026-04-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-04-01T00:01:00Z";
/// See [`T0`].
const T2: &str = "2026-04-01T00:02:00Z";

/// One write a case makes to the property it is about, in the order the case
/// lists it.
#[derive(Debug, Clone)]
enum Write {
    /// A time-series data-plane write (`BatchPutPropertyValues`): one value FOR
    /// the instant it names.
    Series(&'static str, Value),
    /// A control-plane write (`UpdateEntity`): the one current value the
    /// `propertyUpdates` entry carries.
    Set(Value),
    /// A control-plane `RESET_VALUE` update: a write of no value at all.
    Reset,
}

/// One row of the write-order table: the property, the writes it receives and
/// the `DataValue` every reader has to report afterwards.
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
    writes: Vec<Write>,
    /// The whole `DataValue` the property reads as, or `None` where it reads as
    /// nothing at all.
    expected: Option<Value>,
}

/// The workspace, component types and entities every case below uses: one
/// entity carrying a top-level component of [`SENSOR`], and one carrying a
/// component of [`RIG`] with a composite component of [`SENSOR`] inside it.
///
/// The property definitions name one property per case, and their members are
/// the ones `PropertyDefinitionRequest` declares (`dataType`, `isTimeSeries`,
/// `defaultValue`).
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
                    "Status": {"dataType": {"type": "STRING"}, "isTimeSeries": true},
                    "Count": {"dataType": {"type": "INTEGER"}, "isTimeSeries": true},
                    "Ticks": {"dataType": {"type": "LONG"}, "isTimeSeries": true},
                    "Online": {"dataType": {"type": "BOOLEAN"}, "isTimeSeries": true},
                    "Load": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    "Drift": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    "Spare": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    // A default for a reset to fall back to.
                    "Calibration": {
                        "dataType": {"type": "DOUBLE"},
                        "isTimeSeries": true,
                        "defaultValue": {"doubleValue": 9.9},
                    },
                    "Baseline": {
                        "dataType": {"type": "DOUBLE"},
                        "isTimeSeries": true,
                        "defaultValue": {"doubleValue": 1.25},
                    },
                    "Residual": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    // Not time series properties: only the control plane writes
                    // to these.
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
            "components": {"Sensor": {"componentTypeId": SENSOR}},
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

/// The `entityPropertyReference` one case addresses its property by.
fn reference(case: &Case) -> Value {
    let mut reference = json!({"entityId": case.entity_id, "propertyName": case.property});
    if let Some(component_name) = case.component_name {
        reference["componentName"] = json!(component_name);
    }
    if let Some(component_path) = case.component_path {
        reference["componentPath"] = json!(component_path);
    }
    reference
}

/// The `componentUpdates`/`compositeComponentUpdates` entry one control-plane
/// write travels in, keyed by however the case addresses its component.
fn property_update(case: &Case, update: Value) -> Value {
    let updates = json!({"propertyUpdates": {case.property: update}});
    match (case.component_name, case.component_path) {
        (Some(component_name), _) => json!({"componentUpdates": {component_name: updates}}),
        (None, Some(component_path)) => {
            json!({"compositeComponentUpdates": {component_path: updates}})
        }
        (None, None) => panic!("case {:?} addresses no component", case.what),
    }
}

/// Make one write of a case's sequence, asserting it was served.
async fn make_write(emulator: &Emulator, case: &Case, write: &Write) {
    match write {
        Write::Series(at, value) => {
            let written = emulator
                .batch_put_property_values(
                    WORKSPACE,
                    json!([{
                        "entityPropertyReference": reference(case),
                        "propertyValues": [{"time": at, "value": value}],
                    }]),
                )
                .await;
            assert_eq!(
                written.status, 200,
                "case {:?}: BatchPutPropertyValues {value} at {at} expected 200, got {written:?}",
                case.what
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
                "case {:?}: BatchPutPropertyValues expected no errorEntries, got {written:?}",
                case.what
            );
        }
        Write::Set(value) => {
            let body = property_update(case, json!({"updateType": "UPDATE", "value": value}));
            let updated = emulator
                .update_entity(WORKSPACE, case.entity_id, body.clone())
                .await;
            assert_eq!(
                updated.status, 200,
                "case {:?}: UpdateEntity {body} expected 200, got {updated:?}",
                case.what
            );
        }
        Write::Reset => {
            let body = property_update(case, json!({"updateType": "RESET_VALUE"}));
            let updated = emulator
                .update_entity(WORKSPACE, case.entity_id, body.clone())
                .await;
            assert_eq!(
                updated.status, 200,
                "case {:?}: UpdateEntity {body} expected 200, got {updated:?}",
                case.what
            );
        }
    }
}

/// What each reader of a property's current value reports for one case, as the
/// `DataValue` object the reader carries it in (`Value::Null` where the reader
/// reports no value at all).
async fn through_each_reader(emulator: &Emulator, case: &Case) -> Vec<(&'static str, Value)> {
    let mut reported = Vec::new();

    let mut body = json!({"entityId": case.entity_id, "selectedProperties": [case.property]});
    if let Some(component_name) = case.component_name {
        body["componentName"] = json!(component_name);
    }
    if let Some(component_path) = case.component_path {
        body["componentPath"] = json!(component_path);
    }
    let read = emulator.get_property_value(WORKSPACE, body).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: GetPropertyValue expected 200, got {read:?}",
        case.what
    );
    reported.push((
        "GetPropertyValue",
        read.member("propertyValues")[case.property]["propertyValue"].clone(),
    ));

    let mut body = json!({"entityId": case.entity_id});
    if let Some(component_name) = case.component_name {
        body["componentName"] = json!(component_name);
    }
    if let Some(component_path) = case.component_path {
        body["componentPath"] = json!(component_path);
    }
    let listed = emulator.list_properties(WORKSPACE, body).await;
    assert_eq!(
        listed.status, 200,
        "case {:?}: ListProperties expected 200, got {listed:?}",
        case.what
    );
    let summary = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["propertyName"] == json!(case.property))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: ListProperties reported no {}: {listed:?}",
                case.what, case.property
            )
        });
    reported.push(("ListProperties", summary["value"].clone()));

    // `GetEntity` reports the properties of the entity's TOP-LEVEL components,
    // so a composite component's property is read through the two readers that
    // take a `componentPath`.
    if let Some(component_name) = case.component_name {
        let entity = emulator.get_entity(WORKSPACE, case.entity_id).await;
        assert_eq!(
            entity.status, 200,
            "case {:?}: GetEntity expected 200, got {entity:?}",
            case.what
        );
        reported.push((
            "GetEntity",
            entity.member("components")[component_name]["properties"][case.property]["value"]
                .clone(),
        ));
    }

    reported
}

/// RULE: a property reads as the value of the most recent write it received,
/// whichever plane made it — and a `RESET_VALUE` update is such a write, of no
/// value at all.
#[tokio::test]
async fn a_property_reads_as_the_value_of_its_latest_write() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let cases = vec![
        Case {
            what: "a control-plane value written AFTER a series is what a read reports",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Temp",
            writes: vec![
                Write::Series(T0, json!({"doubleValue": 21.5})),
                Write::Set(json!({"doubleValue": 99.0})),
            ],
            expected: Some(json!({"doubleValue": 99.0})),
        },
        Case {
            what: "a reset after a series takes the property's value away",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Status",
            writes: vec![
                Write::Series(T0, json!({"stringValue": "idle"})),
                Write::Reset,
            ],
            expected: None,
        },
        Case {
            what: "a reset after a series leaves the default the definition declares",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Calibration",
            writes: vec![
                Write::Series(T0, json!({"doubleValue": 3.3})),
                Write::Reset,
            ],
            expected: Some(json!({"doubleValue": 9.9})),
        },
        Case {
            what: "a series value written AFTER a control-plane value is what a read reports",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Count",
            writes: vec![
                Write::Set(json!({"integerValue": 1})),
                Write::Series(T1, json!({"integerValue": 7})),
            ],
            expected: Some(json!({"integerValue": 7})),
        },
        Case {
            what: "a series written after a reset is reported again",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Ticks",
            writes: vec![
                Write::Series(T0, json!({"longValue": 5})),
                Write::Reset,
                Write::Series(T1, json!({"longValue": 9})),
            ],
            expected: Some(json!({"longValue": 9})),
        },
        Case {
            what: "a control-plane value wins over the LATEST INSTANT of a several-value series",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Online",
            writes: vec![
                Write::Series(T0, json!({"booleanValue": false})),
                Write::Series(T2, json!({"booleanValue": true})),
                Write::Set(json!({"booleanValue": false})),
            ],
            expected: Some(json!({"booleanValue": false})),
        },
        Case {
            what: "with no control-plane write the series' latest INSTANT is the value",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Load",
            writes: vec![
                Write::Series(T1, json!({"doubleValue": 1.0})),
                Write::Series(T0, json!({"doubleValue": 4.0})),
            ],
            expected: Some(json!({"doubleValue": 1.0})),
        },
        Case {
            what: "a reset after a control-plane value takes it away",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Drift",
            writes: vec![Write::Set(json!({"doubleValue": 2.5})), Write::Reset],
            expected: None,
        },
        Case {
            what: "the later of two control-plane values is the value",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Spare",
            writes: vec![
                Write::Set(json!({"doubleValue": 1.0})),
                Write::Set(json!({"doubleValue": 2.0})),
            ],
            expected: Some(json!({"doubleValue": 2.0})),
        },
        Case {
            what: "a reset on a property that is NOT a time series takes its value away",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Zone",
            writes: vec![Write::Set(json!({"stringValue": "north"})), Write::Reset],
            expected: None,
        },
        Case {
            what: "a reset on a property that is not a time series leaves its default",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Label",
            writes: vec![Write::Set(json!({"stringValue": "west"})), Write::Reset],
            expected: Some(json!({"stringValue": "unset-label"})),
        },
        Case {
            what: "a property no write has reached reads as its default",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Baseline",
            writes: vec![],
            expected: Some(json!({"doubleValue": 1.25})),
        },
        Case {
            what: "a property with no write and no default reads as nothing",
            entity_id: "unit-1",
            component_name: Some("Sensor"),
            component_path: None,
            property: "Residual",
            writes: vec![],
            expected: None,
        },
        Case {
            what: "a composite component's property, addressed by its componentPath",
            entity_id: "rig-1",
            component_name: None,
            component_path: Some("Rig/inner"),
            property: "Temp",
            writes: vec![
                Write::Series(T0, json!({"doubleValue": -7.5})),
                Write::Set(json!({"doubleValue": 12.25})),
            ],
            expected: Some(json!({"doubleValue": 12.25})),
        },
    ];

    for case in &cases {
        for write in &case.writes {
            make_write(&emulator, case, write).await;
        }
        for (reader, reported) in through_each_reader(&emulator, case).await {
            match &case.expected {
                Some(expected) => assert_eq!(
                    reported, *expected,
                    "case {:?} through {reader}: expected {expected}, got {reported:?}",
                    case.what
                ),
                None => assert!(
                    reported.is_null(),
                    "case {:?} through {reader}: expected no value, got {reported:?}",
                    case.what
                ),
            }
        }
    }
}

/// RULE: a control-plane write decides only what the property's CURRENT value
/// reads as — the series is the history of the values written FOR their
/// instants, so it is left whole, and so is the definition a reset keeps.
#[tokio::test]
async fn a_control_plane_write_leaves_the_series_and_the_definition_whole() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let case = Case {
        what: "a series, then a control-plane value, then a reset, then a series again",
        entity_id: "unit-1",
        component_name: Some("Sensor"),
        component_path: None,
        property: "Temp",
        writes: vec![],
        expected: None,
    };

    /// Every value `GetPropertyValueHistory` reports for the property, in the
    /// order it reports them.
    async fn history(emulator: &Emulator, case: &Case, step: &str) -> Vec<Value> {
        let read = emulator
            .get_property_value_history(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "componentName": case.component_name,
                    "selectedProperties": [case.property],
                }),
            )
            .await;
        assert_eq!(
            read.status, 200,
            "step {step:?}: GetPropertyValueHistory expected 200, got {read:?}"
        );
        read.member("propertyValues")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|one| one["values"].as_array().cloned())
            .flatten()
            .map(|value| value["value"].clone())
            .collect()
    }

    make_write(&emulator, &case, &Write::Series(T0, json!({"doubleValue": 1.0}))).await;
    make_write(&emulator, &case, &Write::Series(T1, json!({"doubleValue": 2.0}))).await;
    let written = vec![json!({"doubleValue": 1.0}), json!({"doubleValue": 2.0})];
    assert_eq!(
        history(&emulator, &case, "two series writes").await,
        written,
        "step {:?}: expected the series to hold every written value in time order",
        "two series writes"
    );

    make_write(&emulator, &case, &Write::Set(json!({"doubleValue": 99.0}))).await;
    for (reader, reported) in through_each_reader(&emulator, &case).await {
        assert_eq!(
            reported,
            json!({"doubleValue": 99.0}),
            "step \"a control-plane value\" through {reader}: expected 99.0, got {reported:?}"
        );
    }
    assert_eq!(
        history(&emulator, &case, "a control-plane value").await,
        written,
        "step \"a control-plane value\": expected the series to be left whole"
    );

    make_write(&emulator, &case, &Write::Reset).await;
    for (reader, reported) in through_each_reader(&emulator, &case).await {
        assert!(
            reported.is_null(),
            "step \"a reset\" through {reader}: expected no value, got {reported:?}"
        );
    }
    assert_eq!(
        history(&emulator, &case, "a reset").await,
        written,
        "step \"a reset\": expected the series to be left whole"
    );

    // The declaration that gives the property meaning survives both writes.
    let entity = emulator.get_entity(WORKSPACE, case.entity_id).await;
    assert_eq!(
        entity.status, 200,
        "GetEntity after a reset: expected 200, got {entity:?}"
    );
    let definition =
        &entity.member("components")["Sensor"]["properties"][case.property]["definition"];
    assert_eq!(
        definition["dataType"]["type"],
        json!("DOUBLE"),
        "GetEntity after a reset: expected the definition to be kept, got {definition:?}"
    );
    assert_eq!(
        definition["isTimeSeries"],
        json!(true),
        "GetEntity after a reset: expected the definition to be kept, got {definition:?}"
    );

    // And a later data-plane write is reported again, series and all.
    make_write(&emulator, &case, &Write::Series(T2, json!({"doubleValue": 3.5}))).await;
    for (reader, reported) in through_each_reader(&emulator, &case).await {
        assert_eq!(
            reported,
            json!({"doubleValue": 3.5}),
            "step \"a series after a reset\" through {reader}: expected 3.5, got {reported:?}"
        );
    }
    let mut grown = written.clone();
    grown.push(json!({"doubleValue": 3.5}));
    assert_eq!(
        history(&emulator, &case, "a series after a reset").await,
        grown,
        "step \"a series after a reset\": expected every written value in time order"
    );

    // Reading the current value changes nothing a later caller sees.
    let first = through_each_reader(&emulator, &case).await;
    let second = through_each_reader(&emulator, &case).await;
    assert_eq!(
        first, second,
        "two reads of unchanged state: expected the same answers, got {first:?} then {second:?}"
    );
}
