// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the INTERFACE RELATIONSHIP slice —
//! `PutAssetModelInterfaceRelationship`, which applies an interface asset model
//! to an asset model, and the `DescribeAssetModelInterfaceRelationship` that
//! reads the application back — driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * applying an interface resolves exactly ONE mapping per property the
//!   interface asset model declares, and the mapping's target is the property an
//!   override names, the property of the same name when matching by property
//!   name was asked for, or a property created on the applied-to model when
//!   creating the missing property was asked for and that model declares no
//!   property of that name — a property none of the three resolves is the
//!   modelled refusal, not a partial application;
//! * a hierarchy mapping pairs the interface's hierarchy with the applied-to
//!   model's hierarchy of the same name, and nothing else;
//! * the relationship is PERSISTED: the describe reports the mappings the
//!   application resolved to, addressed by either `CustomID` form, re-applying
//!   the same interface keeps the pairings it resolved, and a pair no
//!   application related is a modelled not-found rather than an empty answer;
//! * applying an interface changes the applied-to model, so it carries a new
//!   version and a transition back to `ACTIVE` that the clock owes.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, is_modelled_id, router};

/// Long enough for the transition the clock owes (`ACTIVATION_DELAY`) to fall
/// due, with room for a slow machine.
const PAST_THE_OWED_TRANSITION: Duration = Duration::from_millis(900);

/// The route both interface-relationship operations address, for the pair of
/// `CustomID` labels `asset_model` and `interface`.
fn relationship_uri(asset_model: &str, interface: &str) -> String {
    format!("/asset-models/{asset_model}/interface/{interface}/asset-model-interface-relationship")
}

/// One measurement property definition under `name`.
fn property(name: &str) -> Value {
    json!({ "name": name, "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}} })
}

/// Create an asset model from `body` and return its id.
async fn create_model(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(status, 202, "case {case}: create model got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no assetModelId in {created}"))
        .to_string()
}

/// Create an asset model of `model_type` carrying one measurement property per
/// name in `properties`.
async fn model_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    model_type: &str,
    properties: &[&str],
) -> String {
    create_model(
        router,
        case,
        json!({
            "assetModelName": name,
            "assetModelType": model_type,
            "assetModelProperties": properties.iter().map(|name| property(name)).collect::<Vec<Value>>(),
        }),
    )
    .await
}

/// The `assetModelProperties` / `assetModelHierarchies` of a stored model, as
/// `DescribeAssetModel` reports them.
async fn describe_model(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/asset-models/{id}"), None).await;
    assert_eq!(status, 200, "case {case}: DescribeAssetModel got {status} {body}");
    body
}

/// The id of the property named `name` in an `assetModelProperties` list.
fn id_of(properties: &Value, case: &str, name: &str) -> String {
    properties
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a property list, got {properties}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("case {case}: no property named {name} in {properties}"))["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: property {name} has no id in {properties}"))
        .to_string()
}

/// Every id in an `assetModelProperties` list.
fn ids(properties: &Value) -> BTreeSet<String> {
    properties
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|property| property["id"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The name the property `id` carries in an `assetModelProperties` list.
fn name_of(properties: &Value, id: &str) -> Option<String> {
    properties.as_array()?.iter().find_map(|property| {
        (property["id"].as_str() == Some(id))
            .then(|| property["name"].as_str().map(ToString::to_string))
            .flatten()
    })
}

/// What a case expects one interface property to map to.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The applied-to model's own property of this name, as it stood before the
    /// interface was applied.
    Held(&'static str),
    /// A property this application created on the applied-to model, carrying the
    /// interface property's name.
    Created(&'static str),
}

/// One row of the mapping table: how an interface's properties are mapped onto
/// an asset model's, under one `propertyMappingConfiguration`.
#[derive(Debug)]
struct Case {
    name: &'static str,
    /// The properties the INTERFACE asset model declares.
    interface_properties: &'static [&'static str],
    /// The properties the applied-to asset model declares.
    model_properties: &'static [&'static str],
    match_by_property_name: bool,
    create_missing_property: bool,
    /// `(interface property name, applied-to property name)` pairs the request
    /// overrides the mapping with.
    overrides: &'static [(&'static str, &'static str)],
    /// What each interface property is expected to map to, or an empty list for
    /// a request the rule refuses.
    expected: &'static [(&'static str, Target)],
    /// The modelled error code a refused row answers with, or `""`.
    refusal: &'static str,
}

/// The table: one row per way the documented configuration resolves a mapping,
/// plus the row where none of them does.
const CASES: &[Case] = &[
    Case {
        name: "matched by property name onto the properties the model already held",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["Temperature", "Pressure", "Serial"],
        match_by_property_name: true,
        create_missing_property: false,
        overrides: &[],
        expected: &[
            ("Temperature", Target::Held("Temperature")),
            ("Pressure", Target::Held("Pressure")),
        ],
        refusal: "",
    },
    Case {
        name: "every interface property created on a model that holds none of them",
        interface_properties: &["Temperature", "Pressure", "Vibration"],
        model_properties: &["Serial"],
        match_by_property_name: true,
        create_missing_property: true,
        overrides: &[],
        expected: &[
            ("Temperature", Target::Created("Temperature")),
            ("Pressure", Target::Created("Pressure")),
            ("Vibration", Target::Created("Vibration")),
        ],
        refusal: "",
    },
    Case {
        name: "the name match takes what is held and creation covers the rest",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["Temperature"],
        match_by_property_name: true,
        create_missing_property: true,
        overrides: &[],
        expected: &[
            ("Temperature", Target::Held("Temperature")),
            ("Pressure", Target::Created("Pressure")),
        ],
        refusal: "",
    },
    Case {
        name: "overrides alone map differently named properties",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["InletTemp", "InletPressure"],
        match_by_property_name: false,
        create_missing_property: false,
        overrides: &[("Temperature", "InletTemp"), ("Pressure", "InletPressure")],
        expected: &[
            ("Temperature", Target::Held("InletTemp")),
            ("Pressure", Target::Held("InletPressure")),
        ],
        refusal: "",
    },
    Case {
        name: "an override wins over the property of the same name",
        interface_properties: &["Temperature"],
        model_properties: &["Temperature", "InletTemp"],
        match_by_property_name: true,
        create_missing_property: true,
        overrides: &[("Temperature", "InletTemp")],
        expected: &[("Temperature", Target::Held("InletTemp"))],
        refusal: "",
    },
    Case {
        name: "creation alone, onto a model that declares none of the interface's names",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["Serial"],
        match_by_property_name: false,
        create_missing_property: true,
        overrides: &[],
        expected: &[
            ("Temperature", Target::Created("Temperature")),
            ("Pressure", Target::Created("Pressure")),
        ],
        refusal: "",
    },
    Case {
        name: "creation that would duplicate a name the model already declares is refused",
        interface_properties: &["Temperature"],
        model_properties: &["Temperature"],
        match_by_property_name: false,
        create_missing_property: true,
        overrides: &[],
        expected: &[],
        refusal: "InvalidRequestException",
    },
    Case {
        name: "a property no override, no name match and no creation resolves is refused",
        interface_properties: &["Temperature", "Pressure"],
        model_properties: &["Temperature", "Pressure"],
        match_by_property_name: false,
        create_missing_property: false,
        overrides: &[],
        expected: &[],
        refusal: "InvalidRequestException",
    },
];

#[tokio::test]
async fn applying_an_interface_resolves_one_mapping_per_interface_property() {
    for case in CASES {
        let router = router();
        let name = case.name;
        let interface = model_with(
            &router,
            name,
            "InterfaceModel",
            "INTERFACE",
            case.interface_properties,
        )
        .await;
        let asset_model =
            model_with(&router, name, "AppliedTo", "ASSET_MODEL", case.model_properties).await;

        let interface_properties = describe_model(&router, name, &interface).await
            ["assetModelProperties"]
            .clone();
        let before = describe_model(&router, name, &asset_model).await["assetModelProperties"]
            .clone();

        let overrides: Vec<Value> = case
            .overrides
            .iter()
            .map(|(interface_property, model_property)| {
                json!({
                    "interfaceAssetModelPropertyId": id_of(&interface_properties, name, interface_property),
                    "assetModelPropertyId": id_of(&before, name, model_property),
                })
            })
            .collect();
        let request = json!({
            "propertyMappingConfiguration": {
                "matchByPropertyName": case.match_by_property_name,
                "createMissingProperty": case.create_missing_property,
                "overrides": overrides,
            }
        });

        let (status, applied) = call(
            &router,
            "PUT",
            &relationship_uri(&asset_model, &interface),
            Some(request),
        )
        .await;

        if !case.refusal.is_empty() {
            assert_eq!(
                status, 400,
                "case {name:?}: expected the modelled refusal, got {status} {applied:?}"
            );
            assert!(
                error_code(&applied).contains(case.refusal),
                "case {name:?}: expected {}, got {:?}",
                case.refusal,
                error_code(&applied)
            );
            // A refused application relates nothing, so the read still refuses.
            let (status, read) = call(
                &router,
                "GET",
                &relationship_uri(&asset_model, &interface),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {name:?}: a refused application must relate nothing, got {status} {read:?}"
            );
            continue;
        }

        assert_eq!(
            status, 202,
            "case {name:?}: PutAssetModelInterfaceRelationship got {status} {applied:?}"
        );
        assert_eq!(
            applied["assetModelId"], json!(asset_model),
            "case {name:?}: expected assetModelId {asset_model}, got {:?}",
            applied["assetModelId"]
        );
        assert_eq!(
            applied["interfaceAssetModelId"], json!(interface),
            "case {name:?}: expected interfaceAssetModelId {interface}, got {:?}",
            applied["interfaceAssetModelId"]
        );

        let (status, read) = call(
            &router,
            "GET",
            &relationship_uri(&asset_model, &interface),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeAssetModelInterfaceRelationship got {status} {read:?}"
        );

        let after =
            describe_model(&router, name, &asset_model).await["assetModelProperties"].clone();
        let held_ids = ids(&before);
        let mappings = read["propertyMappings"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: no propertyMappings in {read:?}"))
            .clone();
        assert_eq!(
            mappings.len(),
            case.expected.len(),
            "case {name:?}: expected {} property mappings, got {mappings:?}",
            case.expected.len()
        );

        for (interface_property, target) in case.expected {
            let interface_property_id = id_of(&interface_properties, name, interface_property);
            let mapped = mappings
                .iter()
                .find(|mapping| {
                    mapping["interfaceAssetModelPropertyId"] == json!(interface_property_id)
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {name:?}: no mapping for the interface property \
                         {interface_property} ({interface_property_id}) in {mappings:?}"
                    )
                });
            let mapped_id = mapped["assetModelPropertyId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {name:?}: mapping {mapped:?} carries no assetModelPropertyId")
                })
                .to_string();
            assert!(
                is_modelled_id(&mapped_id),
                "case {name:?}: {mapped_id} is not a modelled ID"
            );
            match target {
                Target::Held(held) => {
                    let expected = id_of(&before, name, held);
                    assert_eq!(
                        mapped_id, expected,
                        "case {name:?}: expected {interface_property} to map to the held \
                         property {held} ({expected}), got {mapped_id}"
                    );
                }
                Target::Created(created) => {
                    assert!(
                        !held_ids.contains(&mapped_id),
                        "case {name:?}: expected {interface_property} to map to a CREATED \
                         property, got the held {mapped_id}"
                    );
                    assert_eq!(
                        name_of(&after, &mapped_id).as_deref(),
                        Some(*created),
                        "case {name:?}: expected the created property {mapped_id} to be named \
                         {created}, got {:?}",
                        name_of(&after, &mapped_id)
                    );
                }
            }
        }

        // Nothing but the created properties moved: the model keeps every
        // property it held.
        for held in &held_ids {
            assert!(
                ids(&after).contains(held),
                "case {name:?}: the held property {held} is gone from {after:?}"
            );
        }
        let created_count = case
            .expected
            .iter()
            .filter(|(_, target)| matches!(target, Target::Created(_)))
            .count();
        assert_eq!(
            ids(&after).len(),
            held_ids.len() + created_count,
            "case {name:?}: expected {} properties after the application, got {after:?}",
            held_ids.len() + created_count
        );
    }
}

#[tokio::test]
async fn a_hierarchy_mapping_pairs_the_hierarchies_both_models_name() {
    let case = "hierarchy mappings";
    let router = router();
    let child = model_with(&router, case, "Child", "ASSET_MODEL", &["Temperature"]).await;

    // The interface declares two hierarchies; the applied-to model declares one
    // of those names and one of its own.
    let interface = create_model(
        &router,
        case,
        json!({
            "assetModelName": "InterfaceWithHierarchies",
            "assetModelType": "INTERFACE",
            "assetModelProperties": [property("Temperature")],
            "assetModelHierarchies": [
                {"name": "Sensors", "childAssetModelId": child},
                {"name": "Pumps", "childAssetModelId": child},
            ],
        }),
    )
    .await;
    let asset_model = create_model(
        &router,
        case,
        json!({
            "assetModelName": "AppliedToWithHierarchies",
            "assetModelProperties": [property("Temperature")],
            "assetModelHierarchies": [
                {"name": "Sensors", "childAssetModelId": child},
                {"name": "Valves", "childAssetModelId": child},
            ],
        }),
    )
    .await;

    let (status, applied) = call(
        &router,
        "PUT",
        &relationship_uri(&asset_model, &interface),
        Some(json!({
            "propertyMappingConfiguration": { "matchByPropertyName": true }
        })),
    )
    .await;
    assert_eq!(status, 202, "case {case}: the application got {status} {applied:?}");

    let (status, read) = call(
        &router,
        "GET",
        &relationship_uri(&asset_model, &interface),
        None,
    )
    .await;
    assert_eq!(status, 200, "case {case}: the read got {status} {read:?}");

    let interface_model = describe_model(&router, case, &interface).await;
    let applied_model = describe_model(&router, case, &asset_model).await;
    let interface_sensors = id_of(&interface_model["assetModelHierarchies"], case, "Sensors");
    let applied_sensors = id_of(&applied_model["assetModelHierarchies"], case, "Sensors");

    assert_eq!(
        read["hierarchyMappings"],
        json!([{
            "interfaceAssetModelHierarchyId": interface_sensors,
            "assetModelHierarchyId": applied_sensors,
        }]),
        "case {case}: expected only the shared hierarchy to be mapped, got {:?}",
        read["hierarchyMappings"]
    );
}

#[tokio::test]
async fn the_relationship_is_read_back_by_either_label_form_and_re_application_replaces_it() {
    let case = "labels and re-application";
    let router = router();
    let interface = create_model(
        &router,
        case,
        json!({
            "assetModelName": "Interface",
            "assetModelType": "INTERFACE",
            "assetModelExternalId": "interface.one",
            "assetModelProperties": [property("Temperature")],
        }),
    )
    .await;
    let asset_model = create_model(
        &router,
        case,
        json!({
            "assetModelName": "AppliedTo",
            "assetModelExternalId": "applied.one",
            "assetModelProperties": [property("Temperature"), property("InletTemp")],
        }),
    )
    .await;
    let interface_property =
        id_of(&describe_model(&router, case, &interface).await["assetModelProperties"], case, "Temperature");
    let model_properties = describe_model(&router, case, &asset_model).await["assetModelProperties"].clone();
    let by_name = id_of(&model_properties, case, "Temperature");
    let overridden = id_of(&model_properties, case, "InletTemp");

    // A pair nothing applied is a not-found, not an empty answer.
    let (status, missing) = call(
        &router,
        "GET",
        &relationship_uri(&asset_model, &interface),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case}: an unapplied pair must refuse, got {status} {missing:?}"
    );
    assert!(
        error_code(&missing).contains("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {:?}",
        error_code(&missing)
    );

    // Applied, and read back by the `externalId:` form of BOTH labels.
    let (status, applied) = call(
        &router,
        "PUT",
        &relationship_uri("externalId:applied.one", "externalId:interface.one"),
        Some(json!({
            "propertyMappingConfiguration": { "matchByPropertyName": true }
        })),
    )
    .await;
    assert_eq!(status, 202, "case {case}: the application got {status} {applied:?}");
    assert_eq!(
        applied["assetModelId"], json!(asset_model),
        "case {case}: expected the resolved assetModelId {asset_model}, got {:?}",
        applied["assetModelId"]
    );

    for (form, uri) in [
        ("raw ids", relationship_uri(&asset_model, &interface)),
        (
            "externalId labels",
            relationship_uri("externalId:applied.one", "externalId:interface.one"),
        ),
    ] {
        let (status, read) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case}/{form}: the read got {status} {read:?}");
        assert_eq!(
            read["propertyMappings"],
            json!([{
                "interfaceAssetModelPropertyId": interface_property,
                "assetModelPropertyId": by_name,
            }]),
            "case {case}/{form}: expected the name match, got {:?}",
            read["propertyMappings"]
        );
        assert_eq!(
            read["assetModelId"], json!(asset_model),
            "case {case}/{form}: expected assetModelId {asset_model}, got {:?}",
            read["assetModelId"]
        );
        assert_eq!(
            read["hierarchyMappings"],
            json!([]),
            "case {case}/{form}: expected no hierarchy mappings, got {:?}",
            read["hierarchyMappings"]
        );
    }

    // Re-applying the same interface REPLACES the relationship: the second
    // configuration is what the read reports.
    let (status, reapplied) = call(
        &router,
        "PUT",
        &relationship_uri(&asset_model, &interface),
        Some(json!({
            "propertyMappingConfiguration": {
                "overrides": [{
                    "interfaceAssetModelPropertyId": interface_property,
                    "assetModelPropertyId": overridden,
                }],
            }
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case}: the second application got {status} {reapplied:?}"
    );
    let (status, read) = call(
        &router,
        "GET",
        &relationship_uri(&asset_model, &interface),
        None,
    )
    .await;
    assert_eq!(status, 200, "case {case}: the second read got {status} {read:?}");
    assert_eq!(
        read["propertyMappings"],
        json!([{
            "interfaceAssetModelPropertyId": interface_property,
            "assetModelPropertyId": overridden,
        }]),
        "case {case}: expected the overridden mapping, got {:?}",
        read["propertyMappings"]
    );
}

#[tokio::test]
async fn re_applying_the_same_interface_keeps_the_mappings_it_resolved() {
    let case = "idempotent re-application";
    let router = router();
    let interface = model_with(
        &router,
        case,
        "Interface",
        "INTERFACE",
        &["Temperature", "Pressure"],
    )
    .await;
    let asset_model = model_with(&router, case, "AppliedTo", "ASSET_MODEL", &["Serial"]).await;
    // Creation only: no matching by name, so a second application would collide
    // on the names the first one created unless the mapping it resolved stands.
    let creation_only = json!({
        "propertyMappingConfiguration": { "createMissingProperty": true }
    });

    let mut answers = Vec::new();
    for attempt in ["first", "second"] {
        let (status, applied) = call(
            &router,
            "PUT",
            &relationship_uri(&asset_model, &interface),
            Some(creation_only.clone()),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case}/{attempt}: the application got {status} {applied:?}"
        );
        let (status, read) = call(
            &router,
            "GET",
            &relationship_uri(&asset_model, &interface),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case}/{attempt}: the read got {status} {read:?}");
        answers.push(read["propertyMappings"].clone());
    }
    assert_eq!(
        answers[0], answers[1],
        "case {case}: expected the second application to keep {:?}, got {:?}",
        answers[0], answers[1]
    );
    // And it created nothing a second time.
    let properties = describe_model(&router, case, &asset_model).await["assetModelProperties"].clone();
    assert_eq!(
        ids(&properties).len(),
        3,
        "case {case}: expected the model to carry its own property plus the two created once, \
         got {properties:?}"
    );
}

#[tokio::test]
async fn two_interface_properties_cannot_share_one_target() {
    let case = "one target per interface property";
    let router = router();
    let interface = model_with(
        &router,
        case,
        "Interface",
        "INTERFACE",
        &["Temperature", "Pressure"],
    )
    .await;
    let asset_model = model_with(&router, case, "AppliedTo", "ASSET_MODEL", &["Temperature"]).await;
    let interface_properties =
        describe_model(&router, case, &interface).await["assetModelProperties"].clone();
    let shared = id_of(
        &describe_model(&router, case, &asset_model).await["assetModelProperties"],
        case,
        "Temperature",
    );

    // `Pressure` is overridden onto the model's `Temperature`, which `Temperature`
    // also matches by name: one property of the model, two of the interface.
    let (status, body) = call(
        &router,
        "PUT",
        &relationship_uri(&asset_model, &interface),
        Some(json!({
            "propertyMappingConfiguration": {
                "matchByPropertyName": true,
                "overrides": [{
                    "interfaceAssetModelPropertyId": id_of(&interface_properties, case, "Pressure"),
                    "assetModelPropertyId": shared,
                }],
            }
        })),
    )
    .await;
    assert_eq!(status, 400, "case {case}: expected 400, got {status} {body:?}");
    assert!(
        error_code(&body).contains("InvalidRequestException"),
        "case {case}: expected InvalidRequestException, got {:?}",
        error_code(&body)
    );
}

#[tokio::test]
async fn a_request_outside_the_rule_is_the_modelled_refusal() {
    let case = "refusals";
    let router = router();
    let interface = model_with(&router, case, "Interface", "INTERFACE", &["Temperature"]).await;
    let asset_model = model_with(&router, case, "AppliedTo", "ASSET_MODEL", &["Temperature"]).await;
    let plain = model_with(&router, case, "Plain", "ASSET_MODEL", &["Temperature"]).await;
    let unknown = "11111111-2222-3333-4444-555555555555";
    let matching = json!({ "propertyMappingConfiguration": { "matchByPropertyName": true } });

    for (row, uri, expected_status, expected_code) in [
        (
            "an asset model that does not exist",
            relationship_uri(unknown, &interface),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an interface asset model that does not exist",
            relationship_uri(&asset_model, unknown),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a model that is not an interface applied as one",
            relationship_uri(&asset_model, &plain),
            400,
            "InvalidRequestException",
        ),
        (
            "an interface applied to itself",
            relationship_uri(&interface, &interface),
            400,
            "InvalidRequestException",
        ),
    ] {
        let (status, body) = call(&router, "PUT", &uri, Some(matching.clone())).await;
        assert_eq!(
            status, expected_status,
            "case {case}/{row}: expected {expected_status}, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains(expected_code),
            "case {case}/{row}: expected {expected_code}, got {:?}",
            error_code(&body)
        );
        // The read refuses for the same reason the write did.
        let (status, read) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case}/{row}: the read must refuse, got {status} {read:?}"
        );
    }

    // An override naming a property neither model declares names nothing.
    for (row, overrides) in [
        (
            "an override naming no interface property",
            json!([{
                "interfaceAssetModelPropertyId": unknown,
                "assetModelPropertyId": id_of(
                    &describe_model(&router, case, &asset_model).await["assetModelProperties"],
                    case,
                    "Temperature",
                ),
            }]),
        ),
        (
            "an override naming no property of the applied-to model",
            json!([{
                "interfaceAssetModelPropertyId": id_of(
                    &describe_model(&router, case, &interface).await["assetModelProperties"],
                    case,
                    "Temperature",
                ),
                "assetModelPropertyId": unknown,
            }]),
        ),
    ] {
        let (status, body) = call(
            &router,
            "PUT",
            &relationship_uri(&asset_model, &interface),
            Some(json!({
                "propertyMappingConfiguration": {
                    "matchByPropertyName": true,
                    "overrides": overrides,
                }
            })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}/{row}: expected 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case}/{row}: expected ResourceNotFoundException, got {:?}",
            error_code(&body)
        );
    }

    // A refused application left the pair unrelated.
    let (status, read) = call(
        &router,
        "GET",
        &relationship_uri(&asset_model, &interface),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case}: no refused application may relate the pair, got {status} {read:?}"
    );
}

#[tokio::test]
async fn applying_an_interface_moves_the_applied_to_model_and_its_assets() {
    let case = "the applied-to model moves";
    let router = router();
    let interface = model_with(
        &router,
        case,
        "Interface",
        "INTERFACE",
        &["Temperature", "Pressure"],
    )
    .await;
    let asset_model = model_with(&router, case, "AppliedTo", "ASSET_MODEL", &["Temperature"]).await;

    // An asset of the model, created before the interface is applied: it
    // inherits the model's definition, so the property the application creates
    // reaches it too.
    let (status, asset) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({ "assetName": "Unit", "assetModelId": asset_model })),
    )
    .await;
    assert_eq!(status, 202, "case {case}: CreateAsset got {status} {asset:?}");
    let asset_id = asset["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no assetId in {asset:?}"))
        .to_string();

    // Past the create's own owed transition, so the application's is the one
    // under test.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let before = describe_model(&router, case, &asset_model).await;
    let before_version = before["assetModelVersion"].as_str().unwrap_or("").to_string();
    assert_eq!(
        before["assetModelStatus"]["state"], json!("ACTIVE"),
        "case {case}: expected the model to have settled, got {:?}",
        before["assetModelStatus"]
    );

    let (status, applied) = call(
        &router,
        "PUT",
        &relationship_uri(&asset_model, &interface),
        Some(json!({
            "propertyMappingConfiguration": {
                "matchByPropertyName": true,
                "createMissingProperty": true,
            }
        })),
    )
    .await;
    assert_eq!(status, 202, "case {case}: the application got {status} {applied:?}");
    assert_eq!(
        applied["assetModelStatus"]["state"], json!("UPDATING"),
        "case {case}: expected UPDATING while the owed transition is pending, got {:?}",
        applied["assetModelStatus"]
    );
    assert!(
        applied["assetModelArn"]
            .as_str()
            .is_some_and(|arn| arn.contains(&asset_model)),
        "case {case}: expected the model's own ARN, got {:?}",
        applied["assetModelArn"]
    );

    let after = describe_model(&router, case, &asset_model).await;
    assert_ne!(
        after["assetModelVersion"].as_str().unwrap_or(""),
        before_version,
        "case {case}: expected a new version, got {:?}",
        after["assetModelVersion"]
    );

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let settled = describe_model(&router, case, &asset_model).await;
    assert_eq!(
        settled["assetModelStatus"]["state"], json!("ACTIVE"),
        "case {case}: expected the owed transition to settle, got {:?}",
        settled["assetModelStatus"]
    );

    // The created property is on the model, and inherited by its asset.
    let created = id_of(&settled["assetModelProperties"], case, "Pressure");
    let (status, read_asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "case {case}: DescribeAsset got {status} {read_asset:?}");
    assert!(
        read_asset["assetProperties"]
            .as_array()
            .is_some_and(|properties| properties
                .iter()
                .any(|property| property["id"] == json!(created))),
        "case {case}: expected the asset to inherit the created property {created}, got {:?}",
        read_asset["assetProperties"]
    );
}
