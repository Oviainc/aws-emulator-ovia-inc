// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a property its time-series data plane serves
//! nothing of, end to end over HTTP.
//!
//! RULE under test: a request that asks the time-series data plane for a
//! property whose effective definition does not MARK it as a time series
//! property (`isTimeSeries`) is refused as a `ValidationException`, and the
//! refusal names THAT property in one construction — "<propertyName> is not
//! marked as a time-series property." — whichever of the two data-plane
//! operations was asked, however the property's component was addressed, and
//! whatever else that property's definition declares. A property the same
//! component type DOES mark is served, so the refusal is about the mark and not
//! about the component.
//!
//! The rows of the tables below differ along what the rule generalises over:
//! WHICH property is named — its name running from the single character the
//! modelled `Name` admits (`@length(1..256)`, `^[a-zA-Z_\-0-9]+$`, from
//! `smithy_query(query="shape",shape="Name")`) to the 256 it caps at, through
//! the dash, the underscore and the digits its pattern allows — and what that
//! property's definition declares beside its data type (no `isTimeSeries`
//! member at all, an explicit `false`, a default value, a display name);
//! HOW its component is addressed (a top-level component by `componentName`, a
//! composite component by its `componentPath`, and every instance of a
//! component type in a cross-entity query); and WHAT ELSE the request selects
//! beside it (nothing, or a property that IS marked as a time series).

use serde_json::{json, Map, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "seriesmarks";

/// The component type carrying every property of the table, beside the one
/// property it marks as a time series.
const GAUGE: &str = "com.example.gauge";
/// A component type declaring a COMPOSITE component type of [`GAUGE`], so an
/// entity component of this type carries a composite component addressed by the
/// path `Assembly/inner`.
const ASSEMBLY: &str = "com.example.assembly";

/// The entity carrying a top-level [`GAUGE`] component named `Gauge`.
const GAUGE_ENTITY: &str = "gauge-1";
/// The entity carrying an [`ASSEMBLY`] component named `Assembly`, with a
/// composite [`GAUGE`] component inside it.
const ASSEMBLY_ENTITY: &str = "assembly-1";

/// The one property [`GAUGE`] marks as a time series: the data plane's positive
/// control, and the second selection the mixed rows carry.
const SERIES_PROPERTY: &str = "Reading";

/// The instant the servable write below carries.
const T0: &str = "2026-02-01T00:00:00Z";

/// One property that is NOT marked as a time series: what it is called, and the
/// definition [`GAUGE`] declares it with.
#[derive(Debug)]
struct Property {
    case: &'static str,
    name: String,
    definition: Value,
}

/// The properties the rule is asserted over — every one of them a property of
/// the addressed component, and none of them marked as a time series.
fn properties() -> Vec<Property> {
    vec![
        Property {
            case: "a STRING property whose definition declares no isTimeSeries at all",
            name: "Label".to_string(),
            definition: json!({"dataType": {"type": "STRING"}}),
        },
        Property {
            case: "a DOUBLE property whose definition marks isTimeSeries false",
            name: "Setpoint".to_string(),
            definition: json!({"dataType": {"type": "DOUBLE"}, "isTimeSeries": false}),
        },
        Property {
            case: "a BOOLEAN property carrying a default value",
            name: "Enabled".to_string(),
            definition: json!({
                "dataType": {"type": "BOOLEAN"},
                "defaultValue": {"booleanValue": true},
            }),
        },
        Property {
            case: "a LONG property carrying a display name",
            name: "TickLabel".to_string(),
            definition: json!({"dataType": {"type": "LONG"}, "displayName": "Tick label"}),
        },
        Property {
            case: "a property whose name is the one character the modelled Name admits",
            name: "L".to_string(),
            definition: json!({"dataType": {"type": "STRING"}}),
        },
        Property {
            case: "a property whose name carries the dash, the underscore and the digits its \
                   pattern allows",
            name: "Label-2_9".to_string(),
            definition: json!({"dataType": {"type": "STRING"}}),
        },
        Property {
            case: "a property whose name is the 256 characters the modelled Name caps at",
            name: "p".repeat(256),
            definition: json!({"dataType": {"type": "STRING"}}),
        },
    ]
}

/// The clause the refusal ends in: the reason it gives, after the property it
/// names.
const NOT_A_TIME_SERIES: &str = " is not marked as a time-series property.";

/// The message a refusal for `property_name` has to carry, written out here
/// rather than read from the service so the expectation is independent of the
/// construction under test.
fn expected_message(property_name: &str) -> String {
    format!("{property_name}{NOT_A_TIME_SERIES}")
}

/// The `message` an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message").as_str().unwrap_or_default().to_string()
}

/// The workspace, component types and entities every case below uses.
async fn seed(emulator: &Emulator, properties: &[Property]) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let mut definitions = Map::new();
    definitions.insert(
        SERIES_PROPERTY.to_string(),
        json!({"dataType": {"type": "DOUBLE"}, "isTimeSeries": true}),
    );
    for property in properties {
        definitions.insert(property.name.clone(), property.definition.clone());
    }
    let component_types: Vec<(&str, Value)> = vec![
        (
            GAUGE,
            json!({
                "propertyDefinitions": Value::Object(definitions),
                // The write connector a value is set THROUGH: declared so the
                // refusals below are about the time-series MARK and not about a
                // component type that could accept no write at all.
                "functions": data_writer_functions(),
            }),
        ),
        (
            ASSEMBLY,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": GAUGE}}}),
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
            "entityId": GAUGE_ENTITY,
            "entityName": GAUGE_ENTITY,
            "components": {"Gauge": {"componentTypeId": GAUGE}},
        }),
        json!({
            "entityId": ASSEMBLY_ENTITY,
            "entityName": ASSEMBLY_ENTITY,
            "components": {"Assembly": {"componentTypeId": ASSEMBLY}},
            "compositeComponents": {"Assembly/inner": {}},
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

/// One way a history read addresses the component a property belongs to: the
/// members that do the addressing, merged into the request beside the
/// selection.
#[derive(Debug)]
struct Address {
    case: &'static str,
    members: fn() -> Value,
}

/// The addressing forms `GetPropertyValueHistory` accepts.
const ADDRESSES: [Address; 3] = [
    Address {
        case: "a top-level component addressed by componentName",
        members: || json!({"entityId": GAUGE_ENTITY, "componentName": "Gauge"}),
    },
    Address {
        case: "a composite component addressed by componentPath",
        members: || json!({"entityId": ASSEMBLY_ENTITY, "componentPath": "Assembly/inner"}),
    },
    Address {
        case: "every instance of a component type, in a cross-entity query",
        members: || json!({"componentTypeId": GAUGE}),
    },
];

/// RULE: `GetPropertyValueHistory` refuses a selected property that is not
/// marked as a time series property, naming that property, however the
/// component was addressed and whatever else the request selects.
#[tokio::test]
async fn a_history_read_names_the_property_that_is_not_marked_as_a_time_series() {
    let emulator = Emulator::new();
    let properties = properties();
    seed(&emulator, &properties).await;

    // What the request selects beside the property under test: nothing, or the
    // property the component type DOES mark — the selection is a list, so a
    // request carrying a servable property beside an unservable one is still
    // refused for the unservable one.
    let selections: [(&str, &[&str]); 2] = [
        ("selected on its own", &[]),
        ("selected beside a time series property", &[SERIES_PROPERTY]),
    ];

    for property in &properties {
        for address in &ADDRESSES {
            for (selection_case, also) in selections {
                let case = format!(
                    "{} of {}, {selection_case}",
                    property.case, address.case
                );
                let mut selected: Vec<String> =
                    also.iter().map(|name| (*name).to_string()).collect();
                selected.push(property.name.clone());
                let mut body = (address.members)();
                body["selectedProperties"] = json!(selected);

                let refused = emulator.get_property_value_history(WORKSPACE, body).await;
                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {case:?}: expected 400 ValidationException, got {refused:?}"
                );
                let reported = message(&refused);
                assert_eq!(
                    reported,
                    expected_message(&property.name),
                    "case {case:?}: expected the message {:?}, got {reported:?}",
                    expected_message(&property.name)
                );
            }
        }
    }
}

/// RULE: `BatchPutPropertyValues` refuses the same property in the same
/// construction — as the `ValidationException` the operation models, the whole
/// request at the HTTP level, since a reference that resolves to no writable
/// property is not something it degrades to a partial result — however the
/// component was addressed.
#[tokio::test]
async fn a_batch_write_names_the_property_that_is_not_marked_as_a_time_series() {
    let emulator = Emulator::new();
    let properties = properties();
    seed(&emulator, &properties).await;

    // The two references an entry addresses a component by.
    const REFERENCES: [Address; 2] = [
        Address {
            case: "a top-level component addressed by componentName",
            members: || json!({"entityId": GAUGE_ENTITY, "componentName": "Gauge"}),
        },
        Address {
            case: "a composite component addressed by componentPath",
            members: || json!({"entityId": ASSEMBLY_ENTITY, "componentPath": "Assembly/inner"}),
        },
    ];

    for property in &properties {
        for reference in &REFERENCES {
            let case = format!("{} of {}", property.case, reference.case);
            let mut entity_property_reference = (reference.members)();
            entity_property_reference["propertyName"] = json!(property.name);
            let entry = json!({
                "entityPropertyReference": entity_property_reference,
                "propertyValues": [{"time": T0, "value": {"stringValue": "written"}}],
            });

            let written = emulator
                .batch_put_property_values(WORKSPACE, json!([entry]))
                .await;
            assert_eq!(
                (written.status, written.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected 400 ValidationException, got {written:?}"
            );
            let reported = message(&written);
            assert_eq!(
                reported,
                expected_message(&property.name),
                "case {case:?}: expected the message {:?}, got {reported:?}",
                expected_message(&property.name)
            );
        }
    }
}

/// RULE (the other side of it): the property the same component type DOES mark
/// as a time series is written and read back, so the refusal above is about the
/// MARK and not about the component, the entity or the property name.
#[tokio::test]
async fn the_property_that_is_marked_as_a_time_series_is_served() {
    let emulator = Emulator::new();
    let properties = properties();
    seed(&emulator, &properties).await;

    for address in &ADDRESSES {
        let case = format!("{SERIES_PROPERTY} of {}", address.case);
        let mut body = (address.members)();
        body["selectedProperties"] = json!([SERIES_PROPERTY]);
        let read = emulator.get_property_value_history(WORKSPACE, body).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from the history, got {read:?}"
        );
    }

    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": GAUGE_ENTITY,
                    "componentName": "Gauge",
                    "propertyName": SERIES_PROPERTY,
                },
                "propertyValues": [{"time": T0, "value": {"doubleValue": 12.5}}],
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "a write to the marked property: expected 200, got {written:?}"
    );
    assert_eq!(
        written.member("errorEntries"),
        &json!([]),
        "a write to the marked property earns no error, got {written:?}"
    );

    let read = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({
                "entityId": GAUGE_ENTITY,
                "componentName": "Gauge",
                "selectedProperties": [SERIES_PROPERTY],
            }),
        )
        .await;
    assert_eq!(
        read.status, 200,
        "reading the marked property back: expected 200, got {read:?}"
    );
    let values: Vec<Value> = read
        .member("propertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|series| series["values"].as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(
        values,
        vec![json!({"time": T0, "value": {"doubleValue": 12.5}})],
        "reading the marked property back: expected the written value, got {read:?}"
    );
}
