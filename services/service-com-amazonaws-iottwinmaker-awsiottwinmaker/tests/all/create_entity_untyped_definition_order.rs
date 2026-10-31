// SPDX-License-Identifier: CC0-1.0

//! `CreateEntity`'s property-definition pass: WHICH offender the refusal names
//! when one request declares several definitions the rule refuses, end to end
//! over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a status code and a
//! serialized body are all exercised, not just a handler body.
//!
//! The RULE under test: a refusal decided by walking the maps a request carries
//! names the offender that comes FIRST in the request's own key order — for this
//! operation, `components` in component-name order and then each component's
//! `properties` in property-name order, before `compositeComponents` in
//! component-path order and then each path's `properties` in property-name order
//! — so one request is answered with one message whatever order the maps it was
//! deserialized into happen to iterate in. Key order is Rust's order on `str`,
//! which is BYTE order: an uppercase initial sorts before every lowercase one,
//! digits are compared as characters rather than as numbers, and `/` sorts
//! before the alphanumerics a path segment is made of.
//!
//! The rows below span what the rule generalises over: which member carries the
//! offenders, how many of them one request carries, where they sit among
//! definitions that are perfectly legal, and the shape of the keys the ordering
//! has to decide between. The last two tests hold the two halves a refusal rule
//! needs beside the ordering: the same request repeated inside ONE process is
//! answered with the SAME message (each requested map is deserialized into a
//! fresh hash map, whose iteration order differs from the last one's), and a
//! request whose definitions all satisfy the rule is still created and read back
//! whole.

use serde_json::{json, Map, Value};

use crate::harness::Emulator;

/// The workspace every case here works in. Each test builds its own emulator,
/// so the id is shared without the cases sharing state.
const WORKSPACE: &str = "untypedOrderWs";

/// The component type the components instantiate. It declares one
/// `compositeComponentTypes` entry, so a composite path one level down resolves.
const SENSOR_TYPE: &str = "com.example.sensor";
/// The component type [`SENSOR_TYPE`]'s composite declaration names. Its own
/// `Pressure` definition is what an inheriting declaration may refine.
const INNER_TYPE: &str = "com.example.inner";
/// The property [`INNER_TYPE`] declares with a `dataType`, so a composite
/// declaration of it may omit one.
const INHERITED_PROPERTY: &str = "Pressure";
/// The property [`SENSOR_TYPE`] declares with a `dataType`, so a component
/// declaration of it may omit one.
const SENSOR_PROPERTY: &str = "Serial";

/// A property whose definition declares a member but no `dataType`, and whose
/// name no component type on the inheritance chain declares — the offender every
/// refused case here sends more than one of.
fn untyped_property() -> Value {
    json!({"definition": {"isRequiredInEntity": true}})
}

/// A property whose definition carries the `dataType` every reported
/// `PropertyDefinitionResponse` requires, so the rule accepts it.
fn typed_property() -> Value {
    json!({"definition": {"dataType": {"type": "STRING"}}})
}

/// A `components` entry instantiating [`SENSOR_TYPE`] and declaring `properties`.
fn component(properties: Value) -> Value {
    json!({"componentTypeId": SENSOR_TYPE, "properties": properties})
}

/// A JSON object built from `entries`, for a row whose map is generated rather
/// than written out.
fn object(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect::<Map<String, Value>>())
}

/// The member path the refusal names for an offending property of a component.
fn component_member(component_name: &str, property_name: &str) -> String {
    format!("components.{component_name}.properties.{property_name}.definition.dataType")
}

/// The member path the refusal names for an offending property of a composite
/// component.
fn composite_member(component_path: &str, property_name: &str) -> String {
    format!("compositeComponents.{component_path}.properties.{property_name}.definition.dataType")
}

/// An emulator holding the workspace the cases work in plus the component types
/// the components instantiate and the composite paths resolve through.
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
            json!({"propertyDefinitions": {INHERITED_PROPERTY: {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            SENSOR_TYPE,
            json!({
                "propertyDefinitions": {SENSOR_PROPERTY: {"dataType": {"type": "STRING"}}},
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

/// Assert one refused request: the modelled `ValidationException` (HTTP 400)
/// whose message names `named` — the offender first in key order — and none of
/// the `others` the same request carried.
fn assert_refusal_names(
    case: &str,
    refused: &crate::harness::Resp,
    named: &str,
    others: &[String],
) {
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected the modelled refusal, got {refused:?}"
    );
    let message = refused.member("message").as_str().unwrap_or_default();
    assert_eq!(
        message,
        format!("{named} is required"),
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

/// One refused case: its name, the request body, the member path the refusal
/// MUST name, and the member paths of the other offenders the same request
/// carries, which it must NOT name.
type OrderCase = (&'static str, Value, String, Vec<String>);

/// RULE: a `CreateEntity` declaring several definitions with no `dataType` is
/// refused for the first of them in the request's own key order — components by
/// component name then property name, then composite components by path then
/// property name.
#[tokio::test]
async fn an_entity_create_names_the_first_untyped_definition_in_key_order() {
    let emulator = seeded().await;

    let twelve: Vec<String> = (0..12).map(|index| format!("comp{index:02}")).collect();
    let rows: Vec<OrderCase> = vec![
        (
            // The boundary row: ONE offender is the first offender.
            "oneOffender",
            json!({"components": {"Sensor": component(json!({"heat": untyped_property()}))}}),
            component_member("Sensor", "heat"),
            vec![],
        ),
        (
            // Three offending components whose names sort differently from any
            // order a hash map could hand the walk.
            "componentNamesMixedCase",
            json!({
                "components": {
                    "zulu": component(json!({"p": untyped_property()})),
                    "mid": component(json!({"p": untyped_property()})),
                    "Alpha": component(json!({"p": untyped_property()})),
                },
            }),
            component_member("Alpha", "p"),
            vec![
                component_member("mid", "p"),
                component_member("zulu", "p"),
            ],
        ),
        (
            // Digits are compared as characters, in a component name as in a
            // property name: `C9` sorts before `c10`, which sorts before `c2`.
            "componentNamesWithDigits",
            json!({
                "components": {
                    "c2": component(json!({"p": untyped_property()})),
                    "c10": component(json!({"p": untyped_property()})),
                    "C9": component(json!({"p": untyped_property()})),
                },
            }),
            component_member("C9", "p"),
            vec![component_member("c10", "p"), component_member("c2", "p")],
        ),
        (
            // Several offending properties inside ONE component, beside one the
            // rule accepts, so the ordering is decided among property names.
            "propertyNamesWithinOneComponent",
            json!({
                "components": {
                    "Sensor": component(json!({
                        SENSOR_PROPERTY: typed_property(),
                        "p_2": untyped_property(),
                        "p_10": untyped_property(),
                    })),
                },
            }),
            component_member("Sensor", "p_10"),
            vec![component_member("Sensor", "p_2")],
        ),
        (
            // The offenders sit after a component that is perfectly legal, so
            // the walk has to pass a whole accepted component before refusing.
            "offendersAfterALegalComponent",
            json!({
                "components": {
                    "Aaa": component(json!({SENSOR_PROPERTY: typed_property()})),
                    "Ccc": component(json!({"p": untyped_property()})),
                    "Bbb": component(json!({"p": untyped_property()})),
                },
            }),
            component_member("Bbb", "p"),
            vec![component_member("Ccc", "p")],
        ),
        (
            // Twelve offending components, generated rather than written out,
            // so the row does not depend on how many entries one literal
            // carries.
            "twelveOffendingComponents",
            json!({
                "components": object(
                    twelve
                        .iter()
                        .map(|name| (name.clone(), component(json!({"p": untyped_property()})))),
                ),
            }),
            component_member(&twelve[0], "p"),
            twelve
                .iter()
                .skip(1)
                .map(|name| component_member(name, "p"))
                .collect(),
        ),
        (
            // The same rule through the other map, keyed by PATH: `/` (0x2F)
            // sorts before every alphanumeric, so `Sensor/inner` comes before
            // `SensorB/inner`, and an uppercase initial decides between the
            // property names within a path.
            "compositePathsAndProperties",
            json!({
                "components": {
                    "Sensor": component(json!({})),
                    "SensorB": component(json!({})),
                },
                "compositeComponents": {
                    "SensorB/inner": {"properties": {"alpha": untyped_property()}},
                    "Sensor/inner": {
                        "properties": {"zulu": untyped_property(), "Mid": untyped_property()},
                    },
                },
            }),
            composite_member("Sensor/inner", "Mid"),
            vec![
                composite_member("Sensor/inner", "zulu"),
                composite_member("SensorB/inner", "alpha"),
            ],
        ),
        (
            // Both maps offend: `components` is the member the pass walks
            // first, so the refusal names an offending component even though
            // the composite path sorts before it as a string.
            "bothMapsOffend",
            json!({
                "components": {"Sensor": component(json!({"zzz": untyped_property()}))},
                "compositeComponents": {
                    "Sensor/inner": {"properties": {"aaa": untyped_property()}},
                },
            }),
            component_member("Sensor", "zzz"),
            vec![composite_member("Sensor/inner", "aaa")],
        ),
    ];

    for (case, body, named, others) in rows {
        let entity_id = format!("entity-{case}");
        let mut request = body;
        request["entityId"] = json!(entity_id);
        request["entityName"] = json!(case);
        let refused = emulator.create_entity(WORKSPACE, request).await;
        assert_refusal_names(case, &refused, &named, &others);

        // A refused create persists nothing, so the next row starts from the
        // same store this one did.
        let read = emulator.get_entity(WORKSPACE, &entity_id).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create persists nothing, got {read:?}"
        );
    }
}

/// RULE (the determinism the ordering buys): the SAME request is answered with
/// the SAME message every time it is sent.
///
/// Each call deserializes its maps into fresh hash maps, and a hash map's
/// iteration order is a function of the seed it was built with rather than of
/// its keys, so the repeats below walk the same four offenders in a different
/// order from one another. A walk that answered with the first offender it
/// happened to meet would name a different component on some repeat; the ordered
/// walk names the first in key order on all of them.
///
/// The repeat count is a bound, not an exact figure: with four offenders a walk
/// following the map's own order would name the expected component on about a
/// quarter of the repeats, so sixteen of them leave such a walk about one chance
/// in four thousand million of passing unnoticed. A correct walk passes every
/// repeat, so the test never fails on a tree that holds the rule.
#[tokio::test]
async fn repeating_one_request_is_answered_with_one_message() {
    let emulator = seeded().await;

    // Four offenders, so a walk following the map's own order would name the
    // expected one about a quarter of the time: over the repeats below, a
    // hash-ordered walk is all but certain to name another.
    let offenders = ["Delta", "Bravo", "Alpha", "Charlie"];
    let expected = component_member("Alpha", "p");
    let repeats = 16;

    for repeat in 0..repeats {
        let case = format!("repeat{repeat:02}");
        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": format!("entity-{case}"),
                    "entityName": case,
                    "components": object(
                        offenders
                            .iter()
                            .map(|name| {
                                ((*name).to_string(), component(json!({"p": untyped_property()})))
                            }),
                    ),
                }),
            )
            .await;
        assert_refusal_names(
            &case,
            &refused,
            &expected,
            &offenders
                .iter()
                .filter(|name| **name != "Alpha")
                .map(|name| component_member(name, "p"))
                .collect::<Vec<String>>(),
        );
    }
}

/// RULE (acceptance half): walking the requested maps in key order refuses
/// nothing the rule admits — a create whose every definition carries a
/// `dataType` of its own or REFINES one it inherits is stored and read back
/// whole, through both maps at once.
#[tokio::test]
async fn an_ordered_walk_still_accepts_every_definition_the_rule_admits() {
    let emulator = seeded().await;

    let twelve: Vec<String> = (0..12).map(|index| format!("comp{index:02}")).collect();
    let mut components: Map<String, Value> = twelve
        .iter()
        .map(|name| (name.clone(), component(json!({"own": typed_property()}))))
        .collect();
    // A declaration of a property the component type declares may omit the
    // `dataType` it inherits, and several such declarations in one request are
    // all accepted: the ordered walk decides the same rule, not a stricter one.
    components.insert(
        "Sensor".to_string(),
        component(json!({
            SENSOR_PROPERTY: untyped_property(),
            "own": typed_property(),
        })),
    );

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "entity-allAdmitted",
                "entityName": "allAdmitted",
                "components": Value::Object(components),
                "compositeComponents": {
                    "Sensor/inner": {
                        "properties": {
                            INHERITED_PROPERTY: untyped_property(),
                            "own": typed_property(),
                        },
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"allAdmitted\": expected 200 from CreateEntity, got {created:?}"
    );

    let read = emulator.get_entity(WORKSPACE, "entity-allAdmitted").await;
    assert_eq!(
        read.status, 200,
        "case \"allAdmitted\": expected 200 from GetEntity, got {read:?}"
    );
    for name in &twelve {
        assert_eq!(
            read.member("components")[name]["properties"]["own"]["definition"]["dataType"]["type"]
                .as_str(),
            Some("STRING"),
            "case \"allAdmitted\": every created definition must be read back, got {read:?}"
        );
    }
    let sensor = &read.member("components")["Sensor"];
    assert_eq!(
        sensor["properties"][SENSOR_PROPERTY]["definition"]["dataType"]["type"].as_str(),
        Some("STRING"),
        "case \"allAdmitted\": a refining declaration keeps the inherited dataType, got {read:?}"
    );
    assert_eq!(
        sensor["properties"][SENSOR_PROPERTY]["definition"]["isRequiredInEntity"].as_bool(),
        Some(true),
        "case \"allAdmitted\": a refining declaration keeps the member it changed, got {read:?}"
    );
    // `GetEntity` reports a composite component as a SUMMARY, which carries no
    // properties, so the composite's own declarations are read back through
    // `ListProperties` addressed by the path the create used.
    assert_eq!(
        sensor["compositeComponents"]["inner"]["componentPath"].as_str(),
        Some("Sensor/inner"),
        "case \"allAdmitted\": the accepted composite component must be read back, got {read:?}"
    );
    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": "entity-allAdmitted", "componentPath": "Sensor/inner"}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"allAdmitted\": expected 200 from ListProperties, got {listed:?}"
    );
    let summaries = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    // The refining declaration keeps the `dataType` it inherits; the composite's
    // own declaration keeps the one it carried.
    for (property_name, data_type) in [(INHERITED_PROPERTY, "DOUBLE"), ("own", "STRING")] {
        let summary = summaries
            .iter()
            .find(|summary| summary["propertyName"].as_str() == Some(property_name));
        assert_eq!(
            summary.map(|summary| &summary["definition"]["dataType"]["type"]),
            Some(&json!(data_type)),
            "case \"allAdmitted\": composite property {property_name:?} must be read back with \
             dataType {data_type:?}, got {listed:?}"
        );
    }
}
