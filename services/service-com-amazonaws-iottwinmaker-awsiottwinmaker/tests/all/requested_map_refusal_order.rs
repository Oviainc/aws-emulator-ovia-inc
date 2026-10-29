// SPDX-License-Identifier: CC0-1.0

//! Which entry a refusal names when a request map carries SEVERAL offenders,
//! end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`,
//! the same entry the served emulator uses — so a route, a label, a status code
//! and a serialized body are all exercised, not just a handler body.
//!
//! The RULE under test: a rule that refuses a request by walking one of the maps
//! the request carries reports the offender that comes FIRST in key order, so
//! one request is answered with one message however the map was deserialized.
//! The modelled request maps arrive as a `std::collections::HashMap`, whose
//! iteration order is neither key order nor stable from one process to the next,
//! so every writer takes a requested map through
//! `state::requested_in_key_order` before a refusal is decided over it.
//!
//! The suite holds the rule for the writers that walk a requested map and can
//! meet more than one offender in it: `CreateComponentType`'s and
//! `UpdateComponentType`'s `propertyDefinitions`, `CreateEntity`'s `components`
//! and `compositeComponents`, and `UpdateEntity`'s `componentUpdates`. Key
//! order is Rust's order on `str`, which is BYTE order — so uppercase keys sort
//! before lowercase ones and digits are compared as characters, not as numbers —
//! and the rows below span that as well as the number of offenders and their
//! position among entries that are perfectly legal.

use serde_json::{json, Map, Value};

use crate::harness::Emulator;

/// The workspace every case in this suite works in. Each test builds its own
/// emulator, so the id is shared without the cases sharing state.
const WORKSPACE: &str = "mapOrderWs";

/// The component type the entity cases' components instantiate. It declares one
/// `compositeComponentTypes` entry, so a composite path one level down resolves.
const SENSOR_TYPE: &str = "com.example.sensor";
/// The component type [`SENSOR_TYPE`]'s composite declaration names.
const INNER_TYPE: &str = "com.example.inner";

/// A property definition carrying no `dataType` — the offender every case here
/// sends more than one of. It names another member, so it is a definition the
/// request really declares rather than an empty object.
fn untyped() -> Value {
    json!({"isRequiredInEntity": true})
}

/// A property definition a writer accepts: one carrying the `dataType` every
/// reported `PropertyDefinitionResponse` requires.
fn typed() -> Value {
    json!({"dataType": {"type": "STRING"}})
}

/// A property a `CreateEntity` declares inline whose definition carries no
/// `dataType`.
fn untyped_property() -> Value {
    json!({"definition": {"isRequiredInEntity": true}})
}

/// A JSON object built from `entries`, for a row whose map is generated rather
/// than written out.
fn object(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect::<Map<String, Value>>())
}

/// An emulator holding the workspace the cases work in plus the component types
/// the entity cases instantiate and resolve their composite paths through.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case works in must exist"
    );
    // In reference order: a component type's `compositeComponentTypes` has to
    // name one the workspace already holds.
    for (component_type_id, body) in [
        (
            INNER_TYPE,
            json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            SENSOR_TYPE,
            json!({
                "propertyDefinitions": {"Serial": {"dataType": {"type": "STRING"}}},
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

/// One refused case: its name, the request body, the member path the refusal
/// MUST name (the offender first in key order), and the member paths of the
/// other offenders the same request carries, which the refusal must NOT name.
type OrderCase = (&'static str, Value, String, Vec<String>);

/// Assert one refused case: the modelled `ValidationException` (HTTP 400) whose
/// message names the first offender in key order and none of the later ones.
fn assert_refusal_names(case: &str, refused: &crate::harness::Resp, named: &str, others: &[String]) {
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected the modelled refusal, got {refused:?}"
    );
    let message = refused.member("message").as_str().unwrap_or_default();
    assert!(
        message.contains(named),
        "case {case:?}: the refusal must name the offender first in key order ({named:?}), \
         got {message:?}"
    );
    for later in others {
        assert!(
            !message.contains(later.as_str()),
            "case {case:?}: the refusal must not name {later:?}, which is not the first \
             offender in key order, got {message:?}"
        );
    }
}

/// RULE: `CreateComponentType` refuses a request carrying several property
/// definitions without a `dataType` on the one whose property name comes first
/// in key order.
///
/// The rows differ along how many offenders the request carries (one, two,
/// three, twelve), where they sit among definitions that are perfectly legal,
/// whether the request writes them in key order or against it, and along the
/// shape of the keys — mixed case, digits compared as characters, separators —
/// because that is what the rule generalises over.
#[tokio::test]
async fn a_component_type_create_names_the_first_untyped_definition_in_key_order() {
    let emulator = seeded().await;

    let member = |name: &str| format!("propertyDefinitions.{name}.dataType");
    let many: Vec<(String, Value)> = (0..12)
        .map(|index| (format!("offender{index:02}"), untyped()))
        .collect();
    let rows: Vec<OrderCase> = vec![
        (
            // The boundary row: ONE offender is the first offender.
            "single",
            json!({"propertyDefinitions": {"untyped": untyped()}}),
            member("untyped"),
            vec![],
        ),
        (
            // Written against key order, so a walk that followed the request's
            // own order would name `zulu`.
            "twoAgainstKeyOrder",
            json!({"propertyDefinitions": {"zulu": untyped(), "alpha": untyped()}}),
            member("alpha"),
            vec![member("zulu")],
        ),
        (
            // The offenders sit AFTER definitions that are legal, so the walk
            // has to pass legal entries before it refuses.
            "amongTypedDefinitions",
            json!({
                "propertyDefinitions": {
                    "serial": typed(),
                    "pressure": typed(),
                    "mid": untyped(),
                    "beta": untyped(),
                },
            }),
            member("beta"),
            vec![member("mid")],
        ),
        (
            // Key order is BYTE order on `str`, so an uppercase initial sorts
            // before every lowercase one.
            "mixedCase",
            json!({
                "propertyDefinitions": {
                    "alpha": untyped(),
                    "Zeta": untyped(),
                    "Beta": untyped(),
                },
            }),
            member("Beta"),
            vec![member("Zeta"), member("alpha")],
        ),
        (
            // Digits are compared as characters too: `p_10` sorts before `p_2`.
            "digitsAreCharacters",
            json!({"propertyDefinitions": {"p_2": untyped(), "p_10": untyped()}}),
            member("p_10"),
            vec![member("p_2")],
        ),
        (
            // Twelve offenders, generated rather than written out, so the row
            // does not depend on how many entries one literal carries.
            "twelveOffenders",
            json!({"propertyDefinitions": object(many.clone())}),
            member("offender00"),
            many.iter()
                .skip(1)
                .map(|(name, _)| member(name))
                .collect(),
        ),
    ];

    for (case, body, named, others) in rows {
        let component_type_id = format!("com.example.create.{case}");
        let refused = emulator
            .create_component_type(WORKSPACE, &component_type_id, body)
            .await;
        assert_refusal_names(case, &refused, &named, &others);

        // A refused create persists nothing, so the next row starts from the
        // same store this one did.
        let read = emulator
            .get_component_type(WORKSPACE, &component_type_id)
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create persists nothing, got {read:?}"
        );
    }
}

/// RULE (the same rule through the other writer of the member): a create and an
/// update of a component type refuse the SAME set of definitions without a
/// `dataType` by naming the same property — the one first in key order.
#[tokio::test]
async fn both_writers_of_property_definitions_name_the_same_first_offender() {
    let emulator = seeded().await;

    let offenders = json!({"zulu": untyped(), "Mid": untyped(), "alpha": untyped()});
    // Byte order over the three keys above: `Mid` < `alpha` < `zulu`.
    let expected = "propertyDefinitions.Mid.dataType";

    let created = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.pair",
            json!({"propertyDefinitions": {"alpha": typed(), "zulu": typed(), "Mid": typed()}}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"pair\": the component type the update patches must exist, got {created:?}"
    );

    let refused_update = emulator
        .update_component_type(
            WORKSPACE,
            "com.example.pair",
            json!({"propertyDefinitions": offenders.clone()}),
        )
        .await;
    assert_refusal_names(
        "update.threeOffenders",
        &refused_update,
        expected,
        &[
            "propertyDefinitions.alpha.dataType".to_string(),
            "propertyDefinitions.zulu.dataType".to_string(),
        ],
    );

    let refused_create = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.pairCreate",
            json!({"propertyDefinitions": offenders}),
        )
        .await;
    assert_refusal_names(
        "create.threeOffenders",
        &refused_create,
        expected,
        &[
            "propertyDefinitions.alpha.dataType".to_string(),
            "propertyDefinitions.zulu.dataType".to_string(),
        ],
    );

    let update_message = refused_update.member("message").as_str().unwrap_or_default();
    let create_message = refused_create.member("message").as_str().unwrap_or_default();
    assert!(
        update_message.contains(expected) && create_message.contains(expected),
        "case \"bothWriters\": both writers must name {expected:?}, got {create_message:?} \
         and {update_message:?}"
    );

    // The refused update left the stored definitions exactly as the create
    // wrote them.
    let read = emulator
        .get_component_type(WORKSPACE, "com.example.pair")
        .await;
    assert_eq!(
        read.status, 200,
        "case \"bothWriters\": the component type must still be readable, got {read:?}"
    );
    for property_name in ["alpha", "zulu", "Mid"] {
        assert_eq!(
            read.member("propertyDefinitions")[property_name]["dataType"]["type"].as_str(),
            Some("STRING"),
            "case \"bothWriters\": a refused update changes nothing, got {read:?}"
        );
    }
}

/// RULE: `CreateEntity` refuses a request declaring several properties whose
/// definitions carry no `dataType` on the first offender in key order — the
/// first component in component-name order, then the first property in
/// property-name order inside it, and the same for the composite components it
/// declares.
///
/// The rows differ along which member carries the offenders (`components`,
/// `compositeComponents`), along how many entries of that member offend, along
/// whether the offending property sits beside legal ones, and along the key
/// shapes the ordering has to decide between.
#[tokio::test]
async fn an_entity_create_names_the_first_untyped_property_in_key_order() {
    let emulator = seeded().await;

    let component = |properties: Value| {
        json!({"componentTypeId": SENSOR_TYPE, "properties": properties})
    };
    let rows: Vec<OrderCase> = vec![
        (
            // Two offending components, written against key order.
            "components.twoAgainstKeyOrder",
            json!({
                "components": {
                    "Zulu": component(json!({"beta": untyped_property()})),
                    "Alpha": component(json!({"beta": untyped_property()})),
                },
            }),
            "components.Alpha.properties.beta.definition.dataType".to_string(),
            vec!["components.Zulu.properties.beta.definition.dataType".to_string()],
        ),
        (
            // One component, several offending properties among legal ones.
            "components.propertiesWithinOneComponent",
            json!({
                "components": {
                    "Sensor": component(json!({
                        "Serial": {"definition": {"dataType": {"type": "STRING"}}},
                        "zebra": untyped_property(),
                        "mango": untyped_property(),
                    })),
                },
            }),
            "components.Sensor.properties.mango.definition.dataType".to_string(),
            vec!["components.Sensor.properties.zebra.definition.dataType".to_string()],
        ),
        (
            // The offending component sits after a legal one in key order, so
            // the walk passes a whole legal component first.
            "components.afterALegalComponent",
            json!({
                "components": {
                    "Beta": component(json!({"Serial": {"definition": {"dataType": {"type": "STRING"}}}})),
                    "Gamma": component(json!({"g2": untyped_property(), "g1": untyped_property()})),
                },
            }),
            "components.Gamma.properties.g1.definition.dataType".to_string(),
            vec!["components.Gamma.properties.g2.definition.dataType".to_string()],
        ),
        (
            // The composite components carry the same rule, keyed by PATH.
            "compositeComponents.twoPaths",
            json!({
                "components": {"Sensor": component(json!({}))},
                "compositeComponents": {
                    "Sensor/inner": {"properties": {"zulu": untyped_property()}},
                    "Sensor/Inner": {"properties": {"alpha": untyped_property()}},
                },
            }),
            "compositeComponents.Sensor/Inner.properties.alpha.definition.dataType".to_string(),
            vec!["compositeComponents.Sensor/inner.properties.zulu.definition.dataType".to_string()],
        ),
    ];

    for (case, body, named, others) in rows {
        let entity_id = format!("entity-{}", case.replace('.', "-"));
        let mut request = body;
        request["entityId"] = json!(entity_id);
        request["entityName"] = json!(entity_id);
        let refused = emulator.create_entity(WORKSPACE, request).await;
        assert_refusal_names(case, &refused, &named, &others);

        let read = emulator.get_entity(WORKSPACE, &entity_id).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create persists nothing, got {read:?}"
        );
    }
}

/// RULE: `UpdateEntity` refuses a patch whose `componentUpdates` carry several
/// property definitions without a `dataType` on the first offender in key order
/// — the first component in component-name order, then the first property in
/// property-name order inside it.
#[tokio::test]
async fn an_entity_update_names_the_first_untyped_property_update_in_key_order() {
    let emulator = seeded().await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "patched",
                "entityName": "the entity the patches address",
                "components": {
                    "Sensor": {
                        "componentTypeId": SENSOR_TYPE,
                        "properties": {
                            "Serial": {
                                "definition": {"dataType": {"type": "STRING"}},
                                "value": {"stringValue": "sn-1"},
                            },
                        },
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"patched\": the entity the patches address must exist, got {created:?}"
    );

    let member = |component: &str, property: &str| {
        format!("componentUpdates.{component}.propertyUpdates.{property}.definition.dataType")
    };
    let added = |properties: Value| {
        json!({
            "componentTypeId": SENSOR_TYPE,
            "updateType": "CREATE",
            "propertyUpdates": properties,
        })
    };
    let rows: Vec<OrderCase> = vec![
        (
            // Two offending component entries, written against key order.
            "componentUpdates.twoAgainstKeyOrder",
            json!({
                "componentUpdates": {
                    "Zulu": added(json!({"beta": untyped_property()})),
                    "Alpha": added(json!({"beta": untyped_property()})),
                },
            }),
            member("Alpha", "beta"),
            vec![member("Zulu", "beta")],
        ),
        (
            // Several offending property updates inside the component the
            // entity already holds, beside one that is legal.
            "propertyUpdates.withinTheStoredComponent",
            json!({
                "componentUpdates": {
                    "Sensor": {
                        "propertyUpdates": {
                            "Serial": {"definition": {"dataType": {"type": "STRING"}}},
                            "zebra": untyped_property(),
                            "Mango": untyped_property(),
                        },
                    },
                },
            }),
            member("Sensor", "Mango"),
            vec![member("Sensor", "zebra")],
        ),
    ];

    for (case, body, named, others) in rows {
        let refused = emulator.update_entity(WORKSPACE, "patched", body).await;
        assert_refusal_names(case, &refused, &named, &others);

        // A refused update changes nothing: the entity still carries exactly
        // what the create wrote.
        let read = emulator.get_entity(WORKSPACE, "patched").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the entity must still be readable, got {read:?}"
        );
        assert_eq!(
            read.member("components")["Sensor"]["properties"]["Serial"]["value"]["stringValue"]
                .as_str(),
            Some("sn-1"),
            "case {case:?}: a refused update changes nothing, got {read:?}"
        );
        assert!(
            read.member("components")["Alpha"].is_null()
                && read.member("components")["Zulu"].is_null(),
            "case {case:?}: a refused update adds no component, got {read:?}"
        );
    }
}

/// RULE (acceptance half): walking a requested map in key order refuses nothing
/// a hash-ordered walk would have accepted — a request whose definitions all
/// satisfy the rule is created and read back whole, however many entries it
/// carries and in whatever order it writes them.
#[tokio::test]
async fn an_ordered_walk_still_accepts_every_definition_the_rule_admits() {
    let emulator = seeded().await;

    // The same twelve keys the refusal row generates, every one of them typed:
    // the ordered walk has to pass all of them.
    let typed_many: Vec<(String, Value)> = (0..12)
        .map(|index| (format!("offender{index:02}"), typed()))
        .collect();
    let created = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.allTyped",
            json!({"propertyDefinitions": object(typed_many.clone())}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"allTyped\": expected 200 from CreateComponentType, got {created:?}"
    );
    let read = emulator
        .get_component_type(WORKSPACE, "com.example.allTyped")
        .await;
    assert_eq!(
        read.status, 200,
        "case \"allTyped\": expected 200 from GetComponentType, got {read:?}"
    );
    for (property_name, _) in &typed_many {
        assert_eq!(
            read.member("propertyDefinitions")[property_name]["dataType"]["type"].as_str(),
            Some("STRING"),
            "case \"allTyped\": every created definition must be read back, got {read:?}"
        );
    }

    // A definition may omit `dataType` where it REFINES one it inherits, and
    // several such definitions in one request are all accepted: the ordered walk
    // decides the same rule, not a stricter one.
    let refining = emulator
        .create_component_type(
            WORKSPACE,
            "com.example.refining",
            json!({
                "extendsFrom": [SENSOR_TYPE],
                "propertyDefinitions": {
                    "zulu": {"dataType": {"type": "DOUBLE"}},
                    "Serial": untyped(),
                },
            }),
        )
        .await;
    assert_eq!(
        refining.status, 200,
        "case \"refining\": a definition refining an inherited one is accepted, got {refining:?}"
    );
    let read = emulator
        .get_component_type(WORKSPACE, "com.example.refining")
        .await;
    assert_eq!(
        read.member("propertyDefinitions")["Serial"]["dataType"]["type"].as_str(),
        Some("STRING"),
        "case \"refining\": the refining definition keeps the inherited dataType, got {read:?}"
    );
    assert_eq!(
        read.member("propertyDefinitions")["Serial"]["isRequiredInEntity"].as_bool(),
        Some(true),
        "case \"refining\": the refining definition keeps the member it changed, got {read:?}"
    );
}
