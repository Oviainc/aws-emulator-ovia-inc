// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `interfaceSummaries` member every
//! `ListAssetModelProperties` summary carries, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a property of an asset model belongs to exactly
//! those interface asset models whose application to that model mapped one of
//! their properties to it, so each summary reports one entry per such mapping,
//! naming the interface asset model and the property of it this property answers
//! for, and no entry — the member absent — for a property no application mapped.
//! The mappings are the stored ones, so this listing and
//! `DescribeAssetModelInterfaceRelationship` cannot disagree, and deleting the
//! relationship takes the entries with it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, is_modelled_id, router};
use crate::asset_model_interface_relationship::{
    create_model, describe_model, id_of, model_with, property, relationship_uri,
};

/// One interface asset model a case applies to the model under test, and the
/// `propertyMappingConfiguration` it is applied with.
struct Interface {
    /// The property names the interface asset model declares.
    properties: &'static [&'static str],
    match_by_property_name: bool,
    create_missing_property: bool,
    /// `(interface property, applied-to model property)` name pairs the
    /// application overrides the mapping of.
    overrides: &'static [(&'static str, &'static str)],
}

/// One row of the table: a model, the interfaces applied to it, and the entries
/// every one of its property summaries must then report.
struct Case {
    name: &'static str,
    /// The property names the applied-to asset model declares.
    model_properties: &'static [&'static str],
    /// The interfaces applied, in order; an entry's index is how `expected`
    /// names it.
    interfaces: &'static [Interface],
    /// Per property name of the applied-to model, the `(interface index,
    /// interface property name)` pairs its summary must report. A property this
    /// does not name must carry no `interfaceSummaries` member at all.
    expected: &'static [(&'static str, &'static [(usize, &'static str)])],
}

/// The table: the rule is one entry per stored mapping that names the property,
/// so the rows differ in how many interfaces are applied, how many of a model's
/// properties one covers, how a mapping was resolved (by name, by an override,
/// by creating the property) and whether any interface is applied at all.
const CASES: &[Case] = &[
    Case {
        name: "no interface applied: every property belongs to none",
        model_properties: &["Temperature", "Pressure"],
        interfaces: &[],
        expected: &[],
    },
    Case {
        name: "one interface matched by name covers only the properties it declares",
        model_properties: &["Temperature", "Pressure", "Humidity"],
        interfaces: &[Interface {
            properties: &["Temperature", "Pressure"],
            match_by_property_name: true,
            create_missing_property: false,
            overrides: &[],
        }],
        expected: &[
            ("Temperature", &[(0, "Temperature")]),
            ("Pressure", &[(0, "Pressure")]),
        ],
    },
    Case {
        name: "two interfaces: a property answers for one property of each",
        model_properties: &["Temperature", "Pressure"],
        interfaces: &[
            Interface {
                properties: &["Temperature"],
                match_by_property_name: true,
                create_missing_property: false,
                overrides: &[],
            },
            Interface {
                properties: &["Temperature", "Pressure"],
                match_by_property_name: true,
                create_missing_property: false,
                overrides: &[],
            },
        ],
        expected: &[
            ("Temperature", &[(0, "Temperature"), (1, "Temperature")]),
            ("Pressure", &[(1, "Pressure")]),
        ],
    },
    Case {
        name: "an override pairs an interface property with a differently named property",
        model_properties: &["Temperature", "Ambient"],
        interfaces: &[Interface {
            properties: &["Outside"],
            match_by_property_name: false,
            create_missing_property: false,
            overrides: &[("Outside", "Ambient")],
        }],
        expected: &[("Ambient", &[(0, "Outside")])],
    },
    Case {
        name: "a property the application created belongs to the interface that asked for it",
        model_properties: &["Temperature"],
        interfaces: &[Interface {
            properties: &["Pressure"],
            match_by_property_name: false,
            create_missing_property: true,
            overrides: &[],
        }],
        expected: &[("Pressure", &[(0, "Pressure")])],
    },
];

/// Every summary `ListAssetModelProperties` reports for `model`, under the `ALL`
/// filter so the rows its composite models contribute are listed too.
async fn list_all(router: &MultiServiceRouter, case: &str, model: &str) -> Vec<Value> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/asset-models/{model}/properties?maxResults=250&filter=ALL"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAssetModelProperties got {status} {body}"
    );
    body["assetModelPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetModelPropertySummaries in {body}"))
        .clone()
}

/// The `(interfaceAssetModelId, interfaceAssetModelPropertyId)` pairs one summary
/// reports, or `None` when it carries no `interfaceSummaries` member at all.
fn reported_pairs(summary: &Value, case: &str) -> Option<BTreeSet<(String, String)>> {
    let member = summary
        .get("interfaceSummaries")
        .filter(|value| !value.is_null())?;
    let entries = member
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: interfaceSummaries is not a list in {summary}"));
    Some(
        entries
            .iter()
            .map(|entry| {
                let interface = entry["interfaceAssetModelId"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: no interfaceAssetModelId in {entry}")
                });
                let interface_property = entry["interfaceAssetModelPropertyId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no interfaceAssetModelPropertyId in {entry}")
                    });
                assert!(
                    is_modelled_id(interface),
                    "case {case:?}: interfaceAssetModelId {interface} is not a modelled ID"
                );
                assert!(
                    is_modelled_id(interface_property),
                    "case {case:?}: interfaceAssetModelPropertyId {interface_property} is not a \
                     modelled ID"
                );
                (interface.to_string(), interface_property.to_string())
            })
            .collect(),
    )
}

/// The name a summary carries.
fn summary_name(summary: &Value, case: &str) -> String {
    summary["name"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a summary with no name: {summary}"))
        .to_string()
}

#[tokio::test]
async fn interface_summaries_name_every_interface_property_a_property_answers_for() {
    for case in CASES {
        let router = router();
        let name = case.name;
        let model = model_with(
            &router,
            name,
            "AppliedTo",
            "ASSET_MODEL",
            case.model_properties,
        )
        .await;

        // Each interface in turn: created, then applied to the model under test
        // with the configuration the row states.
        let mut interfaces: Vec<(String, Value)> = Vec::new();
        for (at, interface) in case.interfaces.iter().enumerate() {
            let interface_id = model_with(
                &router,
                name,
                &format!("Interface{at}"),
                "INTERFACE",
                interface.properties,
            )
            .await;
            let interface_properties = describe_model(&router, name, &interface_id).await
                ["assetModelProperties"]
                .clone();
            let held =
                describe_model(&router, name, &model).await["assetModelProperties"].clone();
            let overrides: Vec<Value> = interface
                .overrides
                .iter()
                .map(|(interface_property, model_property)| {
                    json!({
                        "interfaceAssetModelPropertyId":
                            id_of(&interface_properties, name, interface_property),
                        "assetModelPropertyId": id_of(&held, name, model_property),
                    })
                })
                .collect();
            let (status, applied) = call(
                &router,
                "PUT",
                &relationship_uri(&model, &interface_id),
                Some(json!({
                    "propertyMappingConfiguration": {
                        "matchByPropertyName": interface.match_by_property_name,
                        "createMissingProperty": interface.create_missing_property,
                        "overrides": overrides,
                    }
                })),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {name:?}: applying interface {at} got {status} {applied}"
            );
            interfaces.push((interface_id, interface_properties));
        }

        // What the row expects, as the two ids the wire carries.
        let mut expected: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
        for (property_name, pairs) in case.expected {
            let entry = expected.entry(property_name.to_string()).or_default();
            for (at, interface_property) in *pairs {
                let (interface_id, interface_properties) = &interfaces[*at];
                entry.insert((
                    interface_id.clone(),
                    id_of(interface_properties, name, interface_property),
                ));
            }
        }

        let summaries = list_all(&router, name, &model).await;
        let listed: BTreeSet<String> = summaries
            .iter()
            .map(|summary| summary_name(summary, name))
            .collect();
        for property_name in expected.keys() {
            assert!(
                listed.contains(property_name),
                "case {name:?}: the property {property_name} the row expects is not listed: \
                 {listed:?}"
            );
        }

        for summary in &summaries {
            let property_name = summary_name(summary, name);
            let actual = reported_pairs(summary, name);
            match expected.get(&property_name) {
                None => assert!(
                    actual.is_none(),
                    "case {name:?}: the property {property_name} belongs to no interface, so it \
                     must carry no interfaceSummaries, got {actual:?}"
                ),
                Some(pairs) => assert_eq!(
                    actual.as_ref(),
                    Some(pairs),
                    "case {name:?}: expected the property {property_name} to report {pairs:?}, \
                     got {actual:?}"
                ),
            }
        }

        // The stored mappings are the only source: every pair the listing reports
        // for one interface is a mapping `DescribeAssetModelInterfaceRelationship`
        // reports for the same pair of models, and every one of those is reported
        // by the listing.
        for (interface_id, _) in &interfaces {
            let (status, read) =
                call(&router, "GET", &relationship_uri(&model, interface_id), None).await;
            assert_eq!(
                status, 200,
                "case {name:?}: DescribeAssetModelInterfaceRelationship got {status} {read}"
            );
            let stored: BTreeSet<(String, String)> = read["propertyMappings"]
                .as_array()
                .unwrap_or_else(|| panic!("case {name:?}: no propertyMappings in {read}"))
                .iter()
                .map(|mapping| {
                    (
                        mapping["assetModelPropertyId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        mapping["interfaceAssetModelPropertyId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            let from_listing: BTreeSet<(String, String)> = summaries
                .iter()
                .flat_map(|summary| {
                    let id = summary["id"].as_str().unwrap_or_default().to_string();
                    reported_pairs(summary, name)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(interface, _)| interface == interface_id)
                        .map(move |(_, interface_property)| (id.clone(), interface_property))
                        .collect::<Vec<(String, String)>>()
                })
                .collect();
            assert_eq!(
                from_listing, stored,
                "case {name:?}: the listing must report exactly the stored mappings of the \
                 interface {interface_id}, got {from_listing:?} against {stored:?}"
            );
        }

        // The entries are ordered, so two reads of one store report one answer.
        let again = list_all(&router, name, &model).await;
        assert_eq!(
            again, summaries,
            "case {name:?}: two reads of one store must report one order"
        );
    }
}

#[tokio::test]
async fn a_deleted_relationship_takes_the_interface_summaries_of_its_mappings_with_it() {
    let case = "the listing follows the store";
    let router = router();
    // A model whose properties are its own AND a composite model's: a mapping
    // names a property of the model, so the contributed rows belong to no
    // interface whatever is applied.
    let model = create_model(
        &router,
        case,
        json!({
            "assetModelName": "AppliedTo",
            "assetModelProperties": [property("Temperature"), property("Pressure")],
            "assetModelCompositeModels": [{
                "name": "HighTemp",
                "type": "AWS/ALARM",
                "properties": [{
                    "name": "State",
                    "dataType": "STRUCT",
                    "dataTypeSpec": "AWS/ALARM_STATE",
                    "type": {"measurement": {}},
                }],
            }],
        }),
    )
    .await;
    let interface = model_with(&router, case, "InterfaceModel", "INTERFACE", &["Temperature"]).await;
    let interface_properties =
        describe_model(&router, case, &interface).await["assetModelProperties"].clone();
    let expected = BTreeSet::from([(
        interface.clone(),
        id_of(&interface_properties, case, "Temperature"),
    )]);

    let apply = json!({
        "propertyMappingConfiguration": {
            "matchByPropertyName": true,
            "createMissingProperty": false,
            "overrides": [],
        }
    });

    // Applied, deleted, applied again: the member tracks the stored relationship
    // on every step, so the whole cycle is one assertion per step.
    for step in ["applied", "deleted", "re-applied"] {
        let (method, body) = match step {
            "deleted" => ("DELETE", None),
            _ => ("PUT", Some(apply.clone())),
        };
        let (status, answer) = call(
            &router,
            method,
            &relationship_uri(&model, &interface),
            body,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}/{step}: {method} the relationship got {status} {answer}"
        );

        let summaries = list_all(&router, case, &model).await;
        for summary in &summaries {
            let property_name = summary_name(summary, case);
            let actual = reported_pairs(summary, case);
            let expected = (step != "deleted" && property_name == "Temperature")
                .then(|| expected.clone());
            assert_eq!(
                actual, expected,
                "case {case:?}/{step}: expected the property {property_name} to report \
                 {expected:?}, got {actual:?}"
            );
        }
    }
}
