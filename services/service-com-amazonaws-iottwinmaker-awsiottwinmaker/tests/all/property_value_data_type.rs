// SPDX-License-Identifier: CC0-1.0

//! The TYPE of a value written to a component's property, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a status code and a
//! serialized body are all exercised, not just a handler body.
//!
//! RULE under test: a value written to a property is a value of the `dataType`
//! that reaches the property — the declaration the write itself carries, else
//! the one it inherits from the component type its holder instantiates or from a
//! type that type extends — so a write carrying another type, or carrying no
//! typed value at all, is refused with a `ValidationException` and nothing is
//! stored, instead of leaving a read reporting a value of a type the property's
//! reported definition does not declare. A property nothing declares a
//! `dataType` for constrains nothing, so any value may be written to it.
//!
//! The rows below differ along what the rule generalises over: WHICH data type
//! the property declares and which one the value carries (the whole `Type` enum
//! against itself), WHERE the declaration that decides it comes from (the
//! request's own, the component type's, an extended type's, the stored one a
//! patch leaves standing, none at all), WHICH holder carries the property (a
//! top-level component and a composite one addressed by its `componentPath`),
//! and WHICH writer makes the write (`CreateEntity` and `UpdateEntity`).

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::TYPE_VALUES;

use crate::harness::Emulator;

/// The workspace every case here works in. Each test builds its own emulator,
/// so the id is shared without the cases sharing state.
const WORKSPACE: &str = "propTypeWs";

/// A component type declaring no properties at all, so a component of it
/// carries only the properties — and the declarations — its own request names.
const PLAIN_TYPE: &str = "com.example.plain";
/// The component type [`SENSOR_TYPE`] extends. Its `Serial` declaration is the
/// one a component of `SENSOR_TYPE` inherits through the chain.
const BASE_TYPE: &str = "com.example.base";
/// The component type the components instantiate: it declares `Temperature`,
/// extends [`BASE_TYPE`] and declares one composite component type, so a
/// composite path one level down resolves.
const SENSOR_TYPE: &str = "com.example.sensor";
/// The component type [`SENSOR_TYPE`]'s composite declaration names. Its
/// `Pressure` declaration is what a composite component inherits.
const INNER_TYPE: &str = "com.example.inner";

/// The property the catalog case below declares on the component itself.
const CATALOG_PROPERTY: &str = "Reading";

/// One `DataValue` carrying a value of `data_type`.
///
/// The catalog of data types is the model's own `Type` enum, read off
/// `ops::types::TYPE_VALUES` (generated from it:
/// `smithy_query(query="shape",shape="Type")` lists `RELATIONSHIP`, `STRING`,
/// `LONG`, `BOOLEAN`, `INTEGER`, `DOUBLE`, `LIST`, `MAP`), and each value here
/// travels in the `DataValue` member that type's values travel in
/// (`smithy_query(query="shape",shape="DataValue")`). A `Type` this match does
/// not cover panics rather than being skipped, so a value added to the model's
/// enum fails this suite instead of going untested.
fn value_of(data_type: &str) -> Value {
    match data_type {
        "BOOLEAN" => json!({"booleanValue": true}),
        "DOUBLE" => json!({"doubleValue": 21.5}),
        "INTEGER" => json!({"integerValue": 7}),
        "LIST" => json!({"listValue": [{"stringValue": "one"}]}),
        "LONG" => json!({"longValue": 9_007_199_254_740_993i64}),
        "MAP" => json!({"mapValue": {"key": {"stringValue": "one"}}}),
        "RELATIONSHIP" => json!({"relationshipValue": {"targetEntityId": "targetEntity"}}),
        "STRING" => json!({"stringValue": "warm"}),
        other => panic!("the model's Type enum carries {other:?}, which this suite has no value of"),
    }
}

/// The message the refusal of a value whose type is not the property's carries.
fn mistyped_message(property_name: &str, carried: &str, declared: &str) -> String {
    format!(
        "the value written to property {property_name} is a {carried} where that property's \
         dataType is {declared}"
    )
}

/// The message the refusal of a `value` member carrying no typed value carries.
fn untyped_message(property_name: &str) -> String {
    format!("the value written to property {property_name} carries no value of any data type")
}

/// An emulator holding the workspace the cases work in plus the component types
/// the components instantiate, the chain a declaration is inherited through and
/// the composite paths resolve through.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case works in must exist"
    );
    // In reference order: `extendsFrom` and `compositeComponentTypes` both have
    // to name a component type the workspace already holds.
    for (component_type_id, body) in [
        (PLAIN_TYPE, json!({})),
        (
            INNER_TYPE,
            json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            BASE_TYPE,
            json!({"propertyDefinitions": {"Serial": {"dataType": {"type": "STRING"}}}}),
        ),
        (
            SENSOR_TYPE,
            json!({
                "extendsFrom": [BASE_TYPE],
                "propertyDefinitions": {"Temperature": {"dataType": {"type": "DOUBLE"}}},
                "compositeComponentTypes": {"inner": {"componentTypeId": INNER_TYPE}},
            }),
        ),
    ] {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }
    emulator
}

/// Assert one refused write: the modelled `ValidationException` (HTTP 400)
/// carrying exactly `message`.
fn assert_refused(case: &str, refused: &crate::harness::Resp, message: &str) {
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected the modelled refusal, got {refused:?}"
    );
    assert_eq!(
        refused.member("message").as_str(),
        Some(message),
        "case {case:?}: expected the refusal to read {message:?}, got {refused:?}"
    );
}

/// RULE (the whole catalog, both ways): a property declaring a `dataType`
/// accepts a value of that type and refuses a value of every OTHER type the
/// model's `Type` enum defines, naming the type carried and the type declared.
#[tokio::test]
async fn a_property_value_must_carry_the_data_type_the_property_declares() {
    let emulator = seeded().await;

    assert!(
        TYPE_VALUES.len() >= 8,
        "the model's Type enum must still carry its values, got {TYPE_VALUES:?}"
    );
    for declared in TYPE_VALUES {
        let component = |value: Value| {
            json!({"Spec": {
                "componentTypeId": PLAIN_TYPE,
                "properties": {CATALOG_PROPERTY: {
                    "definition": {"dataType": {"type": declared}},
                    "value": value,
                }},
            }})
        };

        // The value of the declared type is stored, and read back as that
        // value against that `dataType`.
        let case = format!("{declared}.accepts{declared}");
        let entity_id = format!("catalog-{declared}");
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": case,
                    "components": component(value_of(declared)),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );
        let read = emulator.get_entity(WORKSPACE, &entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetEntity, got {read:?}"
        );
        let property = &read.member("components")["Spec"]["properties"][CATALOG_PROPERTY];
        assert_eq!(
            property["definition"]["dataType"]["type"],
            json!(declared),
            "case {case:?}: expected the declared dataType to be read back, got {read:?}"
        );
        assert_eq!(
            property["value"],
            value_of(declared),
            "case {case:?}: expected the written value to be read back, got {read:?}"
        );

        // A value of any other type is refused, and nothing is stored.
        for carried in TYPE_VALUES.iter().filter(|carried| *carried != declared) {
            let case = format!("{declared}.refuses{carried}");
            let entity_id = format!("catalog-{declared}-{carried}");
            let refused = emulator
                .create_entity(
                    WORKSPACE,
                    json!({
                        "entityId": entity_id,
                        "entityName": case,
                        "components": component(value_of(carried)),
                    }),
                )
                .await;
            assert_refused(
                &case,
                &refused,
                &mistyped_message(CATALOG_PROPERTY, carried, declared),
            );
            let read = emulator.get_entity(WORKSPACE, &entity_id).await;
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a refused create persists nothing, got {read:?}"
            );
        }
    }
}

/// What a case expects of the create it sends.
enum Expect {
    /// Refused with this message, and nothing stored.
    Refused(String),
    /// Stored: the `GetEntity` body reports `value` at this JSON pointer.
    Stored(&'static str, Value),
    /// Stored on a composite component: `ListProperties` addressed by this
    /// `componentPath` reports `value` for that property. `GetEntity` reports a
    /// composite component as a summary, which carries no properties, so a
    /// composite's own property is read back through the listing instead.
    StoredOnComposite(&'static str, &'static str, Value),
}

/// One case: its name, the members the create carries beside the ids, and what
/// the write must come to.
type DeclarationCase = (&'static str, Value, Expect);

/// RULE (where the deciding declaration comes from): the `dataType` a written
/// value is judged against is the one the property EFFECTIVELY has — the
/// declaration the request carries, else the nearest one it inherits from the
/// component type its holder instantiates or from a type that type extends —
/// and a property nothing declares a `dataType` for constrains nothing.
#[tokio::test]
async fn a_value_is_judged_against_the_declaration_that_reaches_the_property() {
    let emulator = seeded().await;

    let rows: Vec<DeclarationCase> = vec![
        (
            // The declaration the request itself carries decides.
            "ownDeclaration",
            json!({"components": {"Spec": {
                "componentTypeId": PLAIN_TYPE,
                "properties": {"Reading": {
                    "definition": {"dataType": {"type": "DOUBLE"}},
                    "value": {"stringValue": "hot"},
                }},
            }}}),
            Expect::Refused(mistyped_message("Reading", "STRING", "DOUBLE")),
        ),
        (
            // Nothing is declared by the request: the component type's own
            // declaration reaches the property and decides.
            "inheritedFromTheComponentType",
            json!({"components": {"Sensor": {
                "componentTypeId": SENSOR_TYPE,
                "properties": {"Temperature": {"value": {"stringValue": "hot"}}},
            }}}),
            Expect::Refused(mistyped_message("Temperature", "STRING", "DOUBLE")),
        ),
        (
            // One link further out: the declaration comes from the type the
            // component type EXTENDS.
            "inheritedThroughTheExtendsChain",
            json!({"components": {"Sensor": {
                "componentTypeId": SENSOR_TYPE,
                "properties": {"Serial": {"value": {"longValue": 7}}},
            }}}),
            Expect::Refused(mistyped_message("Serial", "LONG", "STRING")),
        ),
        (
            // A declaration that REFINES an inherited one carries no `dataType`
            // of its own, so the inherited one still decides.
            "refiningDeclarationKeepsTheInheritedDataType",
            json!({"components": {"Sensor": {
                "componentTypeId": SENSOR_TYPE,
                "properties": {"Temperature": {
                    "definition": {"isRequiredInEntity": true},
                    "value": {"stringValue": "hot"},
                }},
            }}}),
            Expect::Refused(mistyped_message("Temperature", "STRING", "DOUBLE")),
        ),
        (
            // The NEAREST declaration decides: one carrying its own `dataType`
            // is what the value is judged against, not the inherited one.
            "ownDeclarationOverridesTheInheritedOne",
            json!({"components": {"Sensor": {
                "componentTypeId": SENSOR_TYPE,
                "properties": {"Temperature": {
                    "definition": {"dataType": {"type": "STRING"}},
                    "value": {"stringValue": "hot"},
                }},
            }}}),
            Expect::Stored(
                "/components/Sensor/properties/Temperature/value/stringValue",
                json!("hot"),
            ),
        ),
        (
            // The same rule for a property of a COMPOSITE component, whose
            // declaration comes from the component type its path resolved to.
            "compositeInheritsItsDeclaration",
            json!({
                "components": {"Sensor": {"componentTypeId": SENSOR_TYPE}},
                "compositeComponents": {"Sensor/inner": {
                    "properties": {"Pressure": {"value": {"stringValue": "high"}}},
                }},
            }),
            Expect::Refused(mistyped_message("Pressure", "STRING", "DOUBLE")),
        ),
        (
            // The acceptance half of the composite case.
            "compositeValueOfTheInheritedType",
            json!({
                "components": {"Sensor": {"componentTypeId": SENSOR_TYPE}},
                "compositeComponents": {"Sensor/inner": {
                    "properties": {"Pressure": {"value": {"doubleValue": 2.5}}},
                }},
            }),
            Expect::StoredOnComposite("Sensor/inner", "Pressure", json!({"doubleValue": 2.5})),
        ),
        (
            // Nothing declares this property — the component type declares no
            // properties and the request declares none — so no `dataType`
            // constrains the value and it is stored as written.
            "nothingDeclaresTheProperty",
            json!({"components": {"Spec": {
                "componentTypeId": PLAIN_TYPE,
                "properties": {"Freeform": {"value": {"stringValue": "anything"}}},
            }}}),
            Expect::Stored(
                "/components/Spec/properties/Freeform/value/stringValue",
                json!("anything"),
            ),
        ),
        (
            // A `value` member carrying no typed value at all is nothing to
            // store, whatever the property declares.
            "valueCarriesNoTypedValue",
            json!({"components": {"Spec": {
                "componentTypeId": PLAIN_TYPE,
                "properties": {"Reading": {
                    "definition": {"dataType": {"type": "DOUBLE"}},
                    "value": {},
                }},
            }}}),
            Expect::Refused(untyped_message("Reading")),
        ),
        (
            // The acceptance half of the inherited case: the value of the
            // inherited type is stored and read back.
            "valueOfTheInheritedType",
            json!({"components": {"Sensor": {
                "componentTypeId": SENSOR_TYPE,
                "properties": {"Temperature": {"value": {"doubleValue": 21.5}}},
            }}}),
            Expect::Stored(
                "/components/Sensor/properties/Temperature/value/doubleValue",
                json!(21.5),
            ),
        ),
    ];

    for (case, members, expected) in rows {
        let entity_id = format!("declaration-{case}");
        let mut request = members;
        request["entityId"] = json!(entity_id);
        request["entityName"] = json!(case);
        let created = emulator.create_entity(WORKSPACE, request).await;

        match expected {
            Expect::Refused(message) => {
                assert_refused(case, &created, &message);
                let read = emulator.get_entity(WORKSPACE, &entity_id).await;
                assert_eq!(
                    (read.status, read.error_type.as_deref()),
                    (404, Some("ResourceNotFoundException")),
                    "case {case:?}: a refused create persists nothing, got {read:?}"
                );
            }
            Expect::Stored(pointer, value) => {
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected 200 from CreateEntity, got {created:?}"
                );
                let read = emulator.get_entity(WORKSPACE, &entity_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected 200 from GetEntity, got {read:?}"
                );
                assert_eq!(
                    read.body.pointer(pointer),
                    Some(&value),
                    "case {case:?}: expected {value} at {pointer}, got {read:?}"
                );
            }
            Expect::StoredOnComposite(component_path, property_name, value) => {
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected 200 from CreateEntity, got {created:?}"
                );
                let listed = emulator
                    .list_properties(
                        WORKSPACE,
                        json!({"entityId": entity_id, "componentPath": component_path}),
                    )
                    .await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected 200 from ListProperties, got {listed:?}"
                );
                let summaries = listed
                    .member("propertySummaries")
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let summary = summaries
                    .iter()
                    .find(|summary| summary["propertyName"].as_str() == Some(property_name));
                assert_eq!(
                    summary.map(|summary| &summary["value"]),
                    Some(&value),
                    "case {case:?}: expected {property_name} to read as {value}, got {listed:?}"
                );
            }
        }
    }
}

/// One patch case: its name, the `UpdateEntity` body it sends, and what the
/// patch must come to — a refusal with this message, or the `(JSON pointer,
/// value)` the read afterwards reports.
type PatchCase = (&'static str, Value, Result<(&'static str, Value), String>);

/// RULE (the other writer): a patch that writes a property's value is held to
/// the same rule, against the declaration the patch itself carries, else the
/// STORED one, else the inherited one — and a refused patch leaves the stored
/// entity exactly as it was.
#[tokio::test]
async fn an_entity_patch_holds_a_written_value_to_the_same_rule() {
    let emulator = seeded().await;

    // One entity per case, each created from the same baseline: a component of
    // the chain-extending component type carrying a stored `INTEGER`
    // declaration of its own beside the two it inherits (`Temperature` from the
    // type, `Serial` from the type it extends), plus a composite component
    // whose `Pressure` declaration is inherited.
    let baseline = json!({
        "components": {"Sensor": {
            "componentTypeId": SENSOR_TYPE,
            "properties": {
                "Reading": {
                    "definition": {"dataType": {"type": "INTEGER"}},
                    "value": {"integerValue": 1},
                },
                "Serial": {"value": {"stringValue": "sn-1"}},
            },
        }},
        "compositeComponents": {"Sensor/inner": {}},
    });

    let rows: Vec<PatchCase> = vec![
        (
            // Judged against the declaration the patch itself carries.
            "patchCarriesTheDeclaration",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Reading": {
                "definition": {"dataType": {"type": "DOUBLE"}},
                "value": {"stringValue": "hot"},
            }}}}}),
            Err(mistyped_message("Reading", "STRING", "DOUBLE")),
        ),
        (
            // No declaration in the patch: the STORED one still decides.
            "patchKeepsTheStoredDeclaration",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Reading": {
                "value": {"stringValue": "hot"},
            }}}}}),
            Err(mistyped_message("Reading", "STRING", "INTEGER")),
        ),
        (
            // A declaration the patch carries REPLACES the stored one, so the
            // value is judged against the new type, not the stored `INTEGER`.
            "patchReplacesTheStoredDeclaration",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Reading": {
                "updateType": "UPDATE",
                "definition": {"dataType": {"type": "STRING"}},
                "value": {"stringValue": "hot"},
            }}}}}),
            Ok((
                "/components/Sensor/properties/Reading/value/stringValue",
                json!("hot"),
            )),
        ),
        (
            // Nothing declared by the patch or by the property: the type the
            // component type EXTENDS declares it.
            "patchAgainstTheInheritedDeclaration",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Serial": {
                "value": {"longValue": 7},
            }}}}}),
            Err(mistyped_message("Serial", "LONG", "STRING")),
        ),
        (
            // A property the patch CREATES is judged against what it inherits.
            "patchCreatesAPropertyOfAnInheritedType",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Temperature": {
                "updateType": "CREATE",
                "value": {"booleanValue": true},
            }}}}}),
            Err(mistyped_message("Temperature", "BOOLEAN", "DOUBLE")),
        ),
        (
            // The same rule through a composite component's own patch.
            "compositePatchAgainstTheInheritedDeclaration",
            json!({"compositeComponentUpdates": {"Sensor/inner": {
                "propertyUpdates": {"Pressure": {"value": {"stringValue": "high"}}},
            }}}),
            Err(mistyped_message("Pressure", "STRING", "DOUBLE")),
        ),
        (
            // A `value` carrying no typed value at all is refused here too.
            "patchValueCarriesNoTypedValue",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Reading": {
                "value": {},
            }}}}}),
            Err(untyped_message("Reading")),
        ),
        (
            // The acceptance half: a value of the inherited type is stored.
            "patchWritesTheInheritedType",
            json!({"componentUpdates": {"Sensor": {"propertyUpdates": {"Temperature": {
                "value": {"doubleValue": 21.5},
            }}}}}),
            Ok((
                "/components/Sensor/properties/Temperature/value/doubleValue",
                json!(21.5),
            )),
        ),
    ];

    for (case, patch, expected) in rows {
        let entity_id = format!("patch-{case}");
        let mut request = baseline.clone();
        request["entityId"] = json!(entity_id);
        request["entityName"] = json!(case);
        let created = emulator.create_entity(WORKSPACE, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );

        let patched = emulator.update_entity(WORKSPACE, &entity_id, patch).await;
        match expected {
            Err(message) => {
                assert_refused(case, &patched, &message);
                // A refused patch changes nothing: the baseline reads back.
                let read = emulator.get_entity(WORKSPACE, &entity_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected 200 from GetEntity, got {read:?}"
                );
                for (pointer, value) in [
                    (
                        "/components/Sensor/properties/Reading/value/integerValue",
                        json!(1),
                    ),
                    (
                        "/components/Sensor/properties/Serial/value/stringValue",
                        json!("sn-1"),
                    ),
                ] {
                    assert_eq!(
                        read.body.pointer(pointer),
                        Some(&value),
                        "case {case:?}: a refused patch leaves {pointer} at {value}, got {read:?}"
                    );
                }
            }
            Ok((pointer, value)) => {
                assert_eq!(
                    patched.status, 200,
                    "case {case:?}: expected 200 from UpdateEntity, got {patched:?}"
                );
                let read = emulator.get_entity(WORKSPACE, &entity_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected 200 from GetEntity, got {read:?}"
                );
                assert_eq!(
                    read.body.pointer(pointer),
                    Some(&value),
                    "case {case:?}: expected {value} at {pointer}, got {read:?}"
                );
            }
        }
    }
}
